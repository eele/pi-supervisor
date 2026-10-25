//! Pi-dtach session poller: every 3 s run
//!   sbx exec -it pi-ai -- bash -lc "pi-dtach -l"
//! and keep the parsed snapshot for the UI. Single-flight by construction
//! (one serial loop + a per-poll timeout kills hung execs).

use serde::Serialize;

use crate::state::AppState;
use crate::util;

#[derive(Serialize, Clone)]
pub struct SessionRow {
    pub name: String,
    pub live: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct SessionsSnapshot {
    pub has_data: bool,
    pub poll_error: String,
    pub sessions: Vec<SessionRow>,
}

/// Parse `pi-dtach -l` output: lines of `%40s %s` where status is "live" or
/// "dead (stale socket)". Tolerates CRLF (clean_text strips), ANSI noise and
/// any sbx preamble lines — only `socket_`-prefixed lines are rows.
pub fn parse_sessions(text: &str) -> Vec<SessionRow> {
    let mut rows = Vec::new();
    for line in util::clean_text(text).lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("socket_") else {
            continue;
        };
        // name column may be padded; status starts after the padding space
        let (name_part, status) = match rest.split_once(char::is_whitespace) {
            Some((n, s)) => (n, s.trim()),
            None => (rest, ""),
        };
        let live = status.eq_ignore_ascii_case("live");
        rows.push(SessionRow {
            name: format!("socket_{name_part}"),
            live,
        });
    }
    // Fixed-width stamp names sort newest-first lexicographically.
    rows.sort_by(|a, b| b.name.cmp(&a.name));
    rows
}

/// Strict socket name shape for Attach (defense against arg injection).
pub fn valid_socket_name(name: &str) -> bool {
    let Some(tail) = name.strip_prefix("socket_") else {
        return false;
    };
    !tail.is_empty()
        && tail
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// Open a NEW Windows Terminal running the interactive attach (`sbx exec -it`
/// needs a real TTY, which a GUI child process can never provide).
/// Falls back to `cmd /k` when wt.exe is unavailable (legacy Win10).
pub fn attach(name: &str) -> Result<(), String> {
    if !valid_socket_name(name) {
        return Err(format!("invalid session name: {name}"));
    }
    let inner = format!("sbx exec -it pi-ai -- bash -lc \"pi-dtach -a {name}\"");
    let mut ok = std::process::Command::new("wt.exe")
        .args(["cmd", "/k"])
        .arg(&inner)
        .spawn();
    if ok.is_err() {
        ok = std::process::Command::new("cmd")
            .args(["/k"])
            .arg(&inner)
            .spawn();
    }
    ok.map(|_| ()).map_err(|e| format!("cannot open terminal for attach: {e}"))
}

pub fn spawn_poller(state: &std::sync::Arc<AppState>) {
    let st = state.clone();
    std::thread::spawn(move || loop {
        let name = st.cfg.lock().unwrap().sandbox_name.clone();
        // Short-circuit when the sandbox is known missing: no point paying
        // the exec cost; UI shows "Sandbox not created" from sandbox state.
        let created = !matches!(
            st.sandbox.lock().unwrap().state,
            super::sandbox::SandboxState::NotCreated
        );
        if created {
            let r = util::sbx_exec_stdin(
                &name,
                "pi-dtach -l",
                b"",
                15_000,
            );
            let mut snap = st.sessions.lock().unwrap();
            match r {
                Ok(out) => {
                    snap.sessions = parse_sessions(&out);
                    snap.poll_error = String::new();
                    snap.has_data = true;
                }
                Err(e) => {
                    // Keep the previous frame rendered; just flag the error.
                    snap.poll_error = e;
                    snap.has_data = true;
                }
            }
        }
        if !created {
            // not-created still counts as "answered" (UI shows the empty state)
            st.sessions.lock().unwrap().has_data = true;
        }
        std::thread::sleep(std::time::Duration::from_millis(3_000));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_padded_live_and_dead() {
        let out = "socket_20261025.150210.447                    live\nsocket_20261024.221503.102                    dead (stale socket)\n";
        let rows = parse_sessions(out);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "socket_20261025.150210.447");
        assert!(rows[0].live);
        assert_eq!(rows[1].name, "socket_20261024.221503.102");
        assert!(!rows[1].live);
    }

    #[test]
    fn tolerates_noise_and_sorts_newest_first() {
        let out = "Sandbox pi-ai started.\r\nsocket_20261022.101010.100 live\r\nrandom junk\r\nsocket_20261025.091612.863 live\r\n";
        let rows = parse_sessions(out);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "socket_20261025.091612.863");
    }

    #[test]
    fn attach_name_validation() {
        assert!(valid_socket_name("socket_20261025.150210.447"));
        assert!(!valid_socket_name("socket_"));
        assert!(!valid_socket_name("pi-dtach; rm -rf /"));
        assert!(!valid_socket_name("socket_a;reboot"));
    }
}
