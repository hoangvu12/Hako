//! `#[tauri::command]` handlers — the invoke surface exposed to the webview.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::capture::{self, RunningCapture, WindowTarget};
use crate::core::device::{self, GpuInfo};
use crate::core::encode::{self, FfmpegProbe};
use crate::library::db::{rebase_marks, shift_marks, ClipRecord, EventMark, Library, NewClip};
use crate::settings::{AudioConfig, Settings};

/// Managed state holding the currently running capture, if any.
#[derive(Default)]
pub struct CaptureState(pub Mutex<Option<RunningCapture>>);

/// Cross-thread request for a capture restart that must be performed by the
/// Valorant orchestrator rather than the settings command thread.
///
/// `update_settings` runs on a command thread and can't reach the orchestrator's
/// loop-local in-progress recording, so it can't restart capture mid-session
/// itself (that would orphan the session's buffer). When a restart-class config
/// change (video encode or audio track layout) lands while a session is teeing
/// into the capture, it sets this flag instead; the orchestrator polls it each
/// tick, finalizes the current clip, restarts capture, and reopens a fresh
/// session for the rest — a clean two-clip split (Medal restarts the same way,
/// it just has no continuous match file to preserve). Outside a session,
/// `update_settings` restarts immediately and never sets this.
#[derive(Default)]
pub struct ConfigRestartSignal(pub AtomicBool);

/// Atomically read + clear the pending config-restart request set by
/// `update_settings` (see [`ConfigRestartSignal`]). Returns true exactly once per
/// request, so the orchestrator acts on it a single time.
pub fn take_config_restart_request(app: &AppHandle) -> bool {
    app.try_state::<ConfigRestartSignal>()
        .map_or(false, |s| s.0.swap(false, Ordering::AcqRel))
}

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
                freeze_overlay: s.freeze_overlay,
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
    drop(guard);
    // Show + position the in-game overlay over the captured window and toast
    // "Now recording" (toasts only appear while capturing).
    crate::overlay::on_capture_started(app, hwnd);
    Ok(())
}

/// Stop the running capture (no-op if none). Delegates to [`stop_capture_with`]
/// so the UI-stop path runs the same overlay teardown as the orchestrator's.
#[tauri::command]
pub fn stop_capture(app: AppHandle) -> Result<(), String> {
    stop_capture_with(&app);
    Ok(())
}

