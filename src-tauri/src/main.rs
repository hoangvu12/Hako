// Prevent an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod events;
mod media;
mod settings;

// Performance-critical capture/encode pipeline.
mod core;
// Riot/Valorant integration.
mod valorant;
// Clip library + thumbnails.
mod library;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, WindowEvent,
};

fn main() {
    let _log_guard = init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Range-aware clip streaming (smooth playback + seeking in the editor).
        .register_uri_scheme_protocol(media::SCHEME, media::handle)
        .manage(commands::CaptureState::default())
        .invoke_handler(tauri::generate_handler![
            commands::recorder_status,
            commands::gpu_info,
            commands::ffmpeg_info,
            commands::list_windows,
            commands::start_capture,
            commands::stop_capture,
            commands::capture_status,
            commands::save_clip,
            commands::clips_list,
            commands::delete_clip,
            commands::rename_clip,
            commands::trim_clip,
            commands::get_settings,
            commands::update_settings,
            commands::valorant_status
        ])
        .setup(|app| {
            app.manage(init_library(app.handle()));
            app.manage(init_settings(app.handle()));
            // Point the graphics-hook loader at the bundled OBS binaries
            // (`<resource_dir>/vendor/obs-hook`) so packaged builds find them.
            // In dev this dir doesn't exist under the resource root; the hook
            // loader then falls back to the in-repo path.
            if let Ok(res) = app.path().resource_dir() {
                let hook_dir = res.join("vendor").join("obs-hook");
                if hook_dir.is_dir() {
                    core::hook::host::set_vendor_hook_dir(hook_dir);
                }
            }
            build_tray(app.handle())?;
            register_clip_hotkey(app.handle());
            // Live Valorant detection: poll presence, record full matches, and
            // auto-cut highlight clips on match end (Mode B). Degrades to manual
            // clips if Riot/capture aren't available.
            valorant::orchestrator::spawn(app.handle().clone());
            tracing::info!("Hako core started");
            Ok(())
        })
        // Window close → hide to tray; the recorder threads keep running.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Hako");
}

/// System tray with show / hide / quit. Built in code so it shares the
/// window icon and we own the menu-event handlers.
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Hako", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide to tray", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Hako", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    TrayIconBuilder::with_id("hako-tray")
        .tooltip("Hako — Valorant clip recorder")
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            // Quit fully stops the recorder (separate from hide-to-tray).
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Initialize logging: stdout (dev) + a rolling daily file under
/// `%LOCALAPPDATA%\hako\logs\hako.log` (crash logging). The returned guard
/// flushes the non-blocking file writer on drop, so `main` must keep it alive.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "hako=info,tauri=info".into());
    let stdout_layer = tracing_subscriber::fmt::layer();

    let log_dir = std::env::var_os("LOCALAPPDATA")
        .map(|p| std::path::PathBuf::from(p).join("hako").join("logs"));
    match log_dir {
        Some(dir) if std::fs::create_dir_all(&dir).is_ok() => {
            let (writer, guard) =
                tracing_appender::non_blocking(tracing_appender::rolling::daily(&dir, "hako.log"));
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer);
            tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .with(file_layer)
                .init();
            Some(guard)
        }
        _ => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stdout_layer)
                .init();
            None
        }
    }
}

/// Open the clip library at `<AppData>/library.db`, falling back to an
/// in-memory DB if the on-disk open fails (so the app still runs).
fn init_library(app: &tauri::AppHandle) -> commands::LibraryState {
    use crate::library::db::Library;
    let on_disk = app.path().app_data_dir().ok().and_then(|dir| {
        let _ = std::fs::create_dir_all(&dir);
        Library::open(&dir.join("library.db"))
            .map_err(|e| tracing::error!("open library db: {e}"))
            .ok()
    });
    let lib = on_disk.unwrap_or_else(|| {
        tracing::warn!("clip library falling back to in-memory (won't persist)");
        Library::open_in_memory().expect("in-memory library")
    });
    commands::LibraryState(std::sync::Mutex::new(lib))
}

/// Load persisted settings (or defaults) from the app config dir.
fn init_settings(app: &tauri::AppHandle) -> commands::SettingsState {
    use crate::settings::Settings;
    let settings = app
        .path()
        .app_config_dir()
        .ok()
        .map(|dir| Settings::load(&Settings::file_in(&dir)))
        .unwrap_or_default();
    commands::SettingsState(std::sync::Mutex::new(settings))
}

/// Register the global "save last 30s" hotkey (**F9**). Registration failure is
/// logged, not fatal (e.g. another app already owns the key). The save runs on
/// its own thread so the shortcut dispatcher is never blocked by mux IO.
fn register_clip_hotkey(app: &tauri::AppHandle) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let handle = app.clone();
    let res = app
        .global_shortcut()
        .on_shortcut("F9", move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return; // fire once on press, ignore the release
            }
            let handle = handle.clone();
            // save_clip_full emits `clip-created` itself; we just log/surface errors.
            std::thread::spawn(move || match commands::save_clip_full(&handle, 30, None) {
                Ok(rec) => tracing::info!("hotkey saved clip → {}", rec.path),
                Err(e) => {
                    tracing::warn!("clip save failed: {e}");
                    let _ = handle.emit(events::RECORDER_ERROR, e);
                }
            });
        });
    if let Err(e) = res {
        tracing::error!("could not register clip hotkey (F9): {e}");
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

