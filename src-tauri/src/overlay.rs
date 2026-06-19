//! In-game overlay toasts: the notice contract emitted to the `overlay` window
//! plus the Win32 positioning that tracks the game window.
//!
//! The overlay is a separate transparent, click-through, always-on-top window
//! (see `overlay.html` + the `overlay` window in `tauri.conf.json`). It is never
//! injected into or drawn inside the game process — that's a hard anti-cheat
//! constraint. Rust owns *what* to show and *where*; the React host just renders.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Default auto-dismiss for a toast (the React host falls back to this too).
pub const DEFAULT_TTL_MS: u32 = 3500;
/// Shorter ttl for the "Recording stopped" toast — it has to render before the
/// overlay window is hidden (see [`on_capture_stopped`]).
const STOPPED_TTL_MS: u32 = 1600;
/// Longer ttl for the disk-low warning (the user should have time to read it).
const DISK_TTL_MS: u32 = 5000;

/// A single overlay toast. Serialized to the `overlay` window as the
/// `overlay-notify` event payload; mirrored by `OverlayNotice` in
/// `src/lib/api.ts`. serde emits camelCase to match the TS interface.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayNotice {
    /// Drives the toast's icon + accent color.
    pub kind: OverlayKind,
    /// Headline, e.g. "Clip saved".
    pub title: String,
    /// Optional second line, e.g. "Last 30s", "1.2 GB free".
    pub subtitle: Option<String>,
    /// Auto-dismiss after this many ms.
    pub ttl_ms: u32,
}

/// The notice kind. serde emits snake_case (`recording_started`, …) to match
/// the `OverlayKind` union in `src/lib/api.ts`.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayKind {
    RecordingStarted,
    RecordingStopped,
    ClipSaved,
    DiskLow,
}

/// Emit a toast to the overlay window. Best-effort: a missing window simply
/// drops the notice. Callers gate on settings first; this is the raw emit.
pub fn notify(app: &AppHandle, notice: OverlayNotice) {
    let _ = app.emit_to("overlay", crate::events::OVERLAY_NOTIFY, notice);
}

/// Overlay window placement, pushed to the React host so it knows which corner
/// to stack toasts in. Mirrors `OverlayConfig` in `src/lib/api.ts`.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayConfig {
    /// `top_left` | `top_right` | `bottom_left` | `bottom_right`.
    pub position: String,
}

/// Read one field off the saved settings, falling back to `default` if the
/// settings state is missing or poisoned.
fn setting<T>(app: &AppHandle, default: T, f: impl FnOnce(&crate::settings::Settings) -> T) -> T {
    app.try_state::<crate::commands::SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| f(&g)))
        .unwrap_or(default)
}

/// Whether the overlay is enabled at all (master switch).
fn enabled(app: &AppHandle) -> bool {
    setting(app, true, |s| s.overlay_enabled)
}

/// Push the current corner placement to the overlay window. Called whenever the
/// overlay is shown and when settings change, so a visible overlay re-corners
/// live and the next show always uses the latest choice.
pub fn push_config(app: &AppHandle) {
    let position = setting(app, "top_right".to_string(), |s| s.overlay_position.clone());
    let _ = app.emit_to("overlay", crate::events::OVERLAY_CONFIG, OverlayConfig { position });
}

/// Hide the overlay window now (e.g. the master switch was turned off while a
/// capture is running, so it shouldn't linger over the game).
pub fn hide_now(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

/// Physical-pixel bounds `(x, y, w, h)` of a top-level window via `GetWindowRect`
/// (the rect is already in physical pixels, so the overlay positions correctly
/// at any DPI). `None` for a stale/invalid HWND or an empty rect.
pub fn window_bounds(hwnd: i64) -> Option<(i32, i32, i32, i32)> {
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    let mut rect = RECT::default();
    // SAFETY: GetWindowRect only reads the window rect into `rect`; a stale HWND
    // returns an error (mapped to None below).
    unsafe { GetWindowRect(HWND(hwnd as *mut c_void), &mut rect).ok()? };
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    (w > 0 && h > 0).then_some((rect.left, rect.top, w, h))
}

/// Move + size the overlay window to cover the physical-pixel rect `(x, y, w, h)`
/// and show it. Covering the full game rect keeps toast placement to pure CSS
/// (matches Medal's full-rect overlay).
pub fn fit_overlay_to_rect(app: &AppHandle, x: i32, y: i32, w: i32, h: i32) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_position(tauri::PhysicalPosition { x, y });
        let _ = win.set_size(tauri::PhysicalSize {
            width: w as u32,
            height: h as u32,
        });
        let _ = win.show();
    }
}

/// Position the overlay over the live Valorant window and show it, falling back
/// to the primary monitor when the game isn't running (so the Settings "Test
/// overlay" button still surfaces a toast on-screen).
pub fn show_overlay_over_game(app: &AppHandle) {
    push_config(app);
    if let Some((x, y, w, h)) =
        crate::core::capture::find_valorant_window().and_then(window_bounds)
    {
        fit_overlay_to_rect(app, x, y, w, h);
        return;
    }
    // No game window: cover the primary monitor so the toast renders in the
    // chosen corner rather than in the placeholder 400x200 box.
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let pos = monitor.position();
        let size = monitor.size();
        fit_overlay_to_rect(app, pos.x, pos.y, size.width as i32, size.height as i32);
    } else if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
    }
}

