//! `#[tauri::command]` handlers — the invoke surface exposed to the webview.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::capture::{self, RunningCapture, WindowTarget};
use crate::core::device::{self, GpuInfo};
use crate::core::encode::{self, FfmpegProbe};
use crate::library::db::{ClipRecord, Library, NewClip};
use crate::settings::Settings;

/// Managed state holding the currently running capture, if any.
#[derive(Default)]
pub struct CaptureState(pub Mutex<Option<RunningCapture>>);

/// Snapshot of recorder state. Mirrors the `RecorderStatus` interface in
/// `src/lib/api.ts`; serde serializes with these exact field names.
#[derive(Debug, Clone, Serialize)]
pub struct RecorderStatus {
    pub capturing: bool,
    pub valorant_detected: bool,
    pub encoder: Option<String>,
    pub buffer_seconds: u32,
    pub message: String,
}

/// Status command. Returns idle state.
#[tauri::command]
pub fn recorder_status() -> RecorderStatus {
    RecorderStatus {
        capturing: false,
        valorant_detected: false,
        encoder: None,
        buffer_seconds: 30,
        message: "Recorder idle".into(),
    }
}

/// GPU adapters + the encoder/device we'd use (Dashboard "GPU/encoder in use").
#[derive(Debug, Clone, Serialize)]
pub struct GpuReport {
    pub adapters: Vec<GpuInfo>,
    pub selected_encoder: Option<String>,
    /// Whether a shared D3D11 device opened on the preferred adapter.
    pub device_ok: bool,
    pub feature_level: Option<String>,
    pub error: Option<String>,
}

/// Enumerate GPUs and validate that we can open a D3D11 device on the
/// preferred adapter (the foundation of the zero-copy pipeline).
#[tauri::command]
pub fn gpu_info() -> GpuReport {
    let adapters = match device::enumerate_gpus() {
        Ok(a) => a,
        Err(e) => {
            return GpuReport {
                adapters: Vec::new(),
                selected_encoder: None,
                device_ok: false,
                feature_level: None,
                error: Some(format!("DXGI enumeration failed: {e}")),
            };
        }
    };

    let preferred = adapters.iter().find(|g| g.preferred);
    let selected_encoder = preferred.and_then(|g| g.encoder.clone());

    // Validate the shared device opens on the chosen adapter.
    let (device_ok, feature_level, error) = match preferred {
        Some(g) => match device::adapter_at(g.index).and_then(|a| device::create_device(Some(&a))) {
            Ok((_dev, _ctx, fl)) => (true, Some(feature_level_label(fl)), None),
            Err(e) => (false, None, Some(format!("D3D11CreateDevice failed: {e}"))),
        },
        None => (false, None, Some("no hardware encoder-capable adapter found".into())),
    };

    GpuReport {
        adapters,
        selected_encoder,
        device_ok,
        feature_level,
        error,
    }
}

/// Probe the bundled FFmpeg: versions + which hardware encoders are reachable.
#[tauri::command]
pub fn ffmpeg_info() -> FfmpegProbe {
    encode::probe()
}

/// List visible top-level windows that can be captured (capture picker).
#[tauri::command]
pub fn list_windows() -> Vec<WindowTarget> {
    capture::list_windows()
}

/// Start WGC capture of the given window (HWND as integer) at `target_fps`.
#[tauri::command]
pub fn start_capture(
    app: AppHandle,
    state: State<CaptureState>,
    settings: State<SettingsState>,
    hwnd: i64,
    target_fps: Option<u32>,
    adapter_index: Option<u32>,
) -> Result<(), String> {
    // Defaults (fps, buffer length, audio, backend) come from saved settings.
    let (cfg_fps, buffer_secs, capture_audio, use_hook) = {
        let s = settings.0.lock().map_err(|_| "settings poisoned")?;
        (s.target_fps, s.buffer_seconds, s.capture_audio, s.uses_hook_capture())
    };
    let mut guard = state.0.lock().map_err(|_| "capture state poisoned")?;
    if guard.is_some() {
        return Err("capture already running".into());
    }
    let fps = target_fps.unwrap_or(cfg_fps);
    // `hook` = opt-in graphics-hook injection (beats the DWM cap, anti-cheat
    // risk); anything else = WGC (default, Vanguard-safe). See `core::hook`.
    let running = if use_hook {
        capture::start_hook(app, hwnd, fps, adapter_index, buffer_secs, capture_audio)?
    } else {
        capture::start(app, hwnd, fps, adapter_index, buffer_secs, capture_audio)?
    };
    *guard = Some(running);
    Ok(())
}

