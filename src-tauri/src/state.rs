//! Shared application state (managed by Tauri as an Arc, cheap to clone
//! into worker threads).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use crate::config::AppConfig;
use crate::sandbox::SandboxInfo;
use crate::sessions::SessionsSnapshot;
use crate::util::Flag;

#[derive(Clone, serde::Serialize)]
pub struct ServiceInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub subtitle: String,
    /// "running" | "starting" | "stopped" | "failed" (wire values).
    pub state: String,
}

pub struct ServiceRuntime {
    pub info: ServiceInfo,
    /// Stop flag for in-process listeners (bridge/notify).
    pub stop: Option<Arc<Flag>>,
    /// Worker thread; `is_finished()` drives the watchdog.
    pub handle: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone, serde::Serialize)]
pub struct RecreateStep {
    pub label: String,
    pub status: String, // pending | active | done | error
}

#[derive(serde::Serialize)]
pub struct RecreateState {
    pub active: bool,
    pub steps: Vec<RecreateStep>,
    pub error: Option<String>,
}

pub struct AppState {
    pub cfg: Mutex<AppConfig>,
    /// Fixed order: [pageant, bridge, notify].
    pub services: Mutex<Vec<ServiceRuntime>>,
    pub sandbox: Mutex<SandboxInfo>,
    pub sessions: Mutex<SessionsSnapshot>,
    pub recreate: Mutex<RecreateState>,
    pub recreate_busy: AtomicBool,
}

impl AppState {
    pub fn new(cfg: AppConfig) -> Arc<Self> {
        let mk = |id, name, subtitle: &str| ServiceRuntime {
            info: ServiceInfo {
                id,
                name,
                subtitle: subtitle.to_string(),
                state: "stopped".into(),
            },
            stop: None,
            handle: None,
        };
        let kit_repo = cfg.zot_repo.clone();
        Arc::new(Self {
            cfg: Mutex::new(cfg),
            services: Mutex::new(vec![
                mk("pageant", "Pageant SSH Agent", "no ~/.ssh ppk loaded yet"),
                mk(
                    "bridge",
                    "Agent TCP Bridge",
                    "tcp://0.0.0.0:60022 · token-authenticated · Pageant WM_COPYDATA",
                ),
                mk(
                    "notify",
                    "Notify Hub",
                    "http://0.0.0.0:60000/notify · Windows toast (reminder)",
                ),
            ]),
            sandbox: Mutex::new(SandboxInfo {
                // Unknown until the first probe lands; start conservative.
                state: crate::sandbox::SandboxState::NotCreated,
                kit: String::new(),
                mount_path: String::new(),
                kit_repo,
            }),
            sessions: Mutex::new(SessionsSnapshot::default()),
            recreate: Mutex::new(RecreateState {
                active: false,
                steps: Vec::new(),
                error: None,
            }),
            recreate_busy: AtomicBool::new(false),
        })
    }

    pub fn service_infos(&self) -> Vec<ServiceInfo> {
        self.services
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.info.clone())
            .collect()
    }

    pub fn set_service_state(&self, idx: usize, state: &str, subtitle: Option<String>) {
        if let Some(s) = self.services.lock().unwrap().get_mut(idx) {
            s.info.state = state.to_string();
            if let Some(sub) = subtitle {
                s.info.subtitle = sub;
            }
        }
    }
}