/// Position the overlay over `hwnd` (the window being captured) and show it. If
/// the bounds can't be read, just show it at its last geometry.
pub fn show_overlay_over_hwnd(app: &AppHandle, hwnd: i64) {
    push_config(app);
    if let Some((x, y, w, h)) = window_bounds(hwnd) {
        fit_overlay_to_rect(app, x, y, w, h);
    } else if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
    }
}

/// Hide the overlay window after `delay_ms` — but only if nothing is capturing
/// by then. A capture may have (re)started during the delay (e.g. a settings
/// change restarts the buffer), and we must not yank a freshly-shown overlay.
fn hide_overlay_after(app: &AppHandle, delay_ms: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        if !crate::commands::is_capturing(&app) {
            if let Some(win) = app.get_webview_window("overlay") {
                let _ = win.hide();
            }
        }
    });
}

// --- Triggers ---------------------------------------------------------------
// One helper per trigger so the call sites stay a single line and the settings
// gating (Part D) has a single place to land per trigger.

/// Capture started: show the overlay over the captured window (so any in-game
/// toast can appear), then toast "Now recording" if that trigger is on. No-op
/// when the overlay is disabled — nothing is ever shown.
pub fn on_capture_started(app: &AppHandle, hwnd: i64) {
    if !enabled(app) {
        return;
    }
    show_overlay_over_hwnd(app, hwnd);
    if setting(app, true, |s| s.overlay_on_capture_state) {
        notify(
            app,
            OverlayNotice {
                kind: OverlayKind::RecordingStarted,
                title: "Now recording".into(),
                subtitle: None,
                ttl_ms: DEFAULT_TTL_MS,
            },
        );
    }
}

/// Capture stopped: toast "Recording stopped" (if that trigger is on), then hide
/// the overlay once the toast has rendered (emit → short delay → hide, per the
/// plan's teardown-race note).
pub fn on_capture_stopped(app: &AppHandle) {
    if !enabled(app) {
        return;
    }
    let delay = if setting(app, true, |s| s.overlay_on_capture_state) {
        notify(
            app,
            OverlayNotice {
                kind: OverlayKind::RecordingStopped,
                title: "Recording stopped".into(),
                subtitle: None,
                ttl_ms: STOPPED_TTL_MS,
            },
        );
        // Give the toast a beat beyond its ttl to play its exit animation.
        STOPPED_TTL_MS as u64 + 400
    } else {
        // No stopped toast: tear the surface down promptly.
        250
    };
    hide_overlay_after(app, delay);
}

/// A manual clip was saved (F9 / UI button). `seconds` is the captured length.
pub fn on_clip_saved(app: &AppHandle, seconds: u32) {
    if !enabled(app) || !setting(app, true, |s| s.overlay_on_clip_saved) {
        return;
    }
    notify(
        app,
        OverlayNotice {
            kind: OverlayKind::ClipSaved,
            title: "Clip saved".into(),
            subtitle: Some(format!("Last {seconds}s")),
            ttl_ms: DEFAULT_TTL_MS,
        },
    );
}

/// The clips drive is running low. `free` is the remaining free bytes.
pub fn on_disk_low(app: &AppHandle, free: u64) {
    if !enabled(app) || !setting(app, true, |s| s.overlay_on_disk_low) {
        return;
    }
    notify(
        app,
        OverlayNotice {
            kind: OverlayKind::DiskLow,
            title: "Storage almost full".into(),
            subtitle: Some(format!("{} free", fmt_bytes(free))),
            ttl_ms: DISK_TTL_MS,
        },
    );
}

// --- Disk-space monitor -----------------------------------------------------

/// Warn when the clips drive drops below 5 GB free.
const DISK_LOW_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Re-arm only once free space recovers above 6 GB (hysteresis, so a drive
/// hovering at the threshold doesn't flap on/off).
const DISK_REARM_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// How often to poll free space.
const DISK_POLL_SECS: u64 = 45;

/// Background poller for the clips drive's free space. Edge-triggered: fires the
/// disk-low toast once on the OK→low crossing and re-arms only after recovering
/// past the hysteresis band. Only surfaces while capturing — the overlay is
/// in-game-only, so out-of-game disk warnings belong in the main UI (out of
/// scope here).
pub fn spawn_disk_monitor(app: AppHandle) {
    std::thread::spawn(move || {
        let mut warned = false;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(DISK_POLL_SECS));
            if !crate::commands::is_capturing(&app) {
                continue;
            }
            let Some(dir) = crate::commands::storage_root(&app) else {
                continue;
            };
            let Some(free) = free_bytes_for(&dir) else {
                continue;
            };
            if !warned && free < DISK_LOW_BYTES {
                warned = true;
                tracing::info!("clips drive low: {} free", fmt_bytes(free));
                on_disk_low(&app, free);
            } else if warned && free >= DISK_REARM_BYTES {
                warned = false; // recovered — re-arm for the next crossing
            }
        }
    });
}

/// Free bytes available to the caller on the volume containing `path`, via
/// `GetDiskFreeSpaceExW`. `None` if the query fails.
fn free_bytes_for(path: &std::path::Path) -> Option<u64> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let dir = HSTRING::from(path.as_os_str());
    let mut free: u64 = 0;
    // SAFETY: writes the free-bytes-available-to-caller out-param; the other two
    // out-params are null (we don't need total/total-free).
    unsafe { GetDiskFreeSpaceExW(&dir, Some(&mut free as *mut u64), None, None).ok()? };
    Some(free)
}

/// Compact human size for the disk-low subtitle (e.g. "4.3 GB", "780 MB").
fn fmt_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}
