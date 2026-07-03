//! In-game overlay toasts: the notice contract emitted to the `overlay` window
//! plus the Win32 positioning that tracks the game window.
//!
//! The overlay is a separate transparent, click-through, always-on-top window
//! (see `overlay.html` + the `overlay` window in `tauri.conf.json`). It is never
//! injected into or drawn inside the game process — that's a hard anti-cheat
//! constraint. Rust owns *what* to show and *where*; the React host just renders.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

/// Default auto-dismiss for a toast (the React host falls back to this too).
pub const DEFAULT_TTL_MS: u32 = 3500;
/// Shorter ttl for the "Recording stopped" toast — it has to render before the
/// overlay window is hidden (see [`on_capture_stopped`]).
const STOPPED_TTL_MS: u32 = 1600;
/// Longer ttl for the disk-low warning (the user should have time to read it).
const DISK_TTL_MS: u32 = 5000;

/// Small overlay surface size. Keeping this window toast-sized avoids forcing a
/// full-game transparent WebView through DWM composition for an entire match.
const TOAST_WINDOW_W: i32 = 400;
const TOAST_WINDOW_H: i32 = 320;
const TOAST_MARGIN: i32 = 16;
/// Bump every time a toast is shown so an older hide timer cannot hide a newer
/// toast that arrived before the old timer fired.
static OVERLAY_EPOCH: AtomicU64 = AtomicU64::new(0);

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

/// Fire a toast over Valorant (or the primary monitor if no game is present),
/// using the same show → notify → hide lifecycle as live in-game toasts.
pub fn toast_over_game(app: &AppHandle, notice: OverlayNotice) {
    show_toast(app, None, notice);
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
    let _ = app.emit_to(
        "overlay",
        crate::events::OVERLAY_CONFIG,
        OverlayConfig { position },
    );
}

/// Hide the overlay window now (e.g. the master switch was turned off while a
/// capture is running, so it shouldn't linger over the game).
pub fn hide_now(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
        // Suspend the (now invisible) renderer so its ~75MB is reclaimed while idle.
        crate::suspend_window_webview(&win, true);
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

/// Move + size the overlay window to a compact toast surface and show it.
fn fit_overlay_to_rect(app: &AppHandle, x: i32, y: i32, w: i32, h: i32) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_position(tauri::PhysicalPosition { x, y });
        let _ = win.set_size(tauri::PhysicalSize {
            width: w.max(1) as u32,
            height: h.max(1) as u32,
        });
        let _ = win.show();
        // Resume the renderer only while a toast is visible. It is suspended again
        // by the toast-scoped hide timer below.
        crate::suspend_window_webview(&win, false);
    }
}

fn overlay_position(app: &AppHandle) -> String {
    setting(app, "top_right".to_string(), |s| s.overlay_position.clone())
}

fn toast_rect_within(bounds: (i32, i32, i32, i32), position: &str) -> (i32, i32, i32, i32) {
    let (bx, by, bw, bh) = bounds;
    let w = TOAST_WINDOW_W.min(bw.max(1));
    let h = TOAST_WINDOW_H.min(bh.max(1));
    let left = position.ends_with("left");
    let top = position.starts_with("top");
    let x = if left {
        bx + TOAST_MARGIN.min((bw - w).max(0))
    } else {
        bx + (bw - w - TOAST_MARGIN).max(0)
    };
    let y = if top {
        by + TOAST_MARGIN.min((bh - h).max(0))
    } else {
        by + (bh - h - TOAST_MARGIN).max(0)
    };
    (x, y, w, h)
}

fn primary_monitor_bounds(app: &AppHandle) -> Option<(i32, i32, i32, i32)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let pos = monitor.position();
    let size = monitor.size();
    Some((pos.x, pos.y, size.width as i32, size.height as i32))
}

fn current_capture_hwnd(app: &AppHandle) -> Option<i64> {
    app.try_state::<crate::commands::CaptureState>()
        .and_then(|s| s.0.lock().ok().and_then(|g| g.as_ref().map(|r| r.hwnd())))
}

/// Position and show the compact overlay surface for one toast. `hwnd` pins it to
/// the captured game when known; otherwise it falls back to Valorant (test button)
/// or the primary monitor. It intentionally does not cover the full game rect.
fn show_overlay_for_toast(app: &AppHandle, hwnd: Option<i64>) -> u64 {
    push_config(app);
    let position = overlay_position(app);
    let bounds = hwnd
        .and_then(window_bounds)
        .or_else(|| crate::core::capture::find_valorant_window().and_then(window_bounds))
        .or_else(|| primary_monitor_bounds(app));
    if let Some(bounds) = bounds {
        let (x, y, w, h) = toast_rect_within(bounds, &position);
        fit_overlay_to_rect(app, x, y, w, h);
    } else if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
        crate::suspend_window_webview(&win, false);
    }
    OVERLAY_EPOCH.fetch_add(1, Ordering::AcqRel) + 1
}

/// Hide the overlay window after `delay_ms`, unless a newer toast was shown in
/// the meantime. This is toast-scoped: capture may still be running when we hide.
fn hide_overlay_after(app: &AppHandle, delay_ms: u64, epoch: u64) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        if OVERLAY_EPOCH.load(Ordering::Acquire) == epoch {
            if let Some(win) = app.get_webview_window("overlay") {
                let _ = win.hide();
                crate::suspend_window_webview(&win, true);
            }
        }
    });
}

fn show_toast(app: &AppHandle, hwnd: Option<i64>, notice: OverlayNotice) {
    let ttl = notice.ttl_ms.max(1) as u64;
    let epoch = show_overlay_for_toast(app, hwnd);
    notify(app, notice);
    hide_overlay_after(app, ttl + 450, epoch);
}

// --- Triggers ---------------------------------------------------------------
// One helper per trigger so the call sites stay a single line and the settings
// gating (Part D) has a single place to land per trigger.

/// Capture started: toast "Now recording" if that trigger is on. The overlay is
/// shown only for the toast lifetime, never for the whole capture.
pub fn on_capture_started(app: &AppHandle, hwnd: i64) {
    if !enabled(app) || !setting(app, true, |s| s.overlay_on_capture_state) {
        return;
    }
    show_toast(
        app,
        Some(hwnd),
        OverlayNotice {
            kind: OverlayKind::RecordingStarted,
            title: "Now recording".into(),
            subtitle: None,
            ttl_ms: DEFAULT_TTL_MS,
        },
    );
}

/// Capture stopped: toast "Recording stopped" (if that trigger is on).
/// If the trigger is off, hide any leftover toast surface promptly.
pub fn on_capture_stopped(app: &AppHandle) {
    if !enabled(app) {
        return;
    }
    if setting(app, true, |s| s.overlay_on_capture_state) {
        show_toast(
            app,
            None,
            OverlayNotice {
                kind: OverlayKind::RecordingStopped,
                title: "Recording stopped".into(),
                subtitle: None,
                ttl_ms: STOPPED_TTL_MS,
            },
        );
    } else {
        hide_now(app);
    }
}

/// A manual clip was saved (F9 / UI button). `seconds` is the captured length.
pub fn on_clip_saved(app: &AppHandle, seconds: u32) {
    if !enabled(app) || !setting(app, true, |s| s.overlay_on_clip_saved) {
        return;
    }
    show_toast(
        app,
        current_capture_hwnd(app),
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
    show_toast(
        app,
        current_capture_hwnd(app),
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
