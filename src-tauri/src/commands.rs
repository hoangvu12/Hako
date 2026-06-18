//! `#[tauri::command]` handlers — the invoke surface exposed to the webview.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::capture::{self, RunningCapture, WindowTarget};
use crate::core::device::{self, GpuInfo};
use crate::core::encode::{self, FfmpegProbe};
use crate::library::db::{rebase_marks, ClipRecord, EventMark, Library, NewClip};
use crate::settings::Settings;

/// Managed state holding the currently running capture, if any.
#[derive(Default)]
pub struct CaptureState(pub Mutex<Option<RunningCapture>>);

/// Snapshot of recorder state. Mirrors the `RecorderStatus` interface in
/// `src/lib/api.ts`; serde serializes with these exact field names.
#[derive(Debug, Clone, Serialize)]
pub struct RecorderStatus {
    pub capturing: bool,
    /// True when a capture is running AND delivering fresh frames. False while the
    /// game is minimized (frozen): the recorder is alive but the footage is stale,
    /// so the UI shows an honest "paused" state instead of "recording".
    pub capturing_live: bool,
    pub valorant_detected: bool,
    pub encoder: Option<String>,
    pub buffer_seconds: u32,
    pub message: String,
}

/// Status command — live recorder snapshot (capturing + Valorant detection).
#[tauri::command]
pub fn recorder_status(app: AppHandle) -> RecorderStatus {
    recorder_status_snapshot(&app)
}

