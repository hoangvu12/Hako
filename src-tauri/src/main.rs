// Prevent an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Fast multithreaded allocator for the whole process (see Cargo.toml note). The
// capture/encode/mux/audio paths allocate frequently across threads; mimalloc
// beats the default Windows heap on that pattern.
//
// NOTE: mimalloc was briefly suspected of the STATUS_HEAP_CORRUPTION (0xc0000374)
// crash and bisected out — it was innocent. The real cause was a VT_BLOB
// PROPVARIANT freeing a stack pointer in the process-loopback audio path (see
// `core::audio` process_loopback::open). The Cargo.toml `libmimalloc-sys` v2 pin
// is therefore no longer required for correctness, only kept as a known-good pin.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
        // Restore the main window's size/position/maximized state on launch.
        // Only those flags — not VISIBLE (we control reveal via the update
        // splash) and not DECORATIONS (the app is intentionally frameless). The
        // `updater` splash is denylisted so it always opens centered at its
        // fixed size.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .with_denylist(&["updater"])
                .build(),
        )
        // Auto-update: the splash window checks GitHub Releases and installs a
        // signed update via these plugins; `process` provides the relaunch.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Range-aware clip streaming (smooth playback + seeking in the editor).
        .register_uri_scheme_protocol(media::SCHEME, media::handle)
        .manage(commands::CaptureState::default())
        // Shared live-match context (map/mode/agent) for tagging manual F9 clips;
        // kept current by the Valorant orchestrator.
        .manage(valorant::live::LiveMatchState::default())
        .invoke_handler(tauri::generate_handler![
            commands::recorder_status,
            commands::gpu_info,
            commands::ffmpeg_info,
            commands::list_windows,
            commands::list_audio_inputs,
            commands::list_audio_outputs,
            commands::list_active_audio_sessions,
            commands::process_loopback_supported,
            commands::start_capture,
            commands::stop_capture,
            commands::capture_status,
            commands::save_clip,
            commands::clips_list,
            commands::delete_clip,
            commands::rename_clip,
            commands::trim_clip,
            commands::clip_audio_tracks,
            commands::read_clip_range,
            commands::remux_with_tracks,
            commands::get_settings,
            commands::update_settings,
            commands::valorant_status,
            finish_to_main
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
            // Reap per-game hook DLL copies left by games from prior sessions
            // (the ones still running keep their copy locked and are skipped).
            core::hook::host::cleanup_stale_hook_dll_copies();
            build_tray(app.handle())?;
            // Register the save-clip global shortcut from the saved hotkey (not a
            // hardcoded key). Editing it in Settings / the titlebar re-registers
            // live via `update_settings` → `set_clip_hotkey`.
            let accel = app
                .state::<commands::SettingsState>()
                .0
                .lock()
                .map(|s| s.save_hotkey.clone())
                .unwrap_or_else(|_| "F9".into());
            set_clip_hotkey(app.handle(), None, &accel);
            // Live Valorant detection: poll presence, record full matches, and
            // auto-cut highlight clips on match end (Mode B). Degrades to manual
            // clips if Riot/capture aren't available.
            valorant::orchestrator::spawn(app.handle().clone());
            // Safety net for the update splash: if the `updater` window never
            // calls `finish_to_main` (e.g. its webview failed to load entirely),
            // reveal the main window anyway after a generous delay so the app can
            // never be stranded behind the splash. We only *show* main (never
            // close `updater`) — closing it could abort an in-flight download,
            // and a legitimately slow download keeps main hidden until relaunch.
            let reveal_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(60));
                if let Some(main) = reveal_handle.get_webview_window("main") {
                    if !main.is_visible().unwrap_or(true) {
                        tracing::warn!("update splash never finished; revealing main window");
                        let _ = main.show();
                        let _ = main.set_focus();
                    }
                }
            });
            tracing::info!("Hako core started");
            Ok(())
        })
        // Window close → hide to tray; the recorder threads keep running. While
        // hidden during a match the UI doesn't need to render, so drop WebView2 to
        // a low memory target to free its renderer RAM (restored on show).
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Only the main window hides to tray; the transient updater
                // splash must close normally so `finish_to_main` can dismiss it.
                if window.label() != "main" {
                    return;
                }
                api.prevent_close();
                // Persist geometry now so even a later hard kill (while hidden to
                // tray) still restores the last size/position on next launch.
                use tauri_plugin_window_state::{AppHandleExt, StateFlags};
                let _ = window.app_handle().save_window_state(
                    StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED,
                );
                let _ = window.hide();
                set_webview_memory_low(window.app_handle(), true);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Hako");
}

