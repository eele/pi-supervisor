// Prevents an extra console window on Windows in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod sandbox;
mod services;
mod sessions;
mod state;
mod supervisor;
mod tokens;
mod util;
mod zot;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use config::WindowState;
use state::AppState;

/// On Windows 11, the DWM automatically rounds window corners. Disable so
/// CSS border-radius is the single source of truth (same as dns-switch).
#[cfg(target_os = "windows")]
fn disable_dwm_rounded_corners(hwnd: isize) {
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *const std::ffi::c_void,
            dw_attribute: u32,
            pv_attribute: *const u32,
            cb_attribute: u32,
        ) -> i32;
    }
    unsafe {
        let dont_round: u32 = 1; // DWMWCP_DONOTROUND
        let _ = DwmSetWindowAttribute(
            hwnd as *const std::ffi::c_void,
            33, // DWMWA_WINDOW_CORNER_PREFERENCE
            &dont_round,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

fn restore_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// ── Tray right-click menu (custom Aqua window, mockup 3) ─────────
// A native Win32 tray menu cannot carry the Aqua skin (rounded #F6F6F6
// card, solid-blue hovered item), so right-click opens a frameless
// transparent window styled by TrayMenu.svelte. It hides on blur,
// exactly like a real menu.

/// Logical size incl. the 10px transparent shadow margin.
const TRAY_MENU_W: f64 = 170.0;
const TRAY_MENU_H: f64 = 104.0;

fn ensure_tray_menu(app: &AppHandle) -> Option<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window("tray-menu") {
        return Some(w);
    }
    let w = WebviewWindowBuilder::new(app, "tray-menu", WebviewUrl::App("index.html#tray-menu".into()))
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .visible(false)
        .inner_size(TRAY_MENU_W, TRAY_MENU_H)
        .build()
        .ok()?;
    // Click anywhere outside → dismiss (blur), like a popup menu.
    let w2 = w.clone();
    w.on_window_event(move |e| {
        if let WindowEvent::Focused(false) = e {
            let _ = w2.hide();
        }
    });
    Some(w)
}

fn show_tray_menu_at(app: &AppHandle, x: i32, y: i32) {
    // First-time creation must NOT run on the main thread: build() waits
    // on the event loop that "belongs" to the main thread — calling it
    // from a sync command (main thread) deadlocks the whole app.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(win) = ensure_tray_menu(&app) {
            let _ = win.set_position(PhysicalPosition::new(x, y));
            let _ = win.show();
            let _ = win.set_focus();
        }
    });
}

/// E2E/test entry point: show the tray menu at a physical position.
#[tauri::command]
fn show_tray_menu(app: AppHandle, x: i32, y: i32) {
    show_tray_menu_at(&app, x, y);
}

/// OS-level truth for E2E asserts (visibilityState can lag SW_HIDE):
/// is the given webview window currently shown?
#[tauri::command]
fn webview_visible(app: AppHandle, label: String) -> bool {
    app.get_webview_window(&label)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// TrayMenu.svelte buttons call this.
#[tauri::command]
fn tray_menu_action(app: AppHandle, action: String) {
    match action.as_str() {
        "open" => {
            if let Some(m) = app.get_webview_window("tray-menu") {
                let _ = m.hide();
            }
            restore_main_window(&app);
        }
        "exit" => app.exit(0),
        _ => {}
    }
}

static MAXIMIZED: AtomicBool = AtomicBool::new(false);
static SAVE_GEN: AtomicU64 = AtomicU64::new(0);

/// Debounced window-state persistence: every resize/move bumps the
/// generation; only the last writer (600 ms of quiet) hits the disk.
fn schedule_window_save(app: AppHandle) {
    let gen = SAVE_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        if SAVE_GEN.load(Ordering::SeqCst) != gen {
            return; // a newer event superseded us
        }
        let Some(st) = app.try_state::<Arc<AppState>>() else {
            return;
        };
        let Some(win) = app.get_webview_window("main") else {
            return;
        };
        let Ok(size) = win.inner_size() else { return };
        let Ok(pos) = win.outer_position() else { return };
        let maximized = win.is_maximized().unwrap_or(false);
        let mut cfg = st.cfg.lock().unwrap();
        cfg.window = Some(WindowState {
            width: size.width,
            height: size.height,
            x: pos.x,
            y: pos.y,
            maximized,
        });
        let snapshot = cfg.clone();
        drop(cfg);
        if let Err(e) = config::save(&snapshot) {
            eprintln!("[pi-supervisor] window state save failed: {e}");
        }
    });
}