/// Stop the running capture (no-op if none).
#[tauri::command]
pub fn stop_capture(state: State<CaptureState>) -> Result<(), String> {
    if let Some(mut running) = state.0.lock().map_err(|_| "capture state poisoned")?.take() {
        running.stop();
    }
    Ok(())
}

/// Whether a capture session is currently running. The recorder lives on
/// background threads (independent of the UI), so the frontend queries this to
/// re-sync its state after navigating away and back — it must NOT stop capture
/// just because a component unmounted.
#[tauri::command]
pub fn capture_status(state: State<CaptureState>) -> bool {
    state.0.lock().map(|g| g.is_some()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Clip library + settings (managed state)
// ---------------------------------------------------------------------------

/// Managed SQLite clip library.
pub struct LibraryState(pub Mutex<Library>);

/// Managed, persisted user settings.
pub struct SettingsState(pub Mutex<Settings>);

/// Save the last `seconds` of the RAM ring to MP4 (stream-copy, no re-encode),
/// generate a thumbnail, record it in the library, and emit `clip-created`.
/// Shared by the `save_clip` command and the F9 hotkey. `event` tags the clip
/// (e.g. "Ace"); `None` ⇒ a manual hotkey clip.
///
/// Clones the clip buffer out of capture state first, then releases the capture
/// lock before muxing so start/stop aren't blocked by file IO.
pub fn save_clip_full(
    app: &AppHandle,
    seconds: u32,
    event: Option<&str>,
) -> Result<ClipRecord, String> {
    let clip = {
        let state = app.state::<CaptureState>();
        let guard = state.0.lock().map_err(|_| "capture state poisoned")?;
        guard.as_ref().ok_or("no capture is running")?.clip()
    };
    let out = clip_output_path(app)?;
    let saved = clip.save_last(seconds, &out)?;

    // Best-effort thumbnail — a clip without one is still valid.
    let thumb = generate_thumbnail(app, &saved.path);

    let size_bytes = std::fs::metadata(&saved.path).map(|m| m.len() as i64).unwrap_or(0);
    let title = saved
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Clip")
        .to_string();
    let new = NewClip {
        path: saved.path.to_string_lossy().to_string(),
        title,
        event: event.map(|s| s.to_string()),
        duration_secs: saved.duration_secs,
        width: saved.width as i64,
        height: saved.height as i64,
        size_bytes,
        thumb_path: thumb,
    };

    let library = app.state::<LibraryState>();
    let record = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        let id = lib.insert(&new)?;
        lib.get(id)?.ok_or("inserted clip vanished")?
    };

    let _ = app.emit(crate::events::CLIP_CREATED, &record);
    tracing::info!("saved clip ({seconds}s) → {}", record.path);
    Ok(record)
}

/// Save the last `seconds` (default 30) of buffered gameplay. Returns the record.
#[tauri::command]
pub fn save_clip(app: AppHandle, seconds: Option<u32>) -> Result<ClipRecord, String> {
    save_clip_full(&app, seconds.unwrap_or(30), None)
}

/// All clips in the library, newest first.
#[tauri::command]
pub fn clips_list(library: State<LibraryState>) -> Result<Vec<ClipRecord>, String> {
    library.0.lock().map_err(|_| "library poisoned")?.list()
}

/// Delete a clip: remove the row, the MP4, and its thumbnail from disk.
#[tauri::command]
pub fn delete_clip(library: State<LibraryState>, id: i64) -> Result<(), String> {
    let lib = library.0.lock().map_err(|_| "library poisoned")?;
    if let Some(rec) = lib.get(id)? {
        let _ = std::fs::remove_file(&rec.path);
        if let Some(t) = rec.thumb_path {
            let _ = std::fs::remove_file(t);
        }
    }
    lib.delete(id)
}

/// Rename a clip's title.
#[tauri::command]
pub fn rename_clip(library: State<LibraryState>, id: i64, title: String) -> Result<(), String> {
    library
        .0
        .lock()
        .map_err(|_| "library poisoned")?
        .rename(id, &title)
}

/// Where a trim writes its result: replace the original file, or save a copy.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrimMode {
    Overwrite,
    Copy,
}

