//! config.json (next to the exe; project root during dev) plus the
//! autostart state — which deliberately lives ONLY in the registry.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

use crate::util;

pub const RUN_KEY: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
pub const RUN_VALUE: &str = "PiSupervisor";

/// Persisted window geometry (physical px) restored at startup.
#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub struct WindowState {
    pub width: u32,
    pub height: u32,
    pub x: i32,
    pub y: i32,
    pub maximized: bool,
}

/// Which services the user wants running at next launch. Manual Stop
/// survives restarts; this is the single source of that intent.
#[derive(Serialize, Deserialize, Clone)]
pub struct ServiceIntent {
    #[serde(default = "on")]
    pub pageant: bool,
    #[serde(default = "on")]
    pub bridge: bool,
    #[serde(default = "on")]
    pub notify: bool,
}

fn on() -> bool {
    true
}

impl Default for ServiceIntent {
    fn default() -> Self {
        Self {
            pageant: true,
            bridge: true,
            notify: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "snake_case")]
pub struct AppConfig {
    /// Sandbox name is FIXED (plan: no rename feature); kept in config only
    /// as an escape hatch for exotic setups, read-only for the UI.
    pub sandbox_name: String,
    /// Host workspace bind-mounted by `sbx run -d` (shown as "Mount path").
    pub mount_path: String,
    /// Last-resort kit tag when zot's tags/list is unreachable ("" = none).
    pub kit_fallback_tag: String,
    pub services: ServiceIntent,
    pub window: Option<WindowState>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            sandbox_name: "pi-ai".into(),
            mount_path: "C:\\Users\\ele\\Tea".into(),
            kit_fallback_tag: String::new(),
            services: ServiceIntent::default(),
            window: None,
        }
    }
}

fn config_path() -> PathBuf {
    let base = if cfg!(debug_assertions) {
        // Dev mode: project root (parent of src-tauri)
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        manifest_dir
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
    } else {
        // Release: directory containing the running executable
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir)
    };
    base.join("config.json")
}

pub fn load() -> AppConfig {
    let path = config_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&data) {
            return cfg;
        }
        eprintln!("[pi-supervisor] config.json corrupt, falling back to defaults");
    }
    AppConfig::default()
}

/// Atomic save: temp file + rename, mirroring the proven dns-switch pattern.
pub fn save(cfg: &AppConfig) -> Result<(), String> {
    let path = config_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Cannot create config dir: {e}"))?;
    }
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &data).map_err(|e| format!("Cannot write config temp file: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("Cannot finalize config file: {e}"))?;
    Ok(())
}

/// Autostart state is the registry itself — never mirrored into config.json,
/// so a manual `reg delete` is always reflected truthfully.
pub fn autostart_enabled() -> bool {
    util::run_cmd(
        "reg",
        &["query".into(), RUN_KEY.into(), "/v".into(), RUN_VALUE.into()],
        None,
        5_000,
    )
    .is_ok()
}

pub fn autostart_set(on: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if on {
        util::run_cmd(
            "reg",
            &[
                "add".into(),
                RUN_KEY.into(),
                "/v".into(),
                RUN_VALUE.into(),
                "/t".into(),
                "REG_SZ".into(),
                "/d".into(),
                format!("\"{}\"", exe.display()),
                "/f".into(),
            ],
            None,
            5_000,
        )?;
    } else {
        // Deleting an absent value errors — tolerate; the desired end state
        // (no Run entry) is what matters.
        let _ = util::run_cmd(
            "reg",
            &["delete".into(), RUN_KEY.into(), "/v".into(), RUN_VALUE.into(), "/f".into()],
            None,
            5_000,
        );
    }
    Ok(())
}

/// Legacy Startup-folder takeover (plan §9): rename pi-related entries to
/// `<name>.disabled` — reversible, nothing is deleted.
pub fn disable_legacy_startup() -> Vec<String> {
    const TARGETS: [&str; 2] = ["pageant-bootstrap.cmd", "server.vbs"];
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Vec::new();
    };
    let dir = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup");
    let mut renamed = Vec::new();
    for name in TARGETS {
        let from = dir.join(name);
        if from.exists() {
            let to = dir.join(format!("{name}.disabled"));
            if fs::rename(&from, &to).is_ok() {
                renamed.push(format!("{name}.disabled"));
            }
        }
    }
    renamed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cfg_round_trips_through_json() {
        let cfg = AppConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sandbox_name, "pi-ai");
        assert!(back.services.notify);
    }

    #[test]
    fn partial_json_gets_defaults() {
        let cfg: AppConfig = serde_json::from_str(r#"{"mount_path":"D:\\x"}"#).unwrap();
        assert_eq!(cfg.mount_path, "D:\\x");
        assert_eq!(cfg.sandbox_name, "pi-ai");
        assert!(cfg.services.bridge);
    }
}