/// Restore persisted geometry; off-screen positions (monitor vanished)
/// fall back to default size, centered.
fn restore_window_state(app: &AppHandle) {
    let Some(st) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    let Some(ws) = st.cfg.lock().unwrap().window else {
        return;
    };
    let Some(win) = app.get_webview_window("main") else {
        return;
    };
    // On-screen sanity: keep if the position lands on any monitor's area.
    let sane = ws.x > -2_000 && ws.y > -2_000 && ws.width >= 560 && ws.height >= 640;
    if sane {
        let _ = win.set_size(tauri::PhysicalSize::new(ws.width, ws.height));
        let _ = win.set_position(tauri::PhysicalPosition::new(ws.x, ws.y));
        if ws.maximized {
            let _ = win.maximize();
        }
    }
}

fn main() {
    let (cfg, config_corrupt) = config::load_ex();
    let st = AppState::new(cfg, config_corrupt);
    // sbx children must bypass the proxy for the configured registry host.
    util::configure_no_proxy(&st.cfg.lock().unwrap().zot_repo);

    tauri::Builder::default()
        .manage(st.clone())
        // Single-instance: a second exe exits immediately and the running
        // window is restored instead (mockup contract: one supervisor).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            restore_main_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            show_tray_menu,
            tray_menu_action,
            webview_visible,
            commands::get_services,
            commands::start_service,
            commands::stop_service,
            commands::get_config_corrupt,
            commands::get_sandbox,
            commands::recreate_sandbox,
            commands::get_recreate_state,
            commands::push_tokens,
            commands::get_sessions,
            commands::attach_session,
            commands::get_autostart,
            commands::set_autostart,
            commands::disable_legacy_startup
        ])
        // Red traffic-light / OS close → hide to tray; only tray Exit quits.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(move |app| {
            #[cfg(target_os = "windows")]
            {
                use raw_window_handle::HasWindowHandle;
                if let Some(window) = app.get_webview_window("main") {
                    if let Ok(handle) = window.window_handle() {
                        if let raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() {
                            disable_dwm_rounded_corners(h.hwnd.get());
                        }
                    }
                }
            }

            // ── Tray: left/double restores, right-click = custom menu ──
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("Pi Supervisor")
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                    | TrayIconEvent::DoubleClick { .. } => {
                        restore_main_window(tray.app_handle());
                    }
                    TrayIconEvent::Click {
                        button: MouseButton::Right,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } => {
                        // Mockup: menu pops above the icon, right-aligned.
                        // (No ensure_tray_menu() HERE: window creation must
                        // stay off the main thread — see show_tray_menu_at.)
                        let app = tray.app_handle().clone();
                        let icon = match (&rect.position, &rect.size) {
                            (
                                tauri::Position::Physical(p),
                                tauri::Size::Physical(s),
                            ) => Some((p.x, p.y, s.width as i32)),
                            (
                                tauri::Position::Logical(l),
                                tauri::Size::Logical(s),
                            ) => Some((l.x as i32, l.y as i32, s.width as i32)),
                            _ => None,
                        };
                        if let Some((ix, iy, iw)) = icon {
                            let sf = app
                                .primary_monitor()
                                .ok()
                                .flatten()
                                .map(|m| m.scale_factor())
                                .unwrap_or(1.0);
                            let mw = (TRAY_MENU_W * sf) as i32;
                            let mh = (TRAY_MENU_H * sf) as i32;
                            show_tray_menu_at(
                                &app,
                                ix + iw - mw,
                                iy - mh - (4.0 * sf) as i32,
                            );
                        }
                    }
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.build(app)?;

            // ── Background machinery ───────────────────────────────────
            supervisor::start_saved(&st);
            supervisor::spawn_watchdog(&st);
            sandbox::spawn_prober(&st);
            sessions::spawn_poller(&st);

            restore_window_state(&app.handle());

            // Maximize state → CSS margin toggle on the frontend.
            {
                let apph = app.handle().clone();
                let _ = apph.emit("pi-maximized", MAXIMIZED.load(Ordering::SeqCst));
                if let Some(win) = app.get_webview_window("main") {
                    let h2 = apph.clone();
                    win.on_window_event(move |event| match event {
                        WindowEvent::Resized(_) => {
                            if let Some(w) = h2.get_webview_window("main") {
                                let now = w.is_maximized().unwrap_or(false);
                                if now != MAXIMIZED.swap(now, Ordering::SeqCst) {
                                    let _ = h2.emit("pi-maximized", now);
                                }
                            }
                            schedule_window_save(h2.clone());
                        }
                        WindowEvent::Moved(_) => {
                            schedule_window_save(h2.clone());
                        }
                        _ => {}
                    });
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                // Reap sbx/taskkill children so nothing outlives the UI.
                util::kill_all_children();
            }
        });
}
