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

mod cloud;
mod commands;
mod events;
mod media;
mod overlay;
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

    // Cap the async runtime size. Tauri's default builds a multi-thread tokio
    // runtime sized to the logical CPU count; our async work (Riot HTTP polls,
    // event emits) is light, so on a gaming box a dozen+ idle worker threads would
    // park on the very cores the game wants. A small fixed pool cuts scheduler
    // wakeups and cache churn during gameplay. Must run before any Tauri call
    // touches the runtime (`set` panics if it's already initialized); the runtime
    // is held in `_rt` for the whole of `main` since dropping it is not allowed.
    let _rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map(|rt| {
            tauri::async_runtime::set(rt.handle().clone());
            rt
        });

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
                // The overlay's geometry tracks the game window at runtime, so
                // it must never be persisted/restored either.
                .with_denylist(&["updater", "overlay"])
                .build(),
        )
        // Auto-update: the splash window checks GitHub Releases and installs a
        // signed update via these plugins; `process` provides the relaunch.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Consumer-cloud OAuth (Phase 2/3): a temporary loopback server catches
        // the browser redirect during a Google Drive / Dropbox / OneDrive connect.
        .plugin(tauri_plugin_oauth::init())
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
            commands::reveal_clip,
            commands::trim_clip,
            commands::clip_audio_tracks,
            commands::read_clip_range,
            commands::remux_with_tracks,
            commands::get_settings,
            commands::update_settings,
            commands::valorant_status,
            commands::overlay_test,
            cloud::cloud_list_providers,
            cloud::cloud_add_provider,
            cloud::cloud_remove_provider,
            cloud::cloud_test_provider,
            cloud::oauth::cloud_connect_gdrive,
            cloud::oauth::cloud_connect_dropbox,
            cloud::oauth::cloud_connect_onedrive,
            cloud::upload::cloud_upload_clip,
            cloud::upload::cloud_cancel_upload,
            cloud::upload::cloud_upload_status,
            cloud::download::cloud_download_clip,
            cloud::retention::cloud_retention_stats,
            cloud::retention::cloud_free_up_space,
            finish_to_main
        ])
        .setup(|app| {
            // The update splash is created hidden + unfocused (see tauri.conf.json)
            // precisely so it can never tab the user out of a fullscreen/borderless
            // game on launch. Reveal it *without activating it*: a plain `show()`
            // (or tao's own initial show) calls `ShowWindow(SW_SHOW)`, which steals
            // the foreground from the game. `show_window_no_activate` instead marks
            // the window `WS_EX_NOACTIVATE` (+ APPWINDOW so it still shows in the
            // taskbar) and shows it with `SW_SHOWNOACTIVATE`, so it paints in the
            // background while the game keeps focus. The splash needs no input — it
            // only reports progress — so non-activating is harmless.
            if let Some(updater) = app.get_webview_window("updater") {
                show_window_no_activate(&updater);
            }
            app.manage(init_library(app.handle()));
            app.manage(init_settings(app.handle()));
            // Cloud upload: managed queue state + the single draining worker.
            // After library/settings so the worker can read them when jobs land.
            app.manage(init_cloud(app.handle()));
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
            set_clip_hotkey(app.handle(), &accel);
            // Live Valorant detection: poll presence, record full matches, and
            // auto-cut highlight clips on match end (Mode B). Degrades to manual
            // clips if Riot/capture aren't available.
            valorant::orchestrator::spawn(app.handle().clone());
            // In-game overlay: warn on the overlay when the clips drive runs low
            // (edge-triggered, only while capturing).
            overlay::spawn_disk_monitor(app.handle().clone());
            // The overlay boots hidden but its WebView2 renderer is live (~75MB).
            // Suspend it shortly after launch so that memory is reclaimed while
            // idle; it auto-resumes when first shown over a capture (see overlay.rs).
            // Delayed so the overlay's mount (event listeners, settings seed) runs
            // first; guarded on still-hidden so an immediate capture doesn't race it.
            let overlay_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                if let Some(win) = overlay_handle.get_webview_window("overlay") {
                    if !win.is_visible().unwrap_or(false) {
                        suspend_window_webview(&win, true);
                    }
                }
            });
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
        // hidden during a match the UI doesn't need to render, so suspend WebView2
        // (pauses its timers/scripts → near-zero renderer CPU, RAM reclaimed by the
        // OS). Resumed on show.
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
                set_webview_suspended(window.app_handle(), true);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Hako");
}