/// Loss-lessly trim a clip to `[start, end)` seconds (stream copy, optional audio
/// drop). `Copy` writes a new library clip; `Overwrite` replaces the original
/// file in place and refreshes its row. Returns the resulting record.
///
/// The FFmpeg remux is slow IO, so the library lock is only held to read the
/// source row and to commit the result — never across the trim itself.
#[tauri::command]
pub fn trim_clip(
    app: AppHandle,
    library: State<LibraryState>,
    id: i64,
    start: f64,
    end: f64,
    drop_audio: bool,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let input = PathBuf::from(&rec.path);

    match mode {
        TrimMode::Copy => {
            let out = clip_output_path(&app)?;
            let res = crate::library::trim::trim_clip(&input, &out, start, end, drop_audio)?;
            let thumb = generate_thumbnail(&app, &out);
            let size_bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            let new = NewClip {
                path: out.to_string_lossy().to_string(),
                title: format!("{} (trim)", rec.title),
                event: rec.event.clone(),
                duration_secs: res.duration_secs,
                width: res.width,
                height: res.height,
                size_bytes,
                thumb_path: thumb,
            };
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                let id = lib.insert(&new)?;
                lib.get(id)?.ok_or("inserted clip vanished")?
            };
            let _ = app.emit(crate::events::CLIP_CREATED, &record);
            tracing::info!("trimmed clip {id} → copy {}", record.path);
            Ok(record)
        }
        TrimMode::Overwrite => {
            // Trim to a fresh temp file, then atomically swap it over the
            // original (which the webview may still hold open — hence retries).
            let tmp = clip_output_path(&app)?;
            let res = crate::library::trim::trim_clip(&input, &tmp, start, end, drop_audio)?;
            replace_file_retrying(&tmp, &input)?;

            let thumb = generate_thumbnail(&app, &input);
            let size_bytes = std::fs::metadata(&input).map(|m| m.len() as i64).unwrap_or(0);
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                lib.update_media(
                    id,
                    res.duration_secs,
                    res.width,
                    res.height,
                    size_bytes,
                    thumb.as_deref(),
                )?;
                lib.get(id)?.ok_or("clip vanished after trim")?
            };
            tracing::info!("trimmed clip {id} → overwrite {}", record.path);
            Ok(record)
        }
    }
}

/// Replace `dst` with `src`, retrying briefly: right after a trim the webview's
/// `<video>` may still hold the old file open (Windows share-deny), so a rename
/// can transiently fail until the element releases it.
fn replace_file_retrying(src: &Path, dst: &Path) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..20 {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(std::time::Duration::from_millis(if attempt < 5 {
                    50
                } else {
                    150
                }));
            }
        }
    }
    let _ = std::fs::remove_file(src);
    Err(format!("could not replace original clip (file in use?): {last}"))
}

/// Current user settings.
#[tauri::command]
pub fn get_settings(settings: State<SettingsState>) -> Result<Settings, String> {
    Ok(settings.0.lock().map_err(|_| "settings poisoned")?.clone())
}

/// Replace + persist user settings.
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    settings: State<SettingsState>,
    next: Settings,
) -> Result<(), String> {
    let path = settings_path(&app)?;
    next.save(&path)?;
    *settings.0.lock().map_err(|_| "settings poisoned")? = next;
    Ok(())
}

/// Best-effort live Valorant status for the `/valorant` panel.
#[tauri::command]
pub async fn valorant_status() -> crate::valorant::service::ValorantStatus {
    crate::valorant::service::probe_status().await
}

/// `<Videos>/Hako`, created if needed (clip + thumbnail storage root).
fn clip_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .video_dir()
        .map_err(|e| format!("resolve Videos dir: {e}"))?
        .join("Hako");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create clip dir: {e}"))?;
    Ok(dir)
}

/// `<Videos>/Hako/hako_clip_<unix_ms>.mp4` (ms so rapid presses don't collide).
fn clip_output_path(app: &AppHandle) -> Result<PathBuf, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(clip_dir(app)?.join(format!("hako_clip_{ts}.mp4")))
}

/// `settings.json` in the app config dir.
fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("resolve config dir: {e}"))?;
    Ok(Settings::file_in(&dir))
}

/// Extract a thumbnail next to the clip (`<Videos>/Hako/thumbs/<stem>.jpg`).
fn generate_thumbnail(app: &AppHandle, video: &Path) -> Option<String> {
    let dir = clip_dir(app).ok()?.join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = video.file_stem()?.to_str()?;
    let out = dir.join(format!("{stem}.jpg"));
    match crate::library::thumbs::extract_thumbnail(video, &out, 480) {
        Ok(()) => Some(out.to_string_lossy().to_string()),
        Err(e) => {
            tracing::warn!("thumbnail failed for {}: {e}", video.display());
            None
        }
    }
}

fn feature_level_label(fl: windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL) -> String {
    use windows::Win32::Graphics::Direct3D::{D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
    match fl {
        f if f == D3D_FEATURE_LEVEL_11_1 => "11_1".into(),
        f if f == D3D_FEATURE_LEVEL_11_0 => "11_0".into(),
        other => format!("0x{:04x}", other.0),
    }
}
