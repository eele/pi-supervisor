//! I2 — sandbox state collection + Recreate orchestration.
//!
//! Probe: `sbx inspect pi-ai` (keyed human text, parsed leniently).
//! Recreate: rm --force → resolve latest zot kit → sbx run -d (permanent
//! residency, no auto-stop → no keepalive needed) → wait ready → push tokens.

use std::sync::Arc;

use serde::Serialize;

use crate::state::{AppState, RecreateStep};
use crate::{tokens, util, zot};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxState {
    Detached, // sbx reports running for the detached-run sandbox
    Stopped,
    NotCreated,
}

#[derive(Serialize, Clone)]
pub struct SandboxInfo {
    pub state: SandboxState,
    pub kit: String,
    pub mount_path: String,
    /// Configured kit repo (config.json `zot_repo`, no tag) — lets the UI
    /// describe the Recreate source without hardcoding the registry.
    pub kit_repo: String,
}

pub fn probe(name: &str) -> Option<SandboxInfo> {
    // run_cmd folds stderr into the Err string; scan either form so the
    // "not found" marker is always visible to the parser.
    let text = util::sbx(&["inspect", name], 30_000).unwrap_or_else(|e| e);
    parse_inspect(&text)
}

/// `sbx inspect` fields (v0.46):
///   "  Kits:      zot.byte.art/blackbox/pi-kit:<stamp>"
///   "  State:     running (2h 17m)"        / "stopped"
///   "  Workspace: C:\\Users\\ele\\Tea"
/// None = unparseable transient (keep the previous card rather than flash
/// a wrong state); NotCreated only on the explicit "not found" marker.
pub fn parse_inspect(text: &str) -> Option<SandboxInfo> {
    if text.trim().is_empty() {
        return None;
    }
    if text.to_lowercase().contains("not found") {
        return Some(SandboxInfo {
            state: SandboxState::NotCreated,
            kit: String::new(),
            mount_path: String::new(),
            kit_repo: String::new(),
        });
    }
    let field = |key: &str| -> Option<String> {
        text.lines()
            .find(|l| l.trim_start().starts_with(key))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
    };
    let state_raw = field("State:")?;
    let state = if state_raw.to_lowercase().starts_with("running") {
        SandboxState::Detached
    } else {
        SandboxState::Stopped
    };
    Some(SandboxInfo {
        state,
        kit: field("Kits:").unwrap_or_default(),
        mount_path: field("Workspace:").unwrap_or_default(),
        kit_repo: String::new(), // stamped by the caller that owns the config
    })
}

/// Kick off Recreate on a worker thread (single-flight: second call errors).
pub fn start_recreate(st: &Arc<AppState>) -> Result<(), String> {
    if st.recreate_busy.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Err("a recreate is already running".into());
    }
    let st2 = st.clone();
    std::thread::spawn(move || {
        run_recreate(&st2);
        st2.recreate_busy
            .store(false, std::sync::atomic::Ordering::SeqCst);
    });
    Ok(())
}

fn run_recreate(st: &Arc<AppState>) {
    let labels = ["rm", "resolve latest kit", "run -d", "wait ready", "push tokens"];
    {
        let mut r = st.recreate.lock().unwrap();
        r.active = true;
        r.error = None;
        r.steps = labels
            .iter()
            .map(|l| RecreateStep {
                label: l.to_string(),
                status: "pending".into(),
            })
            .collect();
    }
    let advance = |st: &AppState, i: usize, active: bool| {
        let mut r = st.recreate.lock().unwrap();
        if let Some(s) = r.steps.get_mut(i) {
            s.status = if active { "active" } else { "done" }.to_string();
        }
    };
    let fail = |st: &AppState, i: usize, e: String| {
        let mut r = st.recreate.lock().unwrap();
        r.active = false;
        r.error = Some(e);
        if let Some(s) = r.steps.get_mut(i) {
            s.status = "error".to_string();
        }
    };

    let cfg = st.cfg.lock().unwrap().clone();
    let name = cfg.sandbox_name.clone();
    let kit_repo = cfg.zot_repo.clone();

    // 1. rm — tolerate "not found" (pure-create path from not-created).
    advance(st, 0, true);
    if let Err(e) = util::sbx(&["rm", "--force", &name], 60_000) {
        if !e.to_lowercase().contains("not found") {
            return fail(st, 0, format!("rm failed: {e}"));
        }
    }
    advance(st, 0, false);

    // 2. resolve latest kit (zot, else config fallback).
    advance(st, 1, true);
    let kit = match zot::latest_kit(&cfg.zot_repo, &cfg.kit_fallback_tag) {
        Ok(k) => k,
        Err(e) => return fail(st, 1, e),
    };
    {
        let mut r = st.recreate.lock().unwrap();
        if let Some(s) = r.steps.get_mut(1) {
            s.label = format!(
                "resolve latest kit ({})",
                kit.rsplit(':').next().unwrap_or("")
            );
        }
    }
    advance(st, 1, false);

    // 3. detached run: permanent residency (the -d flag is what makes a
    //    separate keep-alive daemon unnecessary).
    advance(st, 2, true);
    let run_args = [
        "run",
        "-d",
        "--name",
        name.as_str(),
        kit.as_str(),
        cfg.mount_path.as_str(),
    ];
    if let Err(e) = util::sbx(&run_args, 180_000) {
        // Fresh machines ship kit.allowedSources=["docker.io/"] only, so the
        // zot kit is rejected by CLIENT-SIDE policy. Self-heal exactly the way
        // the error message suggests (merge the publisher prefix in) and
        // retry once; any other failure is reported as-is.
        if !e.to_lowercase().contains("allowlist") {
            return fail(st, 2, format!("sbx run failed: {e}"));
        }
        eprintln!("[sandbox] kit source not allowlisted — updating kit.allowedSources");
        let retried = ensure_kit_allowed(&cfg.zot_repo)
            .and_then(|()| util::sbx(&run_args, 180_000));
        if let Err(e2) = retried {
            return fail(st, 2, format!("sbx run failed: {e2}"));
        }
    }
    advance(st, 2, false);

    // 4. wait for the VM/agent to report running (≤ 120 s).
    advance(st, 3, true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        match probe(&name).map(|i| i.state) {
            Some(SandboxState::Detached) => break,
            _ if std::time::Instant::now() >= deadline => {
                return fail(st, 3, "sandbox did not reach running within 120 s".into())
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(1_500)),
        }
    }
    advance(st, 3, false);

    // 5. tokens.
    advance(st, 4, true);
    if let Err(e) = tokens::push_all(&name) {
        return fail(st, 4, e);
    }
    advance(st, 4, false);

    // Refresh the visible sandbox card, then done.
    if let Some(mut info) = probe(&name) {
        info.kit_repo = kit_repo;
        *st.sandbox.lock().unwrap() = info;
    }
    st.recreate.lock().unwrap().active = false;
}