/// Stop the running capture from a plain `AppHandle` (no `State` extractor) —
/// the orchestrator's auto-stop when the game exits, and the `stop_capture`
/// command. No-op if none. Toasts "Recording stopped" and hides the overlay
/// (on a short delay) when a capture was actually running.
pub fn stop_capture_with(app: &AppHandle) {
    let was_running = match app.state::<CaptureState>().0.lock() {
        Ok(mut guard) => match guard.take() {
            Some(mut running) => {
                running.stop();
                true
            }
            None => false,
        },
        Err(_) => false,
    };
    if was_running {
        crate::overlay::on_capture_stopped(app);
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

/// Whether [`SettingsState`]/[`LibraryState`] have been hydrated from disk yet.
///
/// Both are managed on the builder with *placeholders* (default settings, an
/// in-memory library) before any window exists, to keep an IPC call that wins the
/// startup race from panicking on unmanaged state (see `main.rs`). `setup` then
/// replaces them with the real on-disk values and flips this to `true`. The
/// frontend watches the `state-hydrated` event (and reads this via
/// [`app_hydrated`]) to refetch once the real state lands, so a first
/// `get_settings`/`clips_list` that read placeholders self-heal.
#[derive(Default)]
pub struct HydratedState(pub std::sync::atomic::AtomicBool);

/// Whether the managed state has been hydrated from disk (see [`HydratedState`]).
/// The frontend polls this once after subscribing to `state-hydrated` to cover
/// the race where hydration finished before the listener was registered.
#[tauri::command]
pub fn app_hydrated(state: State<HydratedState>) -> bool {
    state.0.load(std::sync::atomic::Ordering::Acquire)
}

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
    // Opt-in auto-upload to the default cloud provider (no-op when disabled).
    crate::cloud::upload::maybe_auto_upload(app, record.id);
    // In-game overlay toast (manual F9 / UI saves only — auto-clips use the
    // separate `finalize_auto_clip` path and don't toast).
    crate::overlay::on_clip_saved(app, seconds);
    tracing::info!("saved clip ({seconds}s) → {}", record.path);
    Ok(record)
}

/// Save the last `seconds` of buffered gameplay (defaults to the configured clip
/// length, clamped to the buffer depth). Returns the record.
#[tauri::command]
pub async fn save_clip(app: AppHandle, seconds: Option<u32>) -> Result<ClipRecord, String> {
    // Muxing the buffered clip + rendering its thumbnail/filmstrip is heavy IO;
    // off the main thread so a manual save (UI button / F9) doesn't freeze the app.
    tauri::async_runtime::spawn_blocking(move || {
        let seconds = seconds.unwrap_or_else(|| {
            app.state::<SettingsState>()
                .0
                .lock()
                .map(|s| s.clip_capture_seconds())
                .unwrap_or(30)
        });
        save_clip_full(&app, seconds, None)
    })
    .await
    .map_err(|e| format!("save task failed: {e}"))?
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
    // Opt-in auto-upload to the default cloud provider (no-op when disabled).
    crate::cloud::upload::maybe_auto_upload(app, record.id);
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
pub async fn trim_clip(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    drop_audio: bool,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    // Off the main thread (see `remux_with_tracks`): the FFmpeg stream-copy, the
    // thumbnail/filmstrip render, and the overwrite retry-sleep would otherwise
    // freeze the UI for the duration of the save.
    tauri::async_runtime::spawn_blocking(move || {
        trim_clip_blocking(app, id, start, end, drop_audio, mode)
    })
    .await
    .map_err(|e| format!("trim task failed: {e}"))?
}

fn trim_clip_blocking(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    drop_audio: bool,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let library = app.state::<LibraryState>();
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
                event_marks: shift_marks(&rebase_marks(&rec.event_marks, start, end), res.start_shift_secs),
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
                lib.update_event_marks(id, &shift_marks(&rebase_marks(&rec.event_marks, start, end), res.start_shift_secs))?;
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
pub async fn clip_audio_tracks(
    app: AppHandle,
    id: i64,
) -> Result<Vec<crate::library::remux::AudioTrackInfo>, String> {
    // `probe_audio_tracks` opens the file with FFmpeg — file IO + demux setup that
    // runs on the main thread for a sync command. This fires the moment a clip is
    // opened in the editor, so do it on the blocking pool to keep the open snappy.
    tauri::async_runtime::spawn_blocking(move || {
        let library = app.state::<LibraryState>();
        let rec = {
            let lib = library.0.lock().map_err(|_| "library poisoned")?;
            lib.get(id)?.ok_or("clip not found")?
        };
        crate::library::remux::probe_audio_tracks(&PathBuf::from(&rec.path))
    })
    .await
    .map_err(|e| format!("probe task failed: {e}"))?
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

/// One selected stem from the editor: its audio-track index, 0–100 volume, and
/// whether to apply offline noise suppression to it (the mic stem's "noise
/// cancel"). `denoise` defaults to false for older callers (serde default).
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct TrackVolume {
    pub index: u32,
    pub volume: f32,
    #[serde(default)]
    pub denoise: bool,
}

/// Export a clip to `[start, end)` with its audio being the chosen `tracks`
/// (stems) mixed at their volumes — the editor's per-track mute/solo/volume
/// applied on export (browsers can't switch MP4 audio tracks live). Empty
/// `tracks` ⇒ video-only; one stem at full volume ⇒ loss-less stream copy;
/// otherwise the stems are decoded, mixed, and re-encoded to one master track.
/// `Copy` writes a new library clip; `Overwrite` replaces the original.
/// The FFmpeg decode/mix/re-encode (and, on overwrite, the retry-sleep loop in
/// `replace_file_retrying`) is heavy, blocking work. A synchronous `#[command]`
/// runs on the app's main thread, so doing this inline froze the whole UI while a
/// clip exported. The command (below) runs this on the blocking pool instead, so
/// the main thread stays free and the webview's "Saving…" state keeps animating.
#[tauri::command]
pub async fn remux_with_tracks(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    tracks: Vec<TrackVolume>,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    tauri::async_runtime::spawn_blocking(move || {
        remux_with_tracks_blocking(app, id, start, end, tracks, mode)
    })
    .await
    .map_err(|e| format!("export task failed: {e}"))?
}

fn remux_with_tracks_blocking(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    tracks: Vec<TrackVolume>,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let library = app.state::<LibraryState>();
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
            denoise: t.denoise,
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
                event_marks: shift_marks(&rebase_marks(&rec.event_marks, start, end), res.start_shift_secs),
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
                lib.update_event_marks(id, &shift_marks(&rebase_marks(&rec.event_marks, start, end), res.start_shift_secs))?;
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
    let overlay_enabled = next.overlay_enabled;
    let new_freeze_overlay = next.freeze_overlay;
    let new_audio = next.effective_audio();
    // Classify the capture-affecting change the way Medal does:
    //  - a layout-preserving audio change applies LIVE (no restart): a volume/
    //    mute edit (`AudioCaptureVolume` → `SetInputVolume`) or a device swap /
    //    mono flip that keeps the same track layout (`UpdateAudioCaptureAndProcessor`);
    //  - a video-encode change or an audio *layout* change (mic on/off, source
    //    add/remove, separate-tracks, mode switch) needs a capture RESTART.
    let (old_hotkey, restart_needed, audio_live, freeze_changed) = {
        let mut guard = settings.0.lock().map_err(|_| "settings poisoned")?;
        let prev_hotkey = guard.save_hotkey.clone();
        let old_audio = guard.effective_audio();
        let video_changed = guard.video_capture_config_differs(&next);
        let audio_changed = old_audio != new_audio;
        // Layout-preserving change → hot-apply; layout change → restart.
        let audio_live = audio_changed && old_audio.layout_eq(&new_audio);
        let restart_needed = video_changed || (audio_changed && !old_audio.layout_eq(&new_audio));
        // The freeze overlay is a per-frame flag — applied live, never a restart.
        let freeze_changed = guard.freeze_overlay != new_freeze_overlay;
        *guard = next;
        (prev_hotkey, restart_needed, audio_live, freeze_changed)
    };
    if old_hotkey != new_hotkey {
        crate::set_clip_hotkey(&app, &new_hotkey);
    }
    // Changing the clip folder only affects where *new* clips are written (via
    // `clip_dir` reading the fresh setting); moving existing clips is opt-in and
    // driven from the UI (see `count_clips_in` / `migrate_clips_to`). Either way,
    // grant the new folder asset-scope access so clips written there load over
    // `convertFileSrc` instead of 403-ing past the static `$VIDEO/Hako` scope.
    allow_storage_asset_scope(&app);
    // A layout-preserving audio change (volume/mute or a device swap) is pushed
    // straight to the running audio thread — it applies immediately, even
    // mid-match, with no restart (Medal's live `SetInputVolume` /
    // `UpdateAudioCaptureAndProcessor`). Anything that changes the encoder or the
    // audio track layout needs a capture restart instead; that snapshots config
    // at start, so we restart against the same window. Mid-match, the restart is
    // handed to the orchestrator for a clean clip split (see
    // `restart_capture_for_config_change` / `ConfigRestartSignal`).
    if restart_needed {
        restart_capture_for_config_change(&app);
    } else if audio_live {
        apply_audio_config_live(&app, &new_audio);
    }
    // The in-frame freeze overlay ("tabbed out" card) is a per-frame flag, so a
    // change applies to the running capture immediately — no restart, even
    // mid-match.
    if freeze_changed {
        apply_freeze_overlay_live(&app, new_freeze_overlay);
    }
    // Keep a live overlay in sync: re-push the corner placement, and clear the
    // overlay immediately if the master switch was just turned off.
    crate::overlay::push_config(&app);
    if !overlay_enabled {
        crate::overlay::hide_now(&app);
    }
    Ok(())
}

/// Push a layout-preserving audio change (volume/mute or a device swap) to the
/// running capture's audio thread, applying it live with no restart (Medal's
/// `AudioCaptureVolume` / `UpdateAudioCaptureAndProcessor`). Works even mid-match
/// — it never touches the encoder or the track layout: the thread re-derives mix
/// gains and reopens only the input whose device changed. No-op when nothing is
/// capturing (the change is already persisted and picked up at the next start).
fn apply_audio_config_live(app: &AppHandle, audio: &AudioConfig) {
    let state = app.state::<CaptureState>();
    let guard = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(running) = guard.as_ref() {
        running.reconfigure_audio(audio.clone());
        tracing::info!("settings: applied live audio reconfigure (no capture restart)");
    }
}

/// Toggle the in-frame freeze overlay on the running capture live (no restart —
/// it's a per-frame flag). No-op when nothing is capturing (the change is already
/// persisted and applied when capture next starts).
fn apply_freeze_overlay_live(app: &AppHandle, on: bool) {
    let state = app.state::<CaptureState>();
    let guard = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(running) = guard.as_ref() {
        running.set_freeze_overlay(on);
        tracing::info!("settings: applied freeze-overlay toggle live ({on})");
    }
}

/// Restart the live buffer capture so a settings change takes effect. When no
/// session is teeing into the capture, restart immediately. When one *is* (a
/// Valorant match or a rolling full session), restarting here would orphan its
/// buffer — so request a clean split from the orchestrator instead (see
/// [`ConfigRestartSignal`]), which finalizes the current clip before restarting.
fn restart_capture_for_config_change(app: &AppHandle) {
    {
        let state = app.state::<CaptureState>();
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match guard.as_ref() {
            None => return, // no capture running — change applies next start
            Some(running) if running.has_active_session() => {
                // A session is recording into this capture. Hand the restart to
                // the orchestrator so it can split the clip first instead of
                // dropping the in-progress recording.
                if let Some(sig) = app.try_state::<ConfigRestartSignal>() {
                    sig.0.store(true, Ordering::Release);
                }
                tracing::info!(
                    "settings: capture config changed mid-session; requesting a clean split restart"
                );
                return;
            }
            Some(_) => {}
        }
    }
    restart_capture_now(app);
}

/// Stop + restart the running capture against the same window so a new
/// encode/audio config takes effect. Does **not** check for an active session —
/// the caller must ensure none is teeing into the capture (the orchestrator
/// finalizes its match/full session first). No-op if nothing is capturing.
/// Best-effort: a failed restart leaves capture stopped, which the orchestrator
/// re-starts on the next game-window poll.
pub fn restart_capture_now(app: &AppHandle) {
    // Read the target window, then drop the lock before stop/start (both take
    // the same lock internally).
    let hwnd = {
        let state = app.state::<CaptureState>();
        let guard = match state.0.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match guard.as_ref() {
            None => return, // no capture running — change applies next start
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

/// Fire a sample in-game overlay toast (Settings → "Test overlay"). Force-shows
/// the overlay positioned over Valorant — or the primary monitor when the game
/// isn't running — regardless of capture state, so placement, transparency, and
/// click-through can be verified without launching a match.
#[tauri::command]
pub fn overlay_test(app: AppHandle) {
    use crate::overlay::{notify, show_overlay_over_game, OverlayKind, OverlayNotice, DEFAULT_TTL_MS};
    show_overlay_over_game(&app);
    notify(
        &app,
        OverlayNotice {
            kind: OverlayKind::ClipSaved,
            title: "Clip saved".into(),
            subtitle: Some("Test overlay · last 30s".into()),
            ttl_ms: DEFAULT_TTL_MS,
        },
    );
}

/// The directory clips are written to (`<Videos>/Hako`), for the overlay's
/// disk-space monitor. `None` if the Videos dir can't be resolved.
pub fn storage_root(app: &AppHandle) -> Option<PathBuf> {
    clip_dir(app).ok()
}

/// Clip + thumbnail storage root, created if needed. Uses the user's configured
/// `storage_dir` setting when set; otherwise falls back to `<Videos>/Hako`.
fn clip_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let configured = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.storage_dir.clone()))
        .flatten();
    resolve_clip_dir(app, configured.as_deref())
}

/// Grant the asset-protocol scope read access to the configured clip folder, so
/// `convertFileSrc` (clip-card video, posters, filmstrips) can load clips written
/// outside the static `$VIDEO/Hako` scope declared in `tauri.conf.json`. No-op
/// when `storage_dir` is unset (the default folder is already in the static
/// scope). Best-effort: a failure just means those clips fall back to 403, which
/// is logged. Recursive so the `thumbs/` subdir is covered too.
pub fn allow_storage_asset_scope(app: &AppHandle) {
    let dir = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.storage_dir.clone()))
        .flatten();
    let Some(dir) = dir else { return };
    let dir = dir.trim();
    if dir.is_empty() {
        return;
    }
    if let Err(e) = app.asset_protocol_scope().allow_directory(dir, true) {
        tracing::warn!("could not grant asset scope for clip folder {dir}: {e}");
    }
}

/// The default storage root, `<Videos>/Hako`.
fn default_clip_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .video_dir()
        .map_err(|e| format!("resolve Videos dir: {e}"))?
        .join("Hako"))
}