/// Show a window **without giving it the foreground** — used for the update
/// splash so launching while in a fullscreen/borderless game never tabs the user
/// out. We can't do this through Tauri's `show()` (it activates), so we drop to
/// Win32: add `WS_EX_NOACTIVATE` (the window refuses activation on show/click) and
/// `WS_EX_APPWINDOW` (NOACTIVATE windows are hidden from the taskbar by default —
/// this keeps it listed so the user can still click back to it), then show with
/// `SW_SHOWNOACTIVATE`. Best-effort: any failure to get the HWND leaves the window
/// hidden, and the 60 s safety net in `setup` still reveals the main window.
fn show_window_no_activate(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_SHOWNOACTIVATE,
        WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    };
    let Ok(hwnd) = window.hwnd() else {
        tracing::warn!("update splash: no HWND; leaving it hidden");
        return;
    };
    let hwnd = HWND(hwnd.0 as *mut std::ffi::c_void);
    // SAFETY: `hwnd` is a live top-level window we just obtained from Tauri; the
    // style read/write and show are standard, side-effect-free user32 calls.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let want = ex | (WS_EX_NOACTIVATE.0 as isize) | (WS_EX_APPWINDOW.0 as isize);
        if want != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, want);
        }
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// Suspend or resume the main window's WebView2 while it's hidden to tray.
///
/// When hidden during gameplay the UI renders nothing and has no work to do, so
/// we call `ICoreWebView2_3::TrySuspend`: it pauses the renderer's script timers
/// and animations, drops the renderer process to near-zero CPU, and lets the OS
/// reclaim its memory (TrySuspend implies the Low memory target). That's strictly
/// better for gameplay than the old `SetMemoryUsageTargetLevel(Low)`, which —
/// per Microsoft — keeps scripts running, so the React app's timers / query
/// refetch / rAF loops kept waking the CPU on the cores the game wants.
///
/// `TrySuspend` requires the controller to be invisible (else `ERROR_INVALID_STATE`),
/// so callers must `hide()` the window *before* calling this with `suspend=true`.
/// On show, WebView2 auto-resumes once the controller is visible again; we still
/// call `Resume()` explicitly (harmless, and keeps intent obvious). Best-effort:
/// no-op if the window or the WebView2 3+ interface isn't available.
fn set_webview_suspended(app: &tauri::AppHandle, suspend: bool) {
    // Hidden to tray during gameplay → mark the process EcoQoS so the UI/WebView2/
    // async threads prefer efficiency cores (Windows only auto-throttles a hidden
    // window on battery; this covers AC too). The real-time recorder threads opt
    // out via `core::protect_thread_high_qos`, so encoding stays at full speed.
    crate::core::set_process_eco_qos(suspend);
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if !suspend {
        suspend_window_webview(&window, false);
        return;
    }
    // Hiding to tray. Microsoft's WebView2 perf guidance recommends periodically
    // reloading a long-lived webview to return its renderer to a clean memory
    // baseline. We do that here *only when idle* (not capturing): reload the UI,
    // then suspend after a short beat so the reload can settle. During a match the
    // reload would steal CPU the game wants, so we skip it and suspend at once.
    if crate::commands::is_capturing(app) {
        suspend_window_webview(&window, true);
        return;
    }
    let _ = window.eval("window.location.reload()");
    let w = window.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        // Skip if the user reopened the window meanwhile (TrySuspend no-ops on a
        // visible controller anyway; this just avoids the wasted call).
        if !w.is_visible().unwrap_or(false) {
            suspend_window_webview(&w, true);
        }
    });
}