/// Publisher prefix for an allowlist entry: repo minus its last segment
/// ("zot.byte.art/blackbox/pi-kit" → "zot.byte.art/blackbox/" — exactly what
/// the sbx error message itself proposes).
fn kit_allow_prefix(repo: &str) -> String {
    match repo.rfind('/') {
        Some(i) => repo[..i + 1].to_string(),
        None => format!("{repo}/"),
    }
}

/// Merge the kit's publisher prefix into `sbx` settings kit.allowedSources
/// (idempotent: no-op when an existing entry — or "*" — already covers it).
fn ensure_kit_allowed(repo: &str) -> Result<(), String> {
    let prefix = kit_allow_prefix(repo);
    let cur = util::sbx(&["settings", "get", "kit.allowedSources"], 15_000)?;
    let mut list: Vec<String> =
        serde_json::from_str(cur.trim()).unwrap_or_default();
    if list.iter().any(|s| s == "*" || repo.starts_with(s.as_str())) {
        return Ok(());
    }
    list.push(prefix);
    let json = serde_json::to_string(&list).map_err(|e| e.to_string())?;
    util::sbx(
        &["settings", "set", "kit.allowedSources", &json],
        15_000,
    )
    .map(|_| ())
}

/// Periodic prober thread: refresh the card every 3 s (cheap local CLI).
pub fn spawn_prober(st: &Arc<AppState>) {
    let st = st.clone();
    std::thread::spawn(move || loop {
        let (name, kit_repo) = {
            let cfg = st.cfg.lock().unwrap();
            (cfg.sandbox_name.clone(), cfg.zot_repo.clone())
        };
        if let Some(mut info) = probe(&name) {
            info.kit_repo = kit_repo;
            *st.sandbox.lock().unwrap() = info;
        }
        std::thread::sleep(std::time::Duration::from_millis(3_000));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_inspect() {
        let text = "  Name:           pi-ai\n  Kits:           zot.byte.art/blackbox/pi-kit:20261025.184126.782\n  State:          running (2h 17m)\n  Workspace:      C:\\Users\\ele\\Tea\n";
        let i = parse_inspect(text).unwrap();
        assert_eq!(i.state, SandboxState::Detached);
        assert_eq!(i.kit, "zot.byte.art/blackbox/pi-kit:20261025.184126.782");
        assert_eq!(i.mount_path, "C:\\Users\\ele\\Tea");
    }

    #[test]
    fn parses_stopped_and_missing() {
        let stopped =
            parse_inspect("  State:          stopped\n  Kits:           zot.byte.art/blackbox/pi-kit:20260101.000000.000\n")
                .unwrap();
        assert_eq!(stopped.state, SandboxState::Stopped);
        let missing = parse_inspect("error: sandbox 'no-such-box' not found\n  try: sbx ls").unwrap();
        assert_eq!(missing.state, SandboxState::NotCreated);
    }

    #[test]
    fn allow_prefix_is_the_publisher_dir() {
        assert_eq!(
            kit_allow_prefix("zot.byte.art/blackbox/pi-kit"),
            "zot.byte.art/blackbox/"
        );
        assert_eq!(kit_allow_prefix("host/repo"), "host/");
        assert_eq!(kit_allow_prefix("flat"), "flat/");
    }

    #[test]
    fn transient_garbage_keeps_previous_card() {
        assert!(parse_inspect("").is_none());
        assert!(parse_inspect("sbx timed out after 30000 ms").is_none());
    }
}