/// Resolve a storage root from a raw `storage_dir` setting value, creating it on
/// disk. An empty/unset value uses the default `<Videos>/Hako`. A configured path
/// that can't be created (bad path, unplugged drive, missing permissions) falls
/// back to the default so saving a clip never hard-fails on a stale setting.
fn resolve_clip_dir(app: &AppHandle, storage_dir: Option<&str>) -> Result<PathBuf, String> {
    let configured = storage_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    if let Some(custom) = configured {
        match std::fs::create_dir_all(&custom) {
            Ok(()) => return Ok(custom),
            Err(e) => tracing::warn!(
                "clip folder {} is unusable ({e}); falling back to the default folder",
                custom.display()
            ),
        }
    }
    let dir = default_clip_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create clip dir: {e}"))?;
    Ok(dir)
}

/// Move `src` to `dst`, falling back to copy+remove when the two live on
/// different volumes (`rename` is `EXDEV` across drives). No-op if `src` is
/// missing. The destination's parent is created as needed.
fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() || src == dst {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // Cross-volume (or replacing an existing file): copy then drop the original.
    std::fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    std::fs::remove_file(src).map_err(|e| format!("remove {}: {e}", src.display()))?;
    Ok(())
}

/// If `stored` lives under `old_root`, return its path rebased onto `new_root`
/// (preserving the sub-path, e.g. `thumbs/<stem>.jpg`). Returns `None` for files
/// that aren't under the old root, so clips already elsewhere are left alone.
fn relocate(stored: &str, old_root: &Path, new_root: &Path) -> Option<PathBuf> {
    Path::new(stored)
        .strip_prefix(old_root)
        .ok()
        .map(|rel| new_root.join(rel))
}