/// Suspend or resume a single window's WebView2 renderer (the COM call only;
/// callers own visibility/EcoQoS/reload policy). `TrySuspend` pauses the
/// renderer's script timers + animations and lets the OS reclaim its memory;
/// `Resume` undoes it. `TrySuspend` requires the controller to be invisible, so
/// callers must `hide()` the window before calling with `suspend=true`.
/// Best-effort: no-op off Windows or if the WebView2 3+ interface is missing.
pub(crate) fn suspend_window_webview(window: &tauri::WebviewWindow, suspend: bool) {
    let _ = window.with_webview(move |webview| {
        #[cfg(windows)]
        {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
            use webview2_com::TrySuspendCompletedHandler;
            use windows::core::Interface;

            // SAFETY: runs on the UI thread that owns the WebView2 controller; the
            // COM calls are all best-effort and ignore failure.
            unsafe {
                let controller = webview.controller();
                let Ok(core) = controller.CoreWebView2() else {
                    return;
                };
                let Ok(core3) = core.cast::<ICoreWebView2_3>() else {
                    return;
                };
                if suspend {
                    // Async + best-effort: TrySuspend defers if a script is mid-run
                    // and returns `ok=false` if the page can't be suspended (e.g.
                    // playing audio, active downloads) — our hidden UI hits none of
                    // these. The completion bool is logged only.
                    let handler = TrySuspendCompletedHandler::create(Box::new(|_hr, ok| {
                        tracing::debug!(suspended = ok, "webview TrySuspend completed");
                        Ok(())
                    }));
                    let _ = core3.TrySuspend(&handler);
                } else {
                    let _ = core3.Resume();
                }
            }
        }
        #[cfg(not(windows))]
        let _ = (webview, suspend);
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
                set_webview_suspended(app, true);
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

/// Build the cloud upload-queue state and spawn its single draining worker. The
/// worker reads `LibraryState`/`SettingsState` and emits progress/status events,
/// so this must run after those are managed.
fn init_cloud(app: &tauri::AppHandle) -> cloud::CloudState {
    // Clear any uploads left mid-flight by a previous run: the queue + worker are
    // in-memory only, so a persisted `queued`/`uploading` row is a zombie that
    // would otherwise show as an active upload forever. Best-effort.
    if let Some(lib) = app.try_state::<commands::LibraryState>() {
        if let Ok(guard) = lib.0.lock() {
            match guard.cloud_reset_interrupted() {
                Ok(n) if n > 0 => tracing::info!("cloud: reset {n} interrupted upload(s) on startup"),
                Ok(_) => {}
                Err(e) => tracing::warn!("cloud: reset interrupted uploads failed: {e}"),
            }
        }
    }
    let (state, rx) = cloud::CloudState::new();
    cloud::upload::spawn_worker(app.clone(), rx);
    state
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

/// (Re-)register the global save-clip shortcut on the accelerator `accel`.
/// `accel` is a `global-hotkey` accelerator string (e.g. `F9`, `Alt+F7`).
/// Registration failure is logged, not fatal (e.g. another app already owns the
/// key, or the string is invalid). The save runs on its own thread so the
/// shortcut dispatcher is never blocked by mux IO, and the clip length is read
/// live from settings each press (so the CLIPS duration dropdown takes effect
/// without re-registering).
///
/// We own exactly one global shortcut, so every (re)bind first clears ALL prior
/// registrations with `unregister_all` rather than a targeted `unregister(old)`.
/// This matters on Windows: `global-hotkey`'s per-key `unregister` drops the
/// plugin's handler-map entry only *after* the OS `UnregisterHotKey` succeeds, so
/// a flaky OS unregister leaves the old key's handler live in the map and it
/// keeps firing — the rebind silently "doesn't take" (old key still clips, and
/// you can't bind back to it). `unregister_all` `mem::take`s the handler map
/// before the OS call, so the old accelerator can never dispatch again even if
/// the OS call hiccups.
pub(crate) fn set_clip_hotkey(app: &tauri::AppHandle, accel: &str) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let gs = app.global_shortcut();
    // Clear any previous binding first. Best-effort, but log failures — a stale
    // registration outliving its rebind is exactly the bug this guards against.
    if let Err(e) = gs.unregister_all() {
        tracing::warn!("could not clear previous clip hotkey(s): {e}");
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
        // Visible again → resume the renderer (it auto-resumes on show anyway).
        set_webview_suspended(&app, false);
    }
    if let Some(updater) = app.get_webview_window("updater") {
        let _ = updater.close();
    }
}

/// Dev-only sink for React Scan render stats. The frontend serializes
/// `getReport()` (per-component render counts/timings) and ships it here so an
/// agent — which can't see the WebView2 overlay or its console — can just `Read`
/// the JSON file. Writes next to the dev working dir and logs the absolute path
/// to the `tauri dev` terminal. No-op in release builds (the frontend never
/// invokes it there, but gate the body too so it can't write files in prod).
#[tauri::command]
fn dump_render_stats(json: String) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("dump_render_stats is dev-only".into());
    }
    let path = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join("react-scan-report.json");
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    let p = path.display().to_string();
    println!("[react-scan] wrote render report → {p}");
    Ok(p)
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
    // Resume the renderer now that the UI is visible again.
    set_webview_suspended(app, false);
}

