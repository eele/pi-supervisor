//! Process helpers: timed command execution with the sbx env fixes, plus a
//! global registry of live child processes so app exit can reap them (a hung
//! `sbx exec` child would otherwise outlive the UI).

use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Exit-code sentinel we surface on timeout (no system meaning).
pub const TIMEOUT_ERR: &str = "timed out";

/// CREATE_NO_WINDOW: keep release-build console hosts from flashing.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn children() -> &'static Mutex<Vec<u32>> {
    static REG: OnceLock<Mutex<Vec<u32>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Vec::new()))
}

/// NO_PROXY injected into every child unless the parent already sets it.
/// configure_no_proxy() replaces the default at startup from config.json.
static NO_PROXY: OnceLock<String> = OnceLock::new();

/// Derive the child NO_PROXY list from the configured kit repo: the registry
/// host plus its parent domain (`.byte.art` style suffix keeps cert/auth
/// endpoints off the corporate proxy).
pub fn configure_no_proxy(repo: &str) {
    let host = repo.split('/').next().unwrap_or(repo);
    let parent = host
        .split_once('.')
        .map(|(_, rest)| format!(",.{rest}"))
        .unwrap_or_default();
    // Merge with anything the launching shell already set (never drop it).
    let mut value = format!("localhost,127.0.0.1,127.0.0.1:*,{host}{parent}");
    if let Some(pre) = std::env::var("NO_PROXY").ok().filter(|s| !s.is_empty()) {
        value = format!("{pre},{value}");
    }
    let _ = NO_PROXY.set(value.clone());
    // Also exempt the registry from THIS process's proxy env: ureq honours
    // HTTPS_PROXY/NO_PROXY, and the app is often launched from a proxied
    // shell — zot traffic must stay direct (corporate proxy returns 500).
    std::env::set_var("NO_PROXY", &value);
    std::env::set_var("no_proxy", &value);
}

fn no_proxy() -> &'static str {
    NO_PROXY.get_or_init(|| "localhost,127.0.0.1,127.0.0.1:*,zot.byte.art,.byte.art".into())
}

fn register(pid: u32) {
    if let Ok(mut v) = children().lock() {
        v.push(pid);
    }
}

/// Child exited on its own — drop it from the reap registry. Without this
/// the list grows for every poll cycle (prober/poller/notify each spawn one,
/// thousands per day) and the exit-time kill_all_children serializes a
/// taskkill per stale pid: tray Exit appeared to hang for minutes.
fn deregister(pid: u32) {
    if let Ok(mut v) = children().lock() {
        v.retain(|p| *p != pid);
    }
}

/// Kill every registered child that is still running (called on app exit).
pub fn kill_all_children() {
    let Ok(mut v) = children().lock() else { return };
    for pid in v.drain(..) {
        // taskkill by pid; /T takes down any process tree sbx may have spawned.
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// Run `cmd args…` to completion with a wall-clock timeout.
///
/// - `stdin` is piped (one-shot write then closed).
/// - Env: NO_PROXY gets the localhost + configured registry host entries
///   (see configure_no_proxy) unless already set — the corporate proxy
///   otherwise cuts sbx/zot traffic (pi-build README hardens this same fix).
/// - The child pid is registered for exit-time reaping.
pub fn run_cmd(
    program: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    timeout_ms: u64,
) -> Result<Output, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if std::env::var("NO_PROXY").is_err() {
        cmd.env("NO_PROXY", no_proxy());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    let pid = child.id();
    register(pid);

    if let Some(data) = stdin {
        if let Some(mut si) = child.stdin.take() {
            // A closed pipe means the child died early; wait() below reports it.
            let _ = si.write_all(data);
            let _ = si.flush();
        }
    }

    // Drain pipes on reader threads (a big stdout never deadlocks the child)
    // while the caller polls try_wait() — keeping the Child here is what
    // makes the timeout kill possible.
    let (tx, rx) = mpsc::channel::<(u8, Vec<u8>)>();
    if let Some(mut so) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = so.read_to_end(&mut buf);
            let _ = tx.send((0, buf));
        });
    }
    if let Some(mut se) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = se.read_to_end(&mut buf);
            let _ = tx.send((1, buf));
        });
    }
    drop(tx);

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed().as_millis() as u64 > timeout_ms {
                    let _ = child.kill();
                    let _ = child.wait();
                    deregister(pid);
                    return Err(format!("{program} {TIMEOUT_ERR} after {timeout_ms} ms"));
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err(e) => return Err(format!("cannot wait for {program}: {e}")),
        }
    };
    // Reaped on its own: nothing left to kill at app exit.
    deregister(pid);
    // Readers end as soon as the process dies; 5 s is generous headroom.
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    while let Ok((which, buf)) = rx.recv_timeout(std::time::Duration::from_secs(5)) {
        if which == 0 {
            stdout = buf;
        } else {
            stderr = buf;
        }
    }
    let out = Output {
        status,
        stdout,
        stderr,
    };
    if out.status.success() {
        Ok(out)
    } else {
        let mut msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.is_empty() {
            msg = String::from_utf8_lossy(&out.stdout).trim().to_string();
        }
        Err(format!(
            "{program} exited with {}{}",
            out.status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
            if msg.is_empty() {
                String::new()
            } else {
                format!(": {msg}")
            }
        ))
    }
}

/// Convenience: run + capture stdout as UTF-8 string.
pub fn run_out(program: &str, args: &[String], timeout_ms: u64) -> Result<String, String> {
    run_cmd(program, args, None, timeout_ms).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// sbx wrapper: `sbx <args>` with a generous exec timeout.
pub fn sbx(args: &[&str], timeout_ms: u64) -> Result<String, String> {
    run_out(
        "sbx",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        timeout_ms,
    )
}

/// `sbx exec` with token data on stdin (tokens never travel via argv).
pub fn sbx_exec_stdin(
    sandbox: &str,
    shell_cmd: &str,
    data: &[u8],
    timeout_ms: u64,
) -> Result<String, String> {
    let args: Vec<String> = ["exec", sandbox, "--", "sh", "-c", shell_cmd]
        .iter()
        .map(|s| s.to_string())
        .collect();
    run_cmd("sbx", &args, Some(data), timeout_ms)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Nonblocking cooperative flag helper.
pub struct Flag(pub AtomicBool);
impl Flag {
    pub fn new(v: bool) -> Self {
        Self(AtomicBool::new(v))
    }
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    pub fn set(&self, v: bool) {
        self.0.store(v, Ordering::Relaxed);
    }
}

/// Strip ANSI CSI sequences + CR so line parsing sees plain text.
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            // ESC [ params … final-byte | ESC ] … BEL/ST : drop the sequence
            match it.peek() {
                Some('[') | Some(']') => {
                    it.next();
                    for n in it.by_ref() {
                        if matches!(n, 'a'..='z' | 'A'..='Z') || n == '\x07' {
                            break;
                        }
                    }
                }
                _ => {
                    it.next();
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_ansi_and_cr() {
        let raw = "socket_20261025.091612.863\u{1b}[0;0m \u{1b}[32mlive\u{1b}[0m\r\n";
        assert_eq!(
            clean_text(raw),
            "socket_20261025.091612.863 live\n"
        );
    }

    #[test]
    fn clean_text_passthrough_plain() {
        assert_eq!(clean_text("a b c"), "a b c");
    }
}