/// How many library clips currently live under `root`.
fn count_clips_under(app: &AppHandle, root: &Path) -> u32 {
    let Some(library) = app.try_state::<LibraryState>() else {
        return 0;
    };
    let clips = match library.0.lock() {
        Ok(lib) => lib.list().unwrap_or_default(),
        Err(_) => return 0,
    };
    clips
        .iter()
        .filter(|c| Path::new(&c.path).starts_with(root))
        .count() as u32
}

/// Move every clip (and its thumbnail/filmstrip) currently under `old_root` to
/// `new_root` and repoint the library rows. Returns the number of clips moved.
/// Best-effort per file: a clip that fails to move keeps its old path, and a
/// missing thumbnail just regenerates lazily. Synchronous — callers run it off
/// the UI thread (the command below wraps it in `spawn_blocking`).
fn migrate_clips(app: &AppHandle, old_root: &Path, new_root: &Path) -> u32 {
    let library = app.state::<LibraryState>();
    // Snapshot under the lock, then release it for the slow file IO.
    let clips = match library.0.lock() {
        Ok(lib) => lib.list().unwrap_or_default(),
        Err(_) => return 0,
    };

    let mut moved = 0u32;
    for clip in clips {
        let Some(new_path) = relocate(&clip.path, old_root, new_root) else {
            continue; // not under the old folder — leave it where it is
        };
        let new_thumb = clip
            .thumb_path
            .as_deref()
            .and_then(|p| relocate(p, old_root, new_root));
        let new_film = clip
            .filmstrip_path
            .as_deref()
            .and_then(|p| relocate(p, old_root, new_root));

        // Move the video first; only repoint the row once it has landed so a
        // failure leaves the clip fully intact at its old location.
        if let Err(e) = move_file(Path::new(&clip.path), &new_path) {
            tracing::warn!("migrate clip {}: {e}", clip.path);
            continue;
        }
        if let (Some(src), Some(dst)) = (clip.thumb_path.as_deref(), new_thumb.as_ref()) {
            let _ = move_file(Path::new(src), dst);
        }
        if let (Some(src), Some(dst)) = (clip.filmstrip_path.as_deref(), new_film.as_ref()) {
            let _ = move_file(Path::new(src), dst);
        }

        let new_path = new_path.to_string_lossy().to_string();
        let new_thumb = new_thumb.map(|p| p.to_string_lossy().to_string());
        let new_film = new_film.map(|p| p.to_string_lossy().to_string());
        if let Ok(lib) = library.0.lock() {
            if let Err(e) =
                lib.update_paths(clip.id, &new_path, new_thumb.as_deref(), new_film.as_deref())
            {
                tracing::warn!("migrate db update for clip {}: {e}", clip.id);
            }
        }
        moved += 1;
    }

    if moved > 0 {
        tracing::info!("migrated {moved} clip(s) to {}", new_root.display());
    }
    moved
}

