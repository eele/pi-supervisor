//! D1 — Pageant SSH agent supervision.
//!
//! Port of pageant-bootstrap.ps1: launch Pageant with EVERY top-level
//! `~/.ssh/*.ppk` (subdirectories deliberately excluded), then point the
//! user-scope SSH_AUTH_SOCK at Pageant's named pipe and broadcast the change.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::util;

const PAGEANT_FALLBACK: &str = r"C:\Program Files\PuTTY\pageant.exe";

fn pageant_exe() -> String {
    std::env::var("PAGEANT_PATH").unwrap_or_else(|_| PAGEANT_FALLBACK.to_string())
}

fn ssh_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".ssh"))
}

/// All `*.ppk` in the TOP LEVEL of ~/.ssh only — recursive subdirectories
/// (bak/, bak2/, …) are excluded by design.
pub fn top_level_ppks() -> Vec<String> {
    let Some(dir) = ssh_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let is_ppk = p.is_file()
                && p.extension().map(|x| x.eq_ignore_ascii_case("ppk")).unwrap_or(false);
            is_ppk.then(|| p.display().to_string())
        })
        .collect();
    keys.sort();
    keys
}

/// Pageant's window class/name — its documented IPC endpoint for
/// WM_COPYDATA agent requests (see also services/bridge.rs).
pub fn pageant_running() -> bool {
    find_pageant_hwnd().is_some()
}

#[cfg(target_os = "windows")]
pub fn find_pageant_hwnd() -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    unsafe { FindWindowW(w!("Pageant"), w!("Pageant")).ok() }.filter(|h| !h.is_invalid())
}

#[cfg(not(target_os = "windows"))]
pub fn find_pageant_hwnd() -> Option<isize> {
    None
}

/// Start Pageant with all top-level ppk files, refresh SSH_AUTH_SOCK.
/// Starting when it already runs is fine (Pageant merges keys into the
/// existing instance).
pub fn launch() -> Result<usize, String> {
    let keys = top_level_ppks();
    let mut cmd = std::process::Command::new(pageant_exe());
    cmd.args(&keys);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn()
        .map_err(|e| format!("cannot start pageant: {e}"))?;

    // Pageant creates its agent pipe slightly after process start.
    std::thread::sleep(Duration::from_millis(2_000));
    update_ssh_auth_sock();
    Ok(keys.len())
}

/// Stop: kill every pageant.exe (the ps1 bootstrap had no other stop path).
pub fn kill() -> Result<(), String> {
    let _ = util::run_cmd("taskkill", &["/IM".into(), "pageant.exe".into(), "/F".into()], None, 8_000);
    Ok(())
}

/// Locate Pageant's `pageant.*` named pipe, publish it as the USER-scope
/// SSH_AUTH_SOCK and broadcast WM_SETTINGCHANGE so new shells pick it up.
pub fn update_ssh_auth_sock() {
    let Some(pipe) = find_pageant_pipe() else {
        return;
    };
    let val = format!(r"\\.\pipe\{pipe}");
    let _ = util::run_cmd(
        "reg",
        &[
            "add".into(),
            "HKCU\\Environment".into(),
            "/v".into(),
            "SSH_AUTH_SOCK".into(),
            "/t".into(),
            "REG_SZ".into(),
            "/d".into(),
            val,
            "/f".into(),
        ],
        None,
        5_000,
    );
    broadcast_env_change();
}

fn find_pageant_pipe() -> Option<String> {
    // \\.\pipe\ is a virtual directory listing every named pipe.
    let entries = fs::read_dir(r"\\.\pipe\").ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains("pageant.") {
            return Some(name);
        }
    }
    None
}

/// SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, "Environment")
/// — the standard refresh signal [Environment]::SetEnvironmentVariable(User)
/// performs internally.
#[cfg(target_os = "windows")]
fn broadcast_env_change() {
    use windows::core::w;
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            None,
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            1_000,
            None,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn broadcast_env_change() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppk_list_is_flat_and_sorted() {
        let keys = top_level_ppks();
        for k in &keys {
            // No nested directory separators beyond the .ssh root itself.
            let tail = k.rsplit(".ssh").next().unwrap_or("");
            assert!(!tail[1..].contains(['/', '\\']), "nested ppk leaked: {k}");
            assert!(k.to_lowercase().ends_with(".ppk"));
        }
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }
}