/// Compute the current recorder status: whether a capture is running and whether
/// the VALORANT game window is present. Shared by the `recorder_status` command
/// and the orchestrator's per-tick `recorder-status` event (which drives the
/// titlebar's "Now Clipping" indicator live).
pub fn recorder_status_snapshot(app: &AppHandle) -> RecorderStatus {
    // Read capturing + liveness from one lock so they're consistent: a capture is
    // "live" only while it's delivering fresh frames (not minimized/frozen).
    let (capturing, capturing_live) = app
        .state::<CaptureState>()
        .0
        .lock()
        .map(|g| match g.as_ref() {
            Some(c) => (true, c.capturing_live()),
            None => (false, false),
        })
        .unwrap_or((false, false));
    let valorant_detected = capture::find_valorant_window().is_some();
    let buffer_seconds = app
        .state::<SettingsState>()
        .0
        .lock()
        .map(|s| s.buffer_seconds)
        .unwrap_or(30);
    let message = match (valorant_detected, capturing, capturing_live) {
        // Capturing but frozen — be honest that footage is paused.
        (_, true, false) => "Paused — game minimized",
        (true, true, true) => "Recording Valorant",
        (true, false, _) => "Valorant detected",
        (false, true, true) => "Capturing",
        (false, false, _) => "Waiting for game",
    }
    .to_string();
    RecorderStatus {
        capturing,
        capturing_live,
        valorant_detected,
        encoder: None,
        buffer_seconds,
        message,
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
    /// Resolved capture adapter for the current "Selected GPU" setting.
    /// `None` if no usable adapter was found.
    pub capture_adapter: Option<u32>,
    /// Resolved encode adapter for the current setting (== `capture_adapter` on
    /// the zero-copy fast path; a different discrete GPU when cross-adapter).
    pub encode_adapter: Option<u32>,
    /// True when the resolved encode adapter differs from the capture adapter, so
    /// a cross-adapter NV12 hand-off would be needed (Medal-style discrete NVENC).
    pub cross_adapter: bool,
    /// Whether the cross-adapter capability probe passed (a shared keyed-mutex
    /// NV12 texture round-trips capture→encode device). Always true on the
    /// non-cross fast path; false → the pipeline would fall back to single-device.
    pub cross_adapter_ok: bool,
    /// Why the cross-adapter probe failed (so the UI can explain it), else `None`.
    pub cross_adapter_reason: Option<String>,
}

/// Enumerate GPUs and validate that we can open a D3D11 device on the
/// preferred adapter (the foundation of the zero-copy pipeline). Also resolves
/// the (capture, encode) adapter pair for the current "Selected GPU" setting and
/// runs the cross-adapter capability probe (Phase 0 of cross-adapter encode).
#[tauri::command]
pub fn gpu_info(settings: State<SettingsState>) -> GpuReport {
    let adapters = match device::enumerate_gpus() {
        Ok(a) => a,
        Err(e) => {
            return GpuReport {
                adapters: Vec::new(),
                selected_encoder: None,
                device_ok: false,
                feature_level: None,
                error: Some(format!("DXGI enumeration failed: {e}")),
                capture_adapter: None,
                encode_adapter: None,
                cross_adapter: false,
                cross_adapter_ok: true,
                cross_adapter_reason: None,
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

    // Resolve the (capture, encode) pair for the saved "Selected GPU" choice and
    // probe the cross-adapter hand-off (a no-op for the single-device fast path).
    let requested_encode = settings.0.lock().ok().and_then(|s| s.gpu_adapter_index());
    let plan = device::resolve_adapters(&adapters, requested_encode);
    let (capture_adapter, encode_adapter, cross_adapter, cross_adapter_ok, cross_adapter_reason) =
        match plan {
            Some(p) => {
                let probe = device::probe_cross_adapter(&p);
                (
                    Some(p.capture_idx),
                    Some(p.encode_idx),
                    p.cross,
                    probe.ok,
                    probe.reason,
                )
            }
            None => (None, None, false, true, None),
        };

    GpuReport {
        adapters,
        selected_encoder,
        device_ok,
        feature_level,
        error,
        capture_adapter,
        encode_adapter,
        cross_adapter,
        cross_adapter_ok,
        cross_adapter_reason,
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

/// Active microphone / capture endpoints for the "Microphone Source" picker.
#[tauri::command]
pub fn list_audio_inputs() -> Vec<crate::core::audio::AudioInputDevice> {
    crate::core::audio::enumerate_inputs()
}

/// Active render endpoints (speakers / headphones) for the "PC Audio"
/// multi-select in `all_pc_audio` mode.
#[tauri::command]
pub fn list_audio_outputs() -> Vec<crate::core::audio::AudioOutputDevice> {
    crate::core::audio::enumerate_outputs()
}

/// Apps currently playing audio — the live source list for `specific_apps` mode
/// ("additional apps appear here when they play audio"). Polled by the UI.
#[tauri::command]
pub fn list_active_audio_sessions() -> Vec<crate::core::audio::AudioSession> {
    crate::core::audio::enumerate_active_sessions()
}

/// Whether Windows per-process loopback (Windows build ≥ 20348) is available, so
/// the UI can gate the `specific_apps` recording mode and fall back to
/// `all_pc_audio` when it isn't supported.
#[tauri::command]
pub fn process_loopback_supported() -> bool {
    crate::core::audio::is_process_loopback_supported()
}

/// Start capture of the given window (HWND as integer) at `target_fps`.
#[tauri::command]
pub fn start_capture(
    app: AppHandle,
    hwnd: i64,
    target_fps: Option<u32>,
    adapter_index: Option<u32>,
) -> Result<(), String> {
    start_capture_with(&app, hwnd, target_fps, adapter_index)
}

/// Start a capture of `hwnd`, pulling fps/buffer/audio/backend from saved
/// settings. Shared by the `start_capture` command and the Valorant
/// orchestrator's auto-start (Medal-style game detection). Errors if a capture
/// is already running.
pub fn start_capture_with(
    app: &AppHandle,
    hwnd: i64,
    target_fps: Option<u32>,
    adapter_index: Option<u32>,
) -> Result<(), String> {
    let settings = app.state::<SettingsState>();
    let state = app.state::<CaptureState>();
    // Defaults (fps, buffer length, audio config, backend) come from saved
    // settings. `effective_audio()` yields the Medal-style per-source config,
    // synthesizing one from the legacy fields for pre-feature configs.
    let (cfg_fps, buffer_secs, to_disk, audio_cfg, enc_cfg, cfg_adapter) = {
        let s = settings.0.lock().map_err(|_| "settings poisoned")?;
        (
            s.target_fps,
            s.buffer_seconds,
            s.buffers_to_disk(),
            s.effective_audio(),
            encode::EncodeSettings {
                codec: encode::VideoCodec::from_setting(&s.codec),
                bitrate_mbps: s.bitrate_mbps,
                target_res: s.resolution_dims(),
            },
            s.gpu_adapter_index(),
        )
    };
    // Disk buffer (Medal's "Recording buffer: Disk"): spool the instant-replay
    // window to rolling segment files under `<clips>/.hako-buffer` instead of RAM.
    // `None` ⇒ RAM. If the dir can't be prepared we log and fall back to RAM.
    let disk_buffer_dir = if to_disk {
        match buffer_dir(app) {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("disk buffer dir unavailable ({e}); using RAM buffer");
                None
            }
        }
    } else {
        None
    };
    let mut guard = state.0.lock().map_err(|_| "capture state poisoned")?;
    if guard.is_some() {
        return Err("capture already running".into());
    }
    let fps = target_fps.unwrap_or(cfg_fps);
    // Explicit adapter from the caller wins; otherwise use the saved "Selected GPU"
    // (Auto → None, i.e. the display-owning adapter).
    let adapter_index = adapter_index.or(cfg_adapter);
    // Capture via the injected graphics hook (the app's only backend). See
    // `core::hook`.
    let running = capture::start_hook(
        app.clone(), hwnd, fps, adapter_index, buffer_secs, disk_buffer_dir, audio_cfg, enc_cfg,
    )?;
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

/// Stop the running capture from a plain `AppHandle` (no `State` extractor) —
/// the orchestrator's auto-stop when the game exits. No-op if none.
pub fn stop_capture_with(app: &AppHandle) {
    if let Ok(mut guard) = app.state::<CaptureState>().0.lock() {
        if let Some(mut running) = guard.take() {
            running.stop();
        }
    }
}

/// Whether a capture is currently running (from a plain `AppHandle`).
pub fn is_capturing(app: &AppHandle) -> bool {
    app.state::<CaptureState>()
        .0
        .lock()
        .map(|g| g.is_some())
        .unwrap_or(false)
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

    // Best-effort thumbnail + scrubber filmstrip — a clip without them is valid.
    let thumb = generate_thumbnail(app, &saved.path);
    let filmstrip = generate_filmstrip(app, &saved.path, saved.duration_secs);

    let size_bytes = std::fs::metadata(&saved.path).map(|m| m.len() as i64).unwrap_or(0);
    let title = saved
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Clip")
        .to_string();
    // If a Valorant match is live, tag the manual clip with the same agent/map/
    // mode an auto-clip would carry (win/loss + K/D/A are unknowable mid-match).
    // All-`None` when saved outside a match.
    let context = app
        .try_state::<crate::valorant::live::LiveMatchState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.clip_context()))
        .unwrap_or_default();
    let new = NewClip {
        path: saved.path.to_string_lossy().to_string(),
        title,
        event: event.map(|s| s.to_string()),
        events: event.into_iter().map(|s| s.to_string()).collect(),
        duration_secs: saved.duration_secs,
        width: saved.width as i64,
        height: saved.height as i64,
        size_bytes,
        thumb_path: thumb,
        filmstrip_path: filmstrip,
        ..context
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

/// Save the last `seconds` of buffered gameplay (defaults to the configured clip
/// length, clamped to the buffer depth). Returns the record.
#[tauri::command]
pub fn save_clip(app: AppHandle, seconds: Option<u32>) -> Result<ClipRecord, String> {
    let seconds = seconds.unwrap_or_else(|| {
        app.state::<SettingsState>()
            .0
            .lock()
            .map(|s| s.clip_capture_seconds())
            .unwrap_or(30)
    });
    save_clip_full(&app, seconds, None)
}

/// A fresh `<Videos>/Hako/hako_clip_<ms>.mp4` path (for the auto-clipper to cut
/// session sub-ranges into).
pub fn auto_clip_output_path(app: &AppHandle) -> Result<PathBuf, String> {
    clip_output_path(app)
}

/// Register an already-written clip file (e.g. a Valorant auto-clip cut from the
/// Mode-B session) into the library: generate a thumbnail, insert the row, and
/// emit `clip-created`. `dimensions`/`duration` come from the cut result so we
/// don't re-probe. `event` tags it (e.g. "Ace").
pub fn finalize_auto_clip(
    app: &AppHandle,
    path: PathBuf,
    title: String,
    event: &str,
    events: &[String],
    // Per-event positions within the clip (label + offset seconds), for the
    // editor's seek-bar markers. Empty for whole-session saves.
    event_marks: Vec<EventMark>,
    width: i64,
    height: i64,
    duration_secs: f64,
    // Game context for this clip (agent/map/mode/result/K-D-A). Only the
    // context fields are read; build it via `MatchSummary::clip_context()` (or
    // `NewClip::default()` when no match context is available).
    context: NewClip,
) -> Result<ClipRecord, String> {
    let thumb = generate_thumbnail(app, &path);
    let filmstrip = generate_filmstrip(app, &path, duration_secs);
    let size_bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
    let new = NewClip {
        path: path.to_string_lossy().to_string(),
        title,
        event: Some(event.to_string()),
        events: events.to_vec(),
        event_marks,
        duration_secs,
        width,
        height,
        size_bytes,
        thumb_path: thumb,
        filmstrip_path: filmstrip,
        ..context
    };
    let library = app.state::<LibraryState>();
    let record = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        let id = lib.insert(&new)?;
        lib.get(id)?.ok_or("inserted clip vanished")?
    };
    let _ = app.emit(crate::events::CLIP_CREATED, &record);
    tracing::info!("auto-clip saved ({event}) → {}", record.path);
    Ok(record)
}

/// Human-readable label for a clip's event(s): "Ace + Kill" when a merged
/// window covers several, else the single event. Empty list ⇒ falls back to
/// `primary`.
pub fn events_summary(primary: &str, events: &[String]) -> String {
    if events.len() > 1 {
        events.join(" + ")
    } else {
        events.first().map(String::as_str).unwrap_or(primary).to_string()
    }
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
        if let Some(f) = rec.filmstrip_path {
            let _ = std::fs::remove_file(f);
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

/// Reveal a clip's file in the OS file manager (Windows Explorer), with the file
/// selected — the "Open in folder" action in the clip viewer.
#[tauri::command]
pub fn reveal_clip(library: State<LibraryState>, id: i64) -> Result<(), String> {
    let rec = library
        .0
        .lock()
        .map_err(|_| "library poisoned")?
        .get(id)?
        .ok_or("clip not found")?;
    reveal_in_explorer(&rec.path)
}

#[cfg(windows)]
fn reveal_in_explorer(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // `explorer /select,"<path>"` opens the containing folder with the file
    // selected. raw_arg keeps `/select,` unquoted and only the path quoted — the
    // exact form Explorer parses (a fully-quoted single token silently misfires
    // and opens Documents instead). Filenames can't contain `"`, so there's no
    // quote to escape out of. Explorer can exit non-zero spuriously, so we only
    // spawn and don't inspect the status.
    std::process::Command::new("explorer")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to open Explorer: {e}"))
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &str) -> Result<(), String> {
    Err("revealing files is only supported on Windows".into())
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
            let filmstrip = generate_filmstrip(&app, &out, res.duration_secs);
            let size_bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            let new = NewClip {
                path: out.to_string_lossy().to_string(),
                title: format!("{} (trim)", rec.title),
                event: rec.event.clone(),
                events: rec.events.clone(),
                event_marks: rebase_marks(&rec.event_marks, start, end),
                duration_secs: res.duration_secs,
                width: res.width,
                height: res.height,
                size_bytes,
                thumb_path: thumb,
                filmstrip_path: filmstrip,
                ..NewClip::context_from(&rec)
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
            let filmstrip = generate_filmstrip(&app, &input, res.duration_secs);
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
                    filmstrip.as_deref(),
                )?;
                lib.update_event_marks(id, &rebase_marks(&rec.event_marks, start, end))?;
                lib.get(id)?.ok_or("clip vanished after trim")?
            };
            tracing::info!("trimmed clip {id} → overwrite {}", record.path);
            Ok(record)
        }
    }
}

/// The audio tracks in a clip (count + names), for the editor's per-track
/// mute/solo/volume controls. Audio track 0 is the master "All Audio" mix;
/// 1..N are the stems. A clip with ≤1 track shows no per-track UI.
#[tauri::command]
pub fn clip_audio_tracks(
    library: State<LibraryState>,
    id: i64,
) -> Result<Vec<crate::library::remux::AudioTrackInfo>, String> {
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    crate::library::remux::probe_audio_tracks(&PathBuf::from(&rec.path))
}

/// Read a byte range `[start, end)` of a clip file for the editor's live
/// per-stem audio mixer. mediabunny decodes the stems in the webview, but it
/// can't `fetch()` the `hakoclip://` streaming scheme — WebView2 blocks
/// cross-origin fetch to a custom scheme by CORS (the `<video>` element is
/// exempt, which is why playback and export work but the mixer's decode didn't).
/// So the stem bytes are pulled over IPC via mediabunny's `CustomSource`.
/// Returns the raw slice as an `ArrayBuffer`; `end` is clamped to the file size.
#[tauri::command]
pub async fn read_clip_range(
    library: State<'_, LibraryState>,
    id: i64,
    start: u64,
    end: u64,
) -> Result<tauri::ipc::Response, String> {
    use std::io::{Read, Seek, SeekFrom};
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let mut file = std::fs::File::open(&rec.path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let end = end.min(size);
    let start = start.min(end);
    let mut buf = vec![0u8; (end - start) as usize];
    if !buf.is_empty() {
        file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    }
    Ok(tauri::ipc::Response::new(buf))
}

/// One selected stem from the editor: its audio-track index + 0–100 volume.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct TrackVolume {
    pub index: u32,
    pub volume: f32,
}

/// Export a clip to `[start, end)` with its audio being the chosen `tracks`
/// (stems) mixed at their volumes — the editor's per-track mute/solo/volume
/// applied on export (browsers can't switch MP4 audio tracks live). Empty
/// `tracks` ⇒ video-only; one stem at full volume ⇒ loss-less stream copy;
/// otherwise the stems are decoded, mixed, and re-encoded to one master track.
/// `Copy` writes a new library clip; `Overwrite` replaces the original.
#[tauri::command]
pub fn remux_with_tracks(
    app: AppHandle,
    library: State<LibraryState>,
    id: i64,
    start: f64,
    end: f64,
    tracks: Vec<TrackVolume>,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let input = PathBuf::from(&rec.path);
    let sel: Vec<crate::library::remux::TrackSel> = tracks
        .iter()
        .map(|t| crate::library::remux::TrackSel {
            index: t.index,
            gain: (t.volume.max(0.0)) / 100.0,
        })
        .collect();

    match mode {
        TrimMode::Copy => {
            let out = clip_output_path(&app)?;
            let res =
                crate::library::remux::remux_with_tracks(&input, &out, start, end, &sel)?;
            let thumb = generate_thumbnail(&app, &out);
            let filmstrip = generate_filmstrip(&app, &out, res.duration_secs);
            let size_bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            let new = NewClip {
                path: out.to_string_lossy().to_string(),
                title: format!("{} (export)", rec.title),
                event: rec.event.clone(),
                events: rec.events.clone(),
                event_marks: rebase_marks(&rec.event_marks, start, end),
                duration_secs: res.duration_secs,
                width: res.width,
                height: res.height,
                size_bytes,
                thumb_path: thumb,
                filmstrip_path: filmstrip,
                ..NewClip::context_from(&rec)
            };
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                let id = lib.insert(&new)?;
                lib.get(id)?.ok_or("inserted clip vanished")?
            };
            let _ = app.emit(crate::events::CLIP_CREATED, &record);
            tracing::info!("remuxed clip {id} → copy {}", record.path);
            Ok(record)
        }
        TrimMode::Overwrite => {
            let tmp = clip_output_path(&app)?;
            let res =
                crate::library::remux::remux_with_tracks(&input, &tmp, start, end, &sel)?;
            replace_file_retrying(&tmp, &input)?;
            let thumb = generate_thumbnail(&app, &input);
            let filmstrip = generate_filmstrip(&app, &input, res.duration_secs);
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
                    filmstrip.as_deref(),
                )?;
                lib.update_event_marks(id, &rebase_marks(&rec.event_marks, start, end))?;
                lib.get(id)?.ok_or("clip vanished after remux")?
            };
            tracing::info!("remuxed clip {id} → overwrite {}", record.path);
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

/// Replace + persist user settings. When the save-clip hotkey changed, the global
/// shortcut is re-registered on the new accelerator (the old binding is dropped).
#[tauri::command]
pub fn update_settings(
    app: AppHandle,
    settings: State<SettingsState>,
    next: Settings,
) -> Result<(), String> {
    let path = settings_path(&app)?;
    next.save(&path)?;
    let new_hotkey = next.save_hotkey.clone();
    let (old_hotkey, capture_changed) = {
        let mut guard = settings.0.lock().map_err(|_| "settings poisoned")?;
        let prev_hotkey = guard.save_hotkey.clone();
        let capture_changed = guard.capture_config_differs(&next);
        *guard = next;
        (prev_hotkey, capture_changed)
    };
    if old_hotkey != new_hotkey {
        crate::set_clip_hotkey(&app, Some(&old_hotkey), &new_hotkey);
    }
    // A running capture snapshots its fps/buffer/codec/audio config at start, so
    // a change (e.g. enabling the microphone) wouldn't apply to the live buffer.
    // Restart it against the same window to pick up the new config — but never
    // mid-match (that would orphan the in-progress session's buffer); those
    // changes apply when the next match's capture starts.
    if capture_changed {
        restart_capture_for_config_change(&app);
    }
    Ok(())
}

/// Restart the live buffer capture so a settings change takes effect, if one is
/// running and no Valorant match is actively recording into it. Best-effort: a
/// failed restart leaves capture stopped, which the orchestrator re-starts on
/// the next game-window poll.
fn restart_capture_for_config_change(app: &AppHandle) {
    let state = app.state::<CaptureState>();
    // Read the target window + match-busy flag, then drop the lock before
    // stop/start (both take the same lock internally).
    let hwnd = {
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match guard.as_ref() {
            None => return, // no capture running — change applies next start
            Some(running) if running.has_active_session() => {
                tracing::info!(
                    "settings: capture config changed mid-match; applying after the match ends"
                );
                return;
            }
            Some(running) => running.hwnd(),
        }
    };
    stop_capture_with(app);
    if let Err(e) = start_capture_with(app, hwnd, None, None) {
        tracing::warn!("settings: capture restart after config change failed: {e}");
    } else {
        tracing::info!("settings: restarted capture to apply new audio/encode config");
    }
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

/// `<Videos>/Hako/.hako-buffer` — the private spool dir for the disk recording
/// buffer (created/emptied by [`crate::core::disk_buffer::DiskPacketRing`]). Kept
/// on the same drive as clips so it inherits the user's storage choice.
fn buffer_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(clip_dir(app)?.join(".hako-buffer"))
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

/// Number of frames / per-tile width in the editor scrubber's sprite-sheet.
const FILMSTRIP_TILES: u32 = 16;
const FILMSTRIP_TILE_WIDTH: u32 = 160;

/// Extract a sprite-sheet filmstrip next to the clip
/// (`<Videos>/Hako/thumbs/<stem>_strip.jpg`). Best-effort: a clip without one
/// falls back to the poster in the editor.
fn generate_filmstrip(app: &AppHandle, video: &Path, duration_secs: f64) -> Option<String> {
    let dir = clip_dir(app).ok()?.join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = video.file_stem()?.to_str()?;
    let out = dir.join(format!("{stem}_strip.jpg"));
    match crate::library::thumbs::extract_filmstrip(
        video,
        &out,
        FILMSTRIP_TILES,
        FILMSTRIP_TILE_WIDTH,
        duration_secs,
    ) {
        Ok(()) => Some(out.to_string_lossy().to_string()),
        Err(e) => {
            tracing::warn!("filmstrip failed for {}: {e}", video.display());
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