/// Count existing library clips stored under the folder `dir` resolves to (null →
/// default `<Videos>/Hako`). Drives the "move N clips?" prompt the UI shows when
/// the user changes the clip folder; returns 0 when nothing would move.
#[tauri::command]
pub fn count_clips_in(app: AppHandle, dir: Option<String>) -> Result<u32, String> {
    let root = resolve_clip_dir(&app, dir.as_deref())?;
    Ok(count_clips_under(&app, &root))
}

/// Move existing clips from the `from` folder to the `to` folder and repoint the
/// library (both null-default-resolved). Opt-in: the UI calls this only after the
/// user confirms the "move existing clips?" prompt. Returns the count moved; a
/// no-op (0) when the two resolve to the same folder.
#[tauri::command]
pub async fn migrate_clips_to(
    app: AppHandle,
    from: Option<String>,
    to: Option<String>,
) -> Result<u32, String> {
    let old_root = resolve_clip_dir(&app, from.as_deref())?;
    let new_root = resolve_clip_dir(&app, to.as_deref())?;
    if old_root == new_root {
        return Ok(0);
    }
    // Off the UI thread — moving many large files (or copying across drives) can
    // take a while.
    tauri::async_runtime::spawn_blocking(move || migrate_clips(&app, &old_root, &new_root))
        .await
        .map_err(|e| format!("migration task failed: {e}"))
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
pub(crate) fn generate_thumbnail(app: &AppHandle, video: &Path) -> Option<String> {
    let dir = clip_dir(app).ok()?.join("thumbs");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = video.file_stem()?.to_str()?;
    let out = dir.join(format!("{stem}.jpg"));
    // Grid cards render ~340–600px wide; 400px keeps thumbnails sharp there while
    // cutting decode work ~30% vs the old 480px (less to rasterize on the scroll
    // path). Filmstrip tiles are sized separately, above.
    match crate::library::thumbs::extract_thumbnail(video, &out, 400) {
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
pub(crate) fn generate_filmstrip(app: &AppHandle, video: &Path, duration_secs: f64) -> Option<String> {
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
