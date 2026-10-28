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
    /// hostps running-mode intent. NOTE: even when false the MCP listener
    /// stays up (access-disabled responder) — this intent gates command
    /// EXECUTION, never the port (plan r4.1).
    #[serde(default = "on")]
    pub hostps: bool,
    /// "auto" | "force" | "off" — SSE output-stream negotiation mode.
    #[serde(default = "default_sse")]
    pub hostps_sse: String,
    #[serde(default = "default_hostps_port")]
    pub hostps_port: u16,
}

fn on() -> bool {
    true
}
fn default_sse() -> String {
    "auto".into()
}
fn default_hostps_port() -> u16 {
    37022
}

/// string | [string] — old configs keep working untouched.
fn de_mount_paths<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(d)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

impl Default for ServiceIntent {
    fn default() -> Self {
        Self {
            pageant: true,
            bridge: true,
            notify: true,
            hostps: true,
            hostps_sse: default_sse(),
            hostps_port: default_hostps_port(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default, rename_all = "snake_case")]
pub struct AppConfig {
    /// Sandbox name is FIXED (plan: no rename feature); kept in config only
    /// as an escape hatch for exotic setups, read-only for the UI.
    pub sandbox_name: String,
    /// Host workspace(s) bind-mounted by `sbx run -d` (shown as "Mount
    /// path"). sbx takes PATH positionally — several entries mean several
    /// bind mounts and the FIRST one is the workspace. Accepts a bare
    /// string or an array; the legacy singular key `mount_path` is still
    /// read (entries may carry a `:ro` suffix per sbx).
    #[serde(default, alias = "mount_path", deserialize_with = "de_mount_paths")]
    pub mount_paths: Vec<String>,
    /// Kit image repo on the zot registry, host + path WITHOUT tag
    /// ("zot.byte.art/blackbox/pi-kit"). Single source of truth: drives the
    /// tags/list URL, the `sbx run` reference, the sbx NO_PROXY entry, and
    /// the Recreate dialog copy.
    pub zot_repo: String,
    /// Last-resort kit tag when zot's tags/list is unreachable ("" = none).
    pub kit_fallback_tag: String,
    pub services: ServiceIntent,
    pub window: Option<WindowState>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            sandbox_name: "pi-ai".into(),
            mount_paths: vec!["C:\\Users\\ele\\Tea".into()],
            zot_repo: "zot.byte.art/blackbox/pi-kit".into(),
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
    load_ex().0
}

/// load() + the "file existed but was unusable" flag. Silent defaults were
/// a real footgun (bad escape in a hand-edited file → wrong mounts, no
/// trace in the release UI), so the corruption is surfaced to the statusbar.
pub fn load_ex() -> (AppConfig, bool) {
    let path = config_path();
    match fs::read_to_string(&path) {
        Err(_) => return (AppConfig::default(), false), // absent file: fresh install
        Ok(data) => match serde_json::from_str::<AppConfig>(&data) {
            Ok(cfg) => (cfg, false),
            Err(e) => {
                eprintln!("[pi-supervisor] config.json corrupt ({e}), falling back to defaults");
                (AppConfig::default(), true)
            }
        },
    }
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
        assert_eq!(cfg.mount_paths, vec!["D:\\x"]);
        assert_eq!(cfg.sandbox_name, "pi-ai");
        assert_eq!(cfg.zot_repo, "zot.byte.art/blackbox/pi-kit");
        assert!(cfg.services.bridge);
    }

    #[test]
    fn mount_paths_accepts_legacy_string_and_arrays() {
        // legacy singular key, bare string
        let cfg: AppConfig = serde_json::from_str(r#"{"mount_path":"D:\\x"}"#).unwrap();
        assert_eq!(cfg.mount_paths, vec!["D:\\x"]);
        // new plural key, array (extra mount + read-only suffix pass-through)
        let cfg: AppConfig =
            serde_json::from_str(r#"{"mount_paths":["C:\\ws","D:\\refs:ro"]}"#).unwrap();
        assert_eq!(cfg.mount_paths, vec!["C:\\ws", "D:\\refs:ro"]);
        // saving migrates transparently: the round trip lands on plural key
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"mount_paths\""));
        assert!(!json.contains("\"mount_path\""));
    }

    #[test]
    fn zot_repo_override_round_trips() {
        let cfg: AppConfig =
            serde_json::from_str(r#"{"zot_repo":"registry.internal:5000/kit/pi"}"#).unwrap();
        assert_eq!(cfg.zot_repo, "registry.internal:5000/kit/pi");
    }

    #[test]
    fn full_real_world_config_parses() {
        // Byte-shape of an actual on-disk file (incident: a heredoc ate the
        // JSON double-escapes; the resulting \U "invalid escape" must be
        // REJECTED loudly, not half-parsed).
        let cfg: AppConfig = serde_json::from_str(
            r#"{
              "sandbox_name": "pi-ai",
              "mount_paths": ["C:\\Users\\ele\\Tea", "C:\\Users\\ele\\Desktop:ro"],
              "zot_repo": "zot.byte.art/blackbox/pi-kit",
              "kit_fallback_tag": "",
              "services": { "pageant": true, "bridge": true, "notify": true },
              "window": null
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.mount_paths.len(), 2, "got {:?}", cfg.mount_paths);
    }
}
