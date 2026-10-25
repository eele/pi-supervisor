//! Tauri IPC surface — thin wrappers over the modules; all state reads are
//! snapshots (no long lock holds across IPC).

use std::sync::Arc;

use crate::sandbox::{self, SandboxInfo};
use crate::sessions::{self, SessionsSnapshot};
use crate::state::{AppState, RecreateState, ServiceInfo};
use crate::{config, supervisor};

fn snapshot_recreate(st: &AppState) -> RecreateState {
    let r = st.recreate.lock().unwrap();
    RecreateState {
        active: r.active,
        steps: r.steps.clone(),
        error: r.error.clone(),
    }
}

#[derive(serde::Serialize)]
pub struct ServiceDto {
    pub id: &'static str,
    pub name: &'static str,
    pub subtitle: String,
    pub state: String,
}

impl From<ServiceInfo> for ServiceDto {
    fn from(s: ServiceInfo) -> Self {
        Self {
            id: s.id,
            name: s.name,
            subtitle: s.subtitle,
            state: s.state,
        }
    }
}

#[tauri::command]
pub fn get_services(st: tauri::State<'_, Arc<AppState>>) -> Vec<ServiceDto> {
    st.service_infos().into_iter().map(ServiceDto::from).collect()
}

// NOTE ON async: plain `fn` commands run on the MAIN thread — a blocking
// stop/join or a 60 s sbx exec would freeze the whole UI (observed). The
// potentially-slow commands below are async so they run on the async pool.
#[tauri::command]
pub async fn start_service(st: tauri::State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    supervisor::start(&st, &id)
}

#[tauri::command]
pub async fn stop_service(st: tauri::State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    supervisor::stop(&st, &id)
}

#[tauri::command]
pub fn get_sandbox(st: tauri::State<'_, Arc<AppState>>) -> SandboxInfo {
    st.sandbox.lock().unwrap().clone()
}

#[tauri::command]
pub fn recreate_sandbox(st: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    sandbox::start_recreate(&st)
}

#[tauri::command]
pub fn get_recreate_state(st: tauri::State<'_, Arc<AppState>>) -> RecreateState {
    snapshot_recreate(&st)
}

#[tauri::command]
pub async fn push_tokens(st: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    let name = st.cfg.lock().unwrap().sandbox_name.clone();
    if matches!(
        st.sandbox.lock().unwrap().state,
        sandbox::SandboxState::NotCreated
    ) {
        return Err(format!("sandbox {name} is not created"));
    }
    crate::tokens::push_all(&name)
}

#[tauri::command]
pub fn get_sessions(st: tauri::State<'_, Arc<AppState>>) -> SessionsSnapshot {
    st.sessions.lock().unwrap().clone()
}

#[tauri::command]
pub async fn attach_session(name: String) -> Result<(), String> {
    sessions::attach(&name)
}

#[tauri::command]
pub fn get_autostart() -> bool {
    config::autostart_enabled()
}

#[tauri::command]
pub async fn set_autostart(on: bool) -> Result<(), String> {
    config::autostart_set(on)
}

#[tauri::command]
pub async fn disable_legacy_startup() -> Vec<String> {
    config::disable_legacy_startup()
}
