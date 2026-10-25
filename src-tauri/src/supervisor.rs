//! Service supervision: start/stop/intent persistence + watchdog.
//!
//! Index convention everywhere: 0=pageant 1=bridge 2=notify.

use std::sync::Arc;

use crate::services::{bridge, notify, pageant};
use crate::state::AppState;
use crate::util::Flag;

pub fn start(st: &Arc<AppState>, id: &str) -> Result<(), String> {
    match id {
        "pageant" => start_pageant(st),
        "bridge" => start_bridge(st),
        "notify" => start_notify(st),
        other => Err(format!("unknown service: {other}")),
    }
}

pub fn stop(st: &Arc<AppState>, id: &str) -> Result<(), String> {
    match id {
        "pageant" => {
            pageant::kill()?;
            st.set_service_state(0, "stopped", None);
            set_intent(st, "pageant", false);
            Ok(())
        }
        "bridge" => stop_listener(st, 1),
        "notify" => stop_listener(st, 2),
        other => Err(format!("unknown service: {other}")),
    }
}

fn set_intent(st: &Arc<AppState>, id: &str, on: bool) {
    let mut cfg = st.cfg.lock().unwrap();
    match id {
        "pageant" => cfg.services.pageant = on,
        "bridge" => cfg.services.bridge = on,
        "notify" => cfg.services.notify = on,
        _ => {}
    }
    let snapshot = cfg.clone();
    drop(cfg);
    // Persisted immediately: manual Stop/Start must survive restarts.
    if let Err(e) = crate::config::save(&snapshot) {
        eprintln!("[supervisor] cannot persist service intent: {e}");
    }
}

fn stop_listener(st: &Arc<AppState>, idx: usize) -> Result<(), String> {
    let (stop, handle) = {
        let mut svcs = st.services.lock().unwrap();
        let s = &mut svcs[idx];
        (s.stop.take(), s.handle.take())
    };
    if let Some(f) = stop {
        f.set(false);
    }
    if let Some(h) = handle {
        // Bounded wait (never join() blindly: a listener that ignored the
        // stop flag would otherwise hang this command forever).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !h.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if h.is_finished() {
            let _ = h.join();
        } else {
            eprintln!(
                "[supervisor] listener {} did not stop within 5 s — abandoning thread",
                ["pageant", "bridge", "notify"][idx]
            );
        }
    }
    let id = ["pageant", "bridge", "notify"][idx];
    st.set_service_state(idx, "stopped", None);
    set_intent(st, id, false);
    Ok(())
}

fn start_pageant(st: &Arc<AppState>) -> Result<(), String> {
    st.set_service_state(0, "starting", None);
    let st2 = st.clone();
    std::thread::spawn(move || match pageant::launch() {
        Ok(n) => st2.set_service_state(
            0,
            "running",
            Some(format!(
                "{n} ppk loaded (top-level ~/.ssh only) · SSH_AUTH_SOCK maintained"
            )),
        ),
        Err(e) => st2.set_service_state(0, "failed", Some(e)),
    });
    set_intent(st, "pageant", true);
    Ok(())
}

fn start_bridge(st: &Arc<AppState>) -> Result<(), String> {
    // Bind on the CALLER thread: port conflicts report synchronously.
    let listeners = match bridge::bind(bridge::DEFAULT_PORT) {
        Ok(l) => l,
        Err(e) => {
            st.set_service_state(1, "failed", Some(e.clone()));
            return Err(e);
        }
    };
    let stop = Arc::new(Flag::new(true));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || bridge::run(listeners, stop2));
    {
        let mut svcs = st.services.lock().unwrap();
        svcs[1].stop = Some(stop);
        svcs[1].handle = Some(handle);
    }
    st.set_service_state(1, "running", None);
    set_intent(st, "bridge", true);
    Ok(())
}

fn start_notify(st: &Arc<AppState>) -> Result<(), String> {
    let listener = match notify::bind() {
        Ok(l) => l,
        Err(e) => {
            st.set_service_state(2, "failed", Some(e.clone()));
            return Err(e);
        }
    };
    let stop = Arc::new(Flag::new(true));
    let stop2 = stop.clone();
    let handle = std::thread::spawn(move || notify::run(listener, stop2));
    {
        let mut svcs = st.services.lock().unwrap();
        svcs[2].stop = Some(stop);
        svcs[2].handle = Some(handle);
    }
    st.set_service_state(2, "running", None);
    set_intent(st, "notify", true);
    Ok(())
}

/// Bring up whatever the persisted intent asked for (fresh start = all on).
pub fn start_saved(st: &Arc<AppState>) {
    let intent = {
        let cfg = st.cfg.lock().unwrap();
        (cfg.services.pageant, cfg.services.bridge, cfg.services.notify)
    };
    for (id, on) in [("pageant", intent.0), ("bridge", intent.1), ("notify", intent.2)] {
        if on {
            if let Err(e) = start(st, id) {
                eprintln!("[supervisor] autostart {id} failed: {e}");
            }
        }
    }
}

/// Watchdog (plan: crash → auto-restart, visible as failed→starting→running):
/// every 3 s, dead listener threads flip to failed and are revived when the
/// user's intent still says running.
pub fn spawn_watchdog(st: &Arc<AppState>) {
    let st = st.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(3_000));
        // bridge / notify: detect finished worker threads
        for idx in [1usize, 2usize] {
            let finished = {
                let mut svcs = st.services.lock().unwrap();
                match &mut svcs[idx].handle {
                    Some(h) if h.is_finished() => {
                        let h = svcs[idx].handle.take();
                        drop(svcs);
                        if let Some(h) = h {
                            let _ = h.join();
                        }
                        true
                    }
                    _ => false,
                }
            };
            if finished {
                let intent = {
                    let cfg = st.cfg.lock().unwrap();
                    if idx == 1 {
                        cfg.services.bridge
                    } else {
                        cfg.services.notify
                    }
                };
                st.set_service_state(idx, "failed", Some("thread exited — restarting".into()));
                if intent {
                    let id = if idx == 1 { "bridge" } else { "notify" };
                    eprintln!("[supervisor] watchdog: restarting {id}");
                    let _ = start(&st, id);
                }
            }
        }
        // pageant: window probe (external process we do not own a handle to)
        let (state, intent) = {
            let svcs = st.services.lock().unwrap();
            let cfg = st.cfg.lock().unwrap();
            (svcs[0].info.state.clone(), cfg.services.pageant)
        };
        if intent && (state == "running" || state == "failed") && !pageant::pageant_running() {
            if state == "running" {
                eprintln!("[supervisor] watchdog: pageant vanished — restarting");
                st.set_service_state(0, "failed", Some("pageant vanished — restarting".into()));
            }
            let _ = start(&st, "pageant");
        }
    });
}
