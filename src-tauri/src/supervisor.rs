//! Service supervision: start/stop/intent persistence + watchdog.
//!
//! Index convention everywhere: 0=pageant 1=bridge 2=notify 3=hostps.
//! hostps deviates on purpose: its listener lives for the WHOLE process
//! (the access-disabled responder must never go quiet — plan r4.1), so
//! Stop/Start flip an `enabled` mode instead of bind/unbind, and the
//! watchdog revives its thread regardless of intent.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::services::{bridge, hostps, notify, pageant};
use crate::state::AppState;
use crate::util::Flag;

pub const HOSTPS_RUNNING_SUB: &str =
    "http://0.0.0.0:37022/mcp · Bearer (HKCU HOST_PS_KEY) · audit → ~/.pi-su/ps-audit.db";

pub fn hostps_stopped_sub(port: u16) -> String {
    format!(":{port} access-disabled — stopped from the UI, port stays held (agent gets DISABLED, not a connection error)")
}

pub fn start(st: &Arc<AppState>, id: &str) -> Result<(), String> {
    match id {
        "pageant" => start_pageant(st),
        "bridge" => start_bridge(st),
        "notify" => start_notify(st),
        "hostps" => start_hostps(st),
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
        "hostps" => stop_hostps(st),
        other => Err(format!("unknown service: {other}")),
    }
}

fn set_intent(st: &Arc<AppState>, id: &str, on: bool) {
    let mut cfg = st.cfg.lock().unwrap();
    match id {
        "pageant" => cfg.services.pageant = on,
        "bridge" => cfg.services.bridge = on,
        "notify" => cfg.services.notify = on,
        "hostps" => cfg.services.hostps = on,
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
    let id = ["pageant", "bridge", "notify", "hostps"][idx];
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

// ── hostps: mode switch over an always-on listener ────────────────

/// Ensure audit db + listener thread exist; idempotent. The thread is
/// re-spawned by the watchdog whenever it dies, intent-independent (the
/// disabled responder must never go silent). Returns Err only when the
/// audit db or the port bind fails right now.
pub fn hostps_ensure(st: &Arc<AppState>) -> Result<(), String> {
    let ctx = {
        let guard = st.hostps.lock().unwrap();
        match guard.as_ref() {
            Some(c) => c.clone(),
            None => {
                drop(guard);
                let (port, sse, enabled) = {
                    let cfg = st.cfg.lock().unwrap();
                    (cfg.services.hostps_port, cfg.services.hostps_sse.clone(), cfg.services.hostps)
                };
                let audit = crate::psaudit::PsAudit::open()?;
                let ctx = Arc::new(hostps::Ctx::new(audit, sse, port, enabled));
                *st.hostps.lock().unwrap() = Some(ctx.clone());
                ctx
            }
        }
    };
    let alive = {
        let svcs = st.services.lock().unwrap();
        matches!(&svcs[3].handle, Some(h) if !h.is_finished())
    };
    if !alive {
        let listeners = hostps::bind(ctx.port)?;
        let stop = Arc::new(Flag::new(true));
        let stop2 = stop.clone();
        let ctx2 = ctx.clone();
        let handle = std::thread::spawn(move || hostps::run(listeners, ctx2, stop2));
        let mut svcs = st.services.lock().unwrap();
        svcs[3].stop = Some(stop);
        svcs[3].handle = Some(handle);
    }
    let on = ctx.enabled.load(Ordering::Relaxed);
    st.set_service_state(
        3,
        if on { "running" } else { "stopped" },
        Some(if on { HOSTPS_RUNNING_SUB.into() } else { hostps_stopped_sub(ctx.port) }),
    );
    Ok(())
}

fn start_hostps(st: &Arc<AppState>) -> Result<(), String> {
    hostps_ensure(st)?;
    let ctx = st.hostps.lock().unwrap().clone().ok_or("hostps not initialized")?;
    ctx.enabled.store(true, Ordering::Relaxed);
    st.set_service_state(3, "running", Some(HOSTPS_RUNNING_SUB.into()));
    set_intent(st, "hostps", true);
    Ok(())
}

fn stop_hostps(st: &Arc<AppState>) -> Result<(), String> {
    let ctx = st.hostps.lock().unwrap().clone().ok_or("hostps not initialized")?;
    ctx.enabled.store(false, Ordering::Relaxed);
    // In-flight host commands die with the mode switch; the listener stays.
    ctx.kill_in_flight();
    st.set_service_state(3, "stopped", Some(hostps_stopped_sub(ctx.port)));
    set_intent(st, "hostps", false);
    Ok(())
}

/// Bring up whatever the persisted intent asked for (fresh start = all on).
pub fn start_saved(st: &Arc<AppState>) {
    // hostps first: listener exists regardless of intent (disabled mode
    // still answers); the intent only picks the initial mode.
    if let Err(e) = hostps_ensure(st) {
        eprintln!("[supervisor] hostps setup failed: {e}");
        st.set_service_state(3, "failed", Some(e));
    }
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
/// user's intent still says running. hostps revives REGARDLESS of intent
/// (disabled responder must stay reachable).
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
        // hostps listener thread: intent-independent revival
        let hp_finished = {
            let mut svcs = st.services.lock().unwrap();
            match &mut svcs[3].handle {
                Some(h) if h.is_finished() => {
                    let h = svcs[3].handle.take();
                    drop(svcs);
                    if let Some(h) = h {
                        let _ = h.join();
                    }
                    true
                }
                _ => false,
            }
        };
        if hp_finished {
            eprintln!("[supervisor] watchdog: hostps listener died — re-binding");
            st.set_service_state(3, "failed", Some("listener thread exited — restarting".into()));
            if let Err(e) = hostps_ensure(&st) {
                eprintln!("[supervisor] hostps restart failed: {e}");
                st.set_service_state(3, "failed", Some(e));
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
