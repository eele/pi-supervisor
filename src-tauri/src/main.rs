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

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};

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
    let st = AppState::new(config::load());

    tauri::Builder::default()
        .manage(st.clone())
        .invoke_handler(tauri::generate_handler![
            commands::get_services,
            commands::start_service,
            commands::stop_service,
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

            // ── Tray: Open / Exit ──────────────────────────────────────
            let open_item = MenuItem::with_id(app, "tray-open", "Open", true, None::<&str>)?;
            let exit_item = MenuItem::with_id(app, "tray-exit", "Exit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &exit_item])?;
            let mut tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("Pi Supervisor")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "tray-open" => restore_main_window(app),
                    "tray-exit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                    | TrayIconEvent::DoubleClick { .. } => {
                        restore_main_window(tray.app_handle());
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