/// Set the WebView2 memory-usage target for the main window: `Low` while hidden
/// to tray (the engine drops caches / swaps renderer memory out, freeing system
/// RAM during gameplay), `Normal` when shown again. Scripts keep running either
/// way, so live event listeners and the query cache stay warm. No-op if the
/// window or the WebView2 19+ interface isn't available (older runtimes).
fn set_webview_memory_low(app: &tauri::AppHandle, low: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.with_webview(move |webview| {
        #[cfg(windows)]
        {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
                COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
            };
            use windows::core::Interface;

            // SAFETY: runs on the UI thread that owns the WebView2 controller; the
            // COM calls are all best-effort and ignore failure.
            unsafe {
                let controller = webview.controller();
                if let Ok(core) = controller.CoreWebView2() {
                    if let Ok(core19) = core.cast::<ICoreWebView2_19>() {
                        let level = if low {
                            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
                        } else {
                            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
                        };
                        let _ = core19.SetMemoryUsageTargetLevel(level);
                    }
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (webview, low);
    });
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
                set_webview_memory_low(app, true);
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

/// (Re-)register the global save-clip shortcut on the accelerator `accel`,
/// unregistering `old` first when replacing an existing binding. `accel` is a
/// `global-hotkey` accelerator string (e.g. `F9`, `Alt+F7`). Registration failure
/// is logged, not fatal (e.g. another app already owns the key, or the string is
/// invalid). The save runs on its own thread so the shortcut dispatcher is never
/// blocked by mux IO, and the clip length is read live from settings each press
/// (so the CLIPS duration dropdown takes effect without re-registering).
pub(crate) fn set_clip_hotkey(app: &tauri::AppHandle, old: Option<&str>, accel: &str) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let gs = app.global_shortcut();
    if let Some(old) = old {
        let _ = gs.unregister(old); // silent if it wasn't registered
    }

    let handle = app.clone();
    let res = gs.on_shortcut(accel, move |_app, _shortcut, event| {
        if event.state != ShortcutState::Pressed {
            return; // fire once on press, ignore the release
        }
        let handle = handle.clone();
        // Clip length from settings (clamped to the buffer depth), read live.
        let seconds = handle
            .state::<commands::SettingsState>()
            .0
            .lock()
            .map(|s| s.clip_capture_seconds())
            .unwrap_or(30);
        // save_clip_full emits `clip-created` itself; we just log/surface errors.
        std::thread::spawn(move || match commands::save_clip_full(&handle, seconds, None) {
            Ok(rec) => tracing::info!("hotkey saved clip → {}", rec.path),
            Err(e) => {
                tracing::warn!("clip save failed: {e}");
                let _ = handle.emit(events::RECORDER_ERROR, e);
            }
        });
    });
    if let Err(e) = res {
        tracing::error!("could not register clip hotkey ({accel}): {e}");
    }
}

/// Dismiss the update splash and reveal the main window. Called by the updater
/// window once it's finished (no update available, an error, offline, or after a
/// successful install couldn't relaunch). The main window was created hidden and
/// its geometry was already restored by `tauri-plugin-window-state`, so showing
/// it here is flicker-free — the user never sees it jump from the default size.
#[tauri::command]
fn finish_to_main(app: tauri::AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
        // Visible again → full webview memory (it starts Normal anyway).
        set_webview_memory_low(&app, false);
    }
    if let Some(updater) = app.get_webview_window("updater") {
        let _ = updater.close();
    }
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    // Back to full memory now that the UI is visible again.
    set_webview_memory_low(app, false);
}

