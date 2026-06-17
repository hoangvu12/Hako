//! Windows Graphics Capture: GraphicsCaptureItem from a window HWND,
//! free-threaded frame pool, FrameArrived → bounded hand-off → encode thread.
//!
//! WGC only (no injection) — Vanguard-safe. Caps to target FPS and drops on
//! backpressure; never blocks the FrameArrived thread.
//!
//! Pipeline: the WGC `FrameArrived` callback copies the captured
//! BGRA texture into a pooled staging texture (a GPU→GPU copy — no CPU readback,
//! and it must happen before WGC recycles the frame's buffer), then hands that
//! staging texture to a dedicated **encode thread** over a bounded channel. The
//! encode thread owns the `Converter` (BGRA→NV12), the `Encoder` (`h264_qsv`),
//! and a small NV12 texture ring. The callback only copies + sends; if no
//! staging texture is free it drops the frame (encoder backpressure).

#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::core::{BOOL, IInspectable, Interface, Result as WinResult};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BOX, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_TYPELESS, DXGI_FORMAT_B8G8R8X8_UNORM,
    DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible,
};

use crate::core::audio::{self, AudioCapture, AudioMeta};
use crate::settings::AudioConfig;
use crate::core::buffer::{AudioRing, BufferStats, PacketRing};
use crate::core::clock::{MasterClock, TICKS_PER_SECOND};
use crate::core::convert::Converter;
use crate::core::device;
use crate::core::encode::{EncodeSettings, EncodedPacket, Encoder};
use crate::core::hook::{HookCapture, RunningHook};
use crate::core::mux::{self, AudioClip, ClipMeta};
use crate::core::session::SessionWriter;
use crate::events;

/// Number of BGRA staging textures shared between the capture callback and the
/// encode thread. Also bounds in-flight frames (backpressure: callback drops
/// when none are free). Small — we only need to cover channel + encoder latency.
const STAGING_POOL: usize = 4;
/// NV12 textures the encode thread cycles through. Must exceed how many surfaces
/// the encoder holds asynchronously (`async_depth` ≈ 1–2) so a reused texture is
/// never still in flight.
const NV12_RING: usize = 6;
/// WGC frame-pool depth — frames WGC keeps in flight before recycling. 2 is the
/// practical minimum; 3–4 gives headroom so a brief encode-thread stall doesn't
/// make WGC drop *real* frames (it only costs a few capture-sized BGRA textures).
const WGC_POOL_FRAMES: i32 = 4;

/// A capturable top-level window (for the UI picker).
#[derive(Debug, Clone, Serialize)]
pub struct WindowTarget {
    /// HWND as an integer (passed back to `start_capture`).
    pub hwnd: i64,
    pub title: String,
}

/// Live capture + encode throughput, emitted as the `capture-stats` event.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureStats {
    /// Handed-off (captured + copied) frames per second, after the FPS cap.
    pub fps: f64,
    /// Total frames handed off to the encode thread since start.
    pub frames: u64,
    /// Total frames WGC delivered (before the cap) since start.
    pub arrived: u64,
    pub width: u32,
    pub height: u32,
    pub target_fps: u32,
    /// Compressed frames per second coming out of the encoder.
    pub encoded_fps: f64,
    /// Total compressed packets produced since start.
    pub encoded_frames: u64,
    /// Encoded bitrate (kbps) over the last sample window.
    pub encoded_kbps: f64,
}

#[derive(Default)]
struct Shared {
    arrived: AtomicU64,
    handed: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    /// SystemRelativeTime (100 ns units) of the last handed frame, for the cap.
    last_handed_time: AtomicI64,
    /// Compressed packets produced by the encode thread.
    enc_packets: AtomicU64,
    /// Total compressed bytes produced (for the bitrate readout).
    enc_bytes: AtomicU64,
}

/// Result of a successful clip save (used to build the library row).
pub struct SavedClip {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub duration_secs: f64,
}

/// The compressed RAM ring plus the metadata a clip needs to be muxed.
///
/// Shared (`Arc`) between the encode thread — which fills the ring and publishes
/// `meta` once the encoder is built — and the save path (hotkey / command),
/// which slices the ring and stream-copies it to MP4 (`mux.rs`). The encoder's
/// dimensions/fps/extradata aren't known until the encode thread opens it, so
/// `meta` is a `OnceLock` set exactly once.
/// One output audio track's RAM ring + AAC metadata. Track 0 is always the
/// master "All Audio" mix (back-compat + clip playback); tracks 1..N are the
/// per-source stems when "Separate audio tracks" is on. Filled by the audio
/// thread via [`ClipBuffer::push_audio`]; sliced per track on save.
pub struct AudioTrack {
    /// Track label, written as the MP4 stream `title` (Phase 4).
    pub name: String,
    ring: Mutex<AudioRing>,
    meta: OnceLock<AudioMeta>,
}

pub struct ClipBuffer {
    ring: Mutex<PacketRing>,
    meta: OnceLock<ClipMeta>,
    /// Per-track compressed AAC rings (filled by the audio thread). Empty when
    /// audio is disabled; track 0 is the master mix. Layout (count + names) is
    /// fixed at construction so the muxer/session can declare the streams.
    audio_tracks: Vec<AudioTrack>,
    /// Wall-clock tick (100 ns) of the first captured video frame — the anchor
    /// that ties video PTS (1/fps units) to absolute audio PTS for muxing.
    video_base: OnceLock<i64>,
    /// Active Mode-B session writer (Valorant full-match recording). When
    /// installed, every pushed packet is also teed to it; `None` otherwise. The
    /// orchestrator installs/takes it on match start/end (`valorant::orchestrator`).
    session: Mutex<Option<Arc<SessionWriter>>>,
}

impl ClipBuffer {
    /// New clip buffer. `audio_track_names` fixes the audio-track layout (one
    /// AAC ring per name, track 0 = master); empty ⇒ video-only.
    fn new(fps: u32, retention_secs: u32, audio_track_names: Vec<String>) -> Arc<Self> {
        let audio_tracks = audio_track_names
            .into_iter()
            .map(|name| AudioTrack {
                name,
                ring: Mutex::new(AudioRing::new(retention_secs)),
                meta: OnceLock::new(),
            })
            .collect();
        Arc::new(ClipBuffer {
            ring: Mutex::new(PacketRing::new(fps, retention_secs)),
            meta: OnceLock::new(),
            audio_tracks,
            video_base: OnceLock::new(),
            session: Mutex::new(None),
        })
    }

    /// The currently-installed session writer, if any (clones the `Arc`).
    fn active_session(&self) -> Option<Arc<SessionWriter>> {
        self.session.lock().ok().and_then(|g| g.clone())
    }

    /// Append a freshly encoded packet (called on the encode thread). Tees to the
    /// Mode-B session writer when a Valorant match is recording.
    fn push(&self, pkt: EncodedPacket) {
        if let Some(session) = self.active_session() {
            // Reconstruct the packet's wall-clock tick from the shared video-base
            // anchor + its PTS — the same linear map the save path uses to place
            // audio against video. Both are known by the time packets flow.
            if let (Some(&base), Some(meta)) = (self.video_base.get(), self.meta.get()) {
                let fps = meta.fps.max(1) as i64;
                let wall = base + pkt.pts * TICKS_PER_SECOND / fps;
                session.push(&pkt, wall);
            }
        }
        if let Ok(mut r) = self.ring.lock() {
            r.push(pkt);
        }
    }

    /// Append a freshly encoded AAC packet for output track `track_idx` (called
    /// on the audio thread). Every track is teed to the Mode-B session writer (so
    /// Valorant auto-clips are multi-track too), routed to the matching session
    /// stream by index, and also stored in its own per-track ring for the save
    /// path.
    pub fn push_audio(&self, track_idx: usize, pkt: EncodedPacket) {
        if let Some(session) = self.active_session() {
            session.push_audio(track_idx, &pkt);
        }
        if let Some(track) = self.audio_tracks.get(track_idx) {
            if let Ok(mut r) = track.ring.lock() {
                r.push(pkt);
            }
        }
    }

    /// Publish a Mode-B session writer so subsequent packets are teed to it.
    pub fn install_session(&self, writer: Arc<SessionWriter>) {
        if let Ok(mut g) = self.session.lock() {
            *g = Some(writer);
        }
    }

    /// Detach the active session writer (match ended) and hand it back to the
    /// caller to `finish()`. `None` if none was installed.
    pub fn take_session(&self) -> Option<Arc<SessionWriter>> {
        self.session.lock().ok().and_then(|mut g| g.take())
    }

    /// The muxing metadata (dimensions + avcC), once the encoder has opened.
    pub fn clip_meta(&self) -> Option<ClipMeta> {
        self.meta.get().cloned()
    }

    /// The **master** track's AAC stream metadata, once its encoder has opened.
    pub fn audio_meta(&self) -> Option<AudioMeta> {
        self.audio_tracks
            .first()
            .and_then(|t| t.meta.get().cloned())
    }

    /// Every output track's `(name, AudioMeta)` whose encoder has published its
    /// metadata, in track order (0 = master). Used to declare all of the Mode-B
    /// session writer's audio streams up front. Tracks without published meta are
    /// skipped; in practice every encoder opens at audio-thread start, before a
    /// match installs the session writer, so all planned tracks are present.
    pub fn audio_track_metas(&self) -> Vec<(String, AudioMeta)> {
        self.audio_tracks
            .iter()
            .filter_map(|t| t.meta.get().map(|m| (t.name.clone(), m.clone())))
            .collect()
    }

    /// Number of audio tracks (0 ⇒ video-only).
    pub fn audio_track_count(&self) -> usize {
        self.audio_tracks.len()
    }

    /// Publish the muxing metadata (once, when the encoder is ready).
    fn set_meta(&self, meta: ClipMeta) {
        let _ = self.meta.set(meta);
    }

    /// Publish output track `idx`'s AAC stream metadata (once, when its encoder
    /// opens). No-op for an out-of-range index.
    pub fn set_audio_track_meta(&self, idx: usize, meta: AudioMeta) {
        if let Some(track) = self.audio_tracks.get(idx) {
            let _ = track.meta.set(meta);
        }
    }

    /// Record the wall-clock tick of the first video frame (once).
    fn set_video_base(&self, ticks: i64) {
        let _ = self.video_base.set(ticks);
    }

    /// Slice the last `secs` (IDR-aligned) and stream-copy to an MP4 at `out`.
    /// Returns clip metadata on success. Errors if the encoder isn't ready or the
    /// ring is empty (e.g. save pressed immediately after start).
    pub fn save_last(&self, secs: u32, out: &Path) -> std::result::Result<SavedClip, String> {
        let meta = self
            .meta
            .get()
            .ok_or("encoder not initialized yet — try again in a moment")?;
        let packets = self
            .ring
            .lock()
            .map_err(|_| "clip buffer poisoned")?
            .slice_last(secs);
        if packets.is_empty() {
            return Err("buffer is empty — nothing to save".into());
        }
        // Duration tracks wall-clock via the PTS span, NOT frame_count/fps:
        // capture runs below target fps under the DWM composition cap, so PTS
        // (1/fps units, derived from SystemRelativeTime) is the real
        // timeline — frame_count/fps would report e.g. 12s for 30s of footage.
        let (lo, hi) = packets
            .iter()
            .fold((i64::MAX, i64::MIN), |(lo, hi), p| (lo.min(p.pts), hi.max(p.pts)));
        let span_pts = (hi - lo).max(0) + 1; // +1 for the last frame's own duration

        // Audio: slice EVERY output track's AAC ring over the same wall-clock
        // window. The video clip starts at the wall-clock tick of its first
        // packet, derived from the shared video base anchor (PTS is in 1/fps
        // units off it). Track 0 (master mix) comes first; with "Separate audio
        // tracks" on, tracks 1..N are the per-source stems — each becomes its own
        // named MP4 audio stream via `write_clip`.
        let fps = meta.fps.max(1) as i64;
        let video_base = self.video_base.get().copied();
        let mut track_slices: Vec<(usize, Vec<EncodedPacket>)> = Vec::new();
        if let Some(base) = video_base {
            let start_ticks = base + lo * TICKS_PER_SECOND / fps;
            let end_ticks = base + hi * TICKS_PER_SECOND / fps;
            for (i, track) in self.audio_tracks.iter().enumerate() {
                if track.meta.get().is_none() {
                    continue; // encoder for this stem never opened — skip
                }
                let pkts = track
                    .ring
                    .lock()
                    .ok()
                    .map(|r| r.slice_ticks(start_ticks, end_ticks))
                    .unwrap_or_default();
                if !pkts.is_empty() {
                    track_slices.push((i, pkts));
                }
            }
        }
        let clip_start_ticks = video_base
            .map(|base| base + lo * TICKS_PER_SECOND / fps)
            .unwrap_or(0);
        let audio: Vec<AudioClip> = track_slices
            .iter()
            .filter_map(|(i, pkts)| {
                let track = &self.audio_tracks[*i];
                track.meta.get().map(|m| AudioClip {
                    meta: m,
                    name: track.name.as_str(),
                    packets: pkts,
                    clip_start_ticks,
                })
            })
            .collect();

        mux::write_clip(out, meta, &packets, &audio)?;
        Ok(SavedClip {
            path: out.to_path_buf(),
            width: meta.width,
            height: meta.height,
            duration_secs: span_pts as f64 / meta.fps.max(1) as f64,
        })
    }

    /// Current ring health (for `buffer-stats` / dashboard).
    pub fn stats(&self) -> Option<BufferStats> {
        self.ring.lock().ok().map(|r| r.stats())
    }
}

/// Handle to a running capture; drop or call `stop()` to tear it down.
pub struct RunningCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Shared clip buffer for the hotkey/command save path.
    clip: Arc<ClipBuffer>,
}

impl RunningCapture {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// The shared clip buffer — clone out to save without holding capture state.
    pub fn clip(&self) -> Arc<ClipBuffer> {
        self.clip.clone()
    }
}

impl Drop for RunningCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Enumerate visible top-level windows with a title (for the capture picker).
pub fn list_windows() -> Vec<WindowTarget> {
    let mut out: Vec<WindowTarget> = Vec::new();
    // SAFETY: `out` outlives the EnumWindows call; the callback only touches it.
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut out as *mut Vec<WindowTarget> as isize),
        );
    }
    out
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut Vec<WindowTarget>);
    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if !title.is_empty() {
                    out.push(WindowTarget {
                        hwnd: hwnd.0 as i64,
                        title,
                    });
                }
            }
        }
    }
    BOOL(1) // continue enumeration
}

/// Find the live VALORANT **game** window (the Unreal client, not the Riot
/// launcher), used to auto-start capture when the game launches — the way Medal
/// detects the game process. Matches the game window's exact title; returns its
/// HWND or `None` if the game isn't running.
pub fn find_valorant_window() -> Option<i64> {
    let mut found: i64 = 0;
    // SAFETY: `found` outlives the EnumWindows call; the callback only writes it.
    unsafe {
        let _ = EnumWindows(
            Some(find_valorant_proc),
            LPARAM(&mut found as *mut i64 as isize),
        );
    }
    (found != 0).then_some(found)
}

/// The process id that owns `hwnd_raw` (for the `specific_apps` "Game Audio"
/// source — the capture target's PID). `None` for an invalid window.
pub fn pid_for_hwnd(hwnd_raw: i64) -> Option<u32> {
    let mut pid: u32 = 0;
    // SAFETY: GetWindowThreadProcessId just reads window ownership; a stale HWND
    // yields pid 0.
    unsafe {
        GetWindowThreadProcessId(HWND(hwnd_raw as *mut c_void), Some(&mut pid));
    }
    (pid != 0).then_some(pid)
}

/// Whether a window is minimized (iconic). A minimized game — common with
/// exclusive fullscreen when alt-tabbed — usually stops presenting frames, so
/// the graphics hook can't capture it; the auto-capture skips it until it's back
/// on screen rather than re-injecting into a non-rendering process.
pub fn is_window_minimized(hwnd: i64) -> bool {
    // SAFETY: IsIconic just reads window state; a stale/invalid HWND returns false.
    unsafe { IsIconic(HWND(hwnd as *mut c_void)).as_bool() }
}

unsafe extern "system" fn find_valorant_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = &mut *(lparam.0 as *mut i64);
    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                // The game window is titled exactly "VALORANT"; the Riot launcher
                // is "Riot Client", and tab titles like "VALORANT - YouTube" won't
                // match the trimmed-exact compare.
                if title.trim().eq_ignore_ascii_case("VALORANT") {
                    *out = hwnd.0 as i64;
                    return BOOL(0); // stop enumeration
                }
            }
        }
    }
    BOOL(1)
}

/// Cross-thread hand-off plumbing given to the FrameArrived callback.
///
/// The callback pops a free staging texture, copies the captured BGRA frame into
/// it (GPU→GPU), and sends it to the encode thread. `context` is the shared
/// immediate context (multithread-protected by the encoder), so using it from
/// the callback thread alongside the encode thread is safe.
struct Handoff {
    context: ID3D11DeviceContext,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    filled_tx: SyncSender<(ID3D11Texture2D, i64)>,
    /// Even, NV12-compatible capture dimensions; frames must match (or exceed,
    /// then crop to) these. Mismatches are dropped (resize handling is a TODO).
    width: u32,
    height: u32,
}

/// Start capturing the given window, encoding it via QSV on the shared device.
///
/// Sets up the encode thread + WGC on a dedicated MTA thread and reports
/// setup success/failure synchronously; on success the thread emits
/// `capture-stats` until stopped.
pub fn start(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    buffer_secs: u32,
    audio: AudioConfig,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    let target_fps = target_fps.clamp(1, 480);
    // The capture target's PID feeds the `specific_apps` "Game Audio" source; the
    // planned track names fix the clip buffer's audio-track layout up front.
    let game_pid = pid_for_hwnd(hwnd_raw);
    let track_names = audio::planned_track_names(&audio, game_pid);
    // Created here (fps is known) so the handle survives on RunningCapture; the
    // encode thread fills its ring + publishes `meta` (dimensions known later).
    let clip = ClipBuffer::new(target_fps, buffer_secs.clamp(5, 600), track_names);

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let thread = {
        let stop = stop.clone();
        let shared = shared.clone();
        let clip = clip.clone();
        std::thread::Builder::new()
            .name("hako-capture".into())
            .spawn(move || {
                capture_thread(
                    app, hwnd_raw, target_fps, adapter_index, audio, game_pid, stop, shared, clip,
                    enc_cfg, ready_tx,
                )
            })
            .map_err(|e| format!("failed to spawn capture thread: {e}"))?
    };

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(RunningCapture {
            stop,
            thread: Some(thread),
            clip,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("capture thread exited before signalling readiness".into()),
    }
}

fn capture_thread(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio: AudioConfig,
    game_pid: Option<u32>,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    // WGC objects are agile, but the thread still needs COM initialized (MTA).
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    match run_pipeline(
        &app,
        hwnd_raw,
        target_fps,
        adapter_index,
        audio,
        game_pid,
        &stop,
        &shared,
        clip,
        enc_cfg,
    ) {
        Err(e) => {
            let _ = ready_tx.send(Err(e));
        }
        Ok(mut running) => {
            let _ = ready_tx.send(Ok(()));
            emit_loop(&app, target_fps, &stop, &shared);
            running.teardown();
        }
    }

    unsafe {
        RoUninitialize();
    }
}

/// Owns the live capture + encode resources for one session; `teardown` stops
/// the encode thread and releases the WGC objects on the capture thread.
struct RunningPipeline {
    pool: Direct3D11CaptureFramePool,
    session: windows::Graphics::Capture::GraphicsCaptureSession,
    token: i64,
    encode_thread: Option<JoinHandle<()>>,
    /// Desktop+mic audio capture, when enabled. Dropped/stopped on teardown.
    audio: Option<AudioCapture>,
}

impl RunningPipeline {
    fn teardown(&mut self) {
        // Stop audio first (its own thread + WASAPI clients) so it isn't pushing
        // into the clip buffer while we tear the rest down.
        if let Some(mut a) = self.audio.take() {
            a.stop();
        }
        // Removing the handler + closing drops the FrameArrived closure, which
        // owns the only `filled_tx`; the encode thread's `recv` then ends.
        let _ = self.pool.RemoveFrameArrived(self.token);
        let _ = self.session.Close();
        let _ = self.pool.Close();
        if let Some(t) = self.encode_thread.take() {
            let _ = t.join();
        }
    }
}

/// Build the whole pipeline: shared device → encode thread → WGC capture.
fn run_pipeline(
    _app: &AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio: AudioConfig,
    game_pid: Option<u32>,
    _stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningPipeline, String> {
    // Capture on the chosen adapter, else the display-owning one.
    let gpus = device::enumerate_gpus().map_err(|e| format!("enumerate gpus: {e:?}"))?;
    let index = match adapter_index {
        Some(i) => Some(i),
        None => device::default_capture_index(&gpus),
    };
    // Encode adapter == capture adapter on the single-device fast path (Phase 1:
    // dual-device plumbing without cross-adapter yet). The encoder takes the
    // ENCODE adapter's vendor; here they coincide. Logged so a cross-adapter setup
    // is visible once later phases let `encode_idx` diverge from `capture_idx`.
    let vendor = index
        .map(|i| device::vendor_at(&gpus, i))
        .unwrap_or(device::Vendor::Other);
    let adapter = match index {
        Some(i) => Some(device::adapter_at(i).map_err(|e| format!("adapter_at({i}): {e:?}"))?),
        None => None,
    };
    let (d3d_device, context, _fl) =
        device::create_device(adapter.as_ref()).map_err(|e| format!("create device: {e:?}"))?;
    tracing::info!(
        capture_adapter = ?index,
        encode_adapter = ?index,
        cross_adapter = false,
        encode_vendor = vendor.label(),
        "wgc capture: resolved adapters (single-device fast path)"
    );

    let hwnd = HWND(hwnd_raw as *mut c_void);
    let item = create_item_for_window(hwnd).map_err(|e| format!("capture item: {e:?}"))?;
    let size = item.Size().map_err(|e| format!("item size: {e:?}"))?;
    let width = (size.Width.max(0) as u32) & !1;
    let height = (size.Height.max(0) as u32) & !1;
    if width < 2 || height < 2 {
        return Err(format!("window has no capturable size ({width}x{height})"));
    }

    // Staging pool (BGRA) shared with the callback.
    let mut pool_vec = Vec::with_capacity(STAGING_POOL);
    for _ in 0..STAGING_POOL {
        pool_vec.push(
            create_bgra_staging(&d3d_device, width, height)
                .map_err(|e| format!("staging texture: {e:?}"))?,
        );
    }
    let free_pool = Arc::new(Mutex::new(pool_vec));
    let (filled_tx, filled_rx) = sync_channel::<(ID3D11Texture2D, i64)>(STAGING_POOL);

    // Encode thread: build Converter + Encoder on it (raw FFmpeg ptrs aren't
    // Send), report readiness, then convert→encode each handed frame into the
    // shared clip buffer (Mode A).
    let (enc_ready_tx, enc_ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let encode_thread = {
        let capture_device = d3d_device.clone();
        let capture_context = context.clone();
        // Single-device fast path: the encode device IS the capture device, so the
        // converter's NV12 textures feed the encoder with no cross-adapter copy.
        let encode_device = d3d_device.clone();
        let encode_context = context.clone();
        let shared = shared.clone();
        let free_pool = free_pool.clone();
        let clip = clip.clone();
        std::thread::Builder::new()
            .name("hako-encode".into())
            .spawn(move || {
                encode_thread(
                    capture_device, capture_context, encode_device, encode_context, vendor, width,
                    height, target_fps, enc_cfg, filled_rx, free_pool, shared, clip, enc_ready_tx,
                )
            })
            .map_err(|e| format!("spawn encode thread: {e}"))?
    };
    match enc_ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = encode_thread.join();
            return Err(e);
        }
        Err(_) => {
            let _ = encode_thread.join();
            return Err("encode thread exited before signalling readiness".into());
        }
    }

    let handoff = Handoff {
        context: context.clone(),
        free_pool,
        filled_tx,
        width,
        height,
    };

    // Start audio capture (best-effort) once the clip buffer exists; it pushes
    // each output track's AAC into the same ClipBuffer the save path reads. A
    // None means no usable device — the clip is simply video-only. Skip entirely
    // when the config has no enabled source (no audio tracks planned).
    let audio = if clip.audio_track_count() > 0 {
        match AudioCapture::start(clip.clone(), audio, game_pid) {
            Some(a) => Some(a),
            None => {
                tracing::warn!("audio capture requested but could not start; recording video only");
                None
            }
        }
    } else {
        None
    };

    match setup_wgc(&d3d_device, item, target_fps, shared, Some(handoff)) {
        Ok((pool, session, token)) => Ok(RunningPipeline {
            pool,
            session,
            token,
            encode_thread: Some(encode_thread),
            audio,
        }),
        Err(e) => {
            // Dropping the handoff (held by the Err path) drops filled_tx → the
            // encode thread ends; join it before returning.
            let _ = encode_thread.join();
            Err(format!("WGC setup: {e:?}"))
        }
    }
}

// ===========================================================================
// Game-capture (graphics-hook injection) path — opt-in, beats the DWM cap.
// ===========================================================================

/// How long to wait for the injected hook to deliver its first frame before
/// giving up. If the game isn't presenting (minimized) or the DLL was blocked by
/// anti-cheat, we fail here rather than hang the start command.
const HOOK_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(12);

/// Start the **injection** capture path for a window: inject the OBS-derived
/// `graphics-hook` into the game, pull the shared backbuffer at the game's real
/// render rate, and run it through the same `Converter` → `Encoder` → clip-buffer
/// pipeline as WGC. Returns the same [`RunningCapture`] handle as [`start`] so the
/// save path and capture state are identical.
///
/// ⚠️ Injects into the target process. Only call this for users who opted into the
/// game-capture mode behind the ban-risk warning (Settings `capture_mode = hook`).
pub fn start_hook(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    buffer_secs: u32,
    audio: AudioConfig,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    let target_fps = target_fps.clamp(1, 480);
    let game_pid = pid_for_hwnd(hwnd_raw);
    let track_names = audio::planned_track_names(&audio, game_pid);
    let clip = ClipBuffer::new(target_fps, buffer_secs.clamp(5, 600), track_names);

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let thread = {
        let stop = stop.clone();
        let shared = shared.clone();
        let clip = clip.clone();
        std::thread::Builder::new()
            .name("hako-capture-hook".into())
            .spawn(move || {
                hook_capture_thread(
                    app, hwnd_raw, target_fps, adapter_index, audio, game_pid, stop, shared, clip,
                    enc_cfg, ready_tx,
                )
            })
            .map_err(|e| format!("failed to spawn hook capture thread: {e}"))?
    };

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(RunningCapture {
            stop,
            thread: Some(thread),
            clip,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("hook capture thread exited before signalling readiness".into()),
    }
}

fn hook_capture_thread(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio: AudioConfig,
    game_pid: Option<u32>,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    match run_hook_pipeline(
        hwnd_raw,
        target_fps,
        adapter_index,
        audio,
        game_pid,
        &stop,
        &shared,
        clip,
        enc_cfg,
    ) {
        Err(e) => {
            let _ = ready_tx.send(Err(e));
        }
        Ok(mut running) => {
            let _ = ready_tx.send(Ok(()));
            emit_loop(&app, target_fps, &stop, &shared);
            running.teardown();
        }
    }
}

/// Live hook-capture resources for one session; `teardown` stops the frame-source
/// loop (which drops the hook → DLL self-destructs) and then the encode thread.
struct RunningHookPipeline {
    source_stop: Arc<AtomicBool>,
    source_thread: Option<JoinHandle<()>>,
    encode_thread: Option<JoinHandle<()>>,
    audio: Option<AudioCapture>,
}

impl RunningHookPipeline {
    fn teardown(&mut self) {
        if let Some(mut a) = self.audio.take() {
            a.stop();
        }
        // Stopping the source loop drops its `filled_tx` and the `RunningHook`
        // (Stop event + keepalive release), so the encode thread's recv ends.
        self.source_stop.store(true, Ordering::Release);
        if let Some(t) = self.source_thread.take() {
            let _ = t.join();
        }
        if let Some(t) = self.encode_thread.take() {
            let _ = t.join();
        }
    }
}

fn run_hook_pipeline(
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio: AudioConfig,
    game_pid: Option<u32>,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningHookPipeline, String> {
    // Adapter selection DIFFERS from WGC. The hook copies the game's backbuffer
    // into a *legacy* shared texture on the GPU the game RENDERS on (the dGPU on
    // a hybrid/Optimus laptop), and a legacy shared handle can only be reopened
    // via `OpenSharedResource` on that same adapter. So default to the preferred
    // (highest-VRAM = discrete) GPU, NOT the display-owning one WGC uses — on an
    // Optimus laptop the internal panel is driven by the iGPU, which would make
    // the shared-texture open fail. An explicit `adapter_index` still wins.
    let gpus = device::enumerate_gpus().map_err(|e| format!("enumerate gpus: {e:?}"))?;
    let index = adapter_index
        .or_else(|| gpus.iter().find(|g| g.preferred).map(|g| g.index))
        .or_else(|| device::default_capture_index(&gpus));
    // Encode == capture on this (single-device) path too; the encoder takes the
    // encode adapter's vendor (here, the same adapter the hook renders/copies on).
    let vendor = index
        .map(|i| device::vendor_at(&gpus, i))
        .unwrap_or(device::Vendor::Other);
    let adapter = match index {
        Some(i) => Some(device::adapter_at(i).map_err(|e| format!("adapter_at({i}): {e:?}"))?),
        None => None,
    };
    let (d3d_device, context, _fl) =
        device::create_device(adapter.as_ref()).map_err(|e| format!("create device: {e:?}"))?;
    tracing::info!(
        capture_adapter = ?index,
        encode_adapter = ?index,
        cross_adapter = false,
        encode_vendor = vendor.label(),
        "hook capture: resolved adapters (single-device fast path)"
    );

    // Inject + bring the hook up (steps 1–9). Frames flow after this.
    let hwnd = HWND(hwnd_raw as *mut c_void);
    let mut hook = HookCapture::start(hwnd, target_fps)?;

    // Discover the real backbuffer texture (and its format/size) from the first
    // delivered frame. We size the staging pool + converter/encoder to match.
    let deadline = Instant::now() + HOOK_FIRST_FRAME_TIMEOUT;
    let first_desc = loop {
        if stop.load(Ordering::Acquire) {
            return Err("capture stopped before the hook produced a frame".into());
        }
        if let Some((tex, _ts)) = hook.acquire(&d3d_device)? {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { tex.GetDesc(&mut desc) };
            break desc;
        }
        if Instant::now() >= deadline {
            return Err(
                "hook injected but delivered no frame in time — the game may be \
                 minimized, or the hook was blocked by anti-cheat (Vanguard)"
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    };

    let width = first_desc.Width & !1;
    let height = first_desc.Height & !1;
    if width < 2 || height < 2 {
        return Err(format!("hook reported an unusable size ({width}x{height})"));
    }
    tracing::info!(
        format = first_desc.Format.0,
        width,
        height,
        "hook: first backbuffer frame — format/size (note: non-BGRA/RGBA UNORM \
         formats may be rejected by the BGRA→NV12 VideoProcessor)"
    );

    // Staging pool in the backbuffer's own format (the GPU→GPU copy needs source
    // and destination formats to match; the VideoProcessor input view then reads
    // whatever RGB format it is — BGRA or RGBA).
    let mut pool_vec = Vec::with_capacity(STAGING_POOL);
    for _ in 0..STAGING_POOL {
        pool_vec.push(
            create_staging_like(&d3d_device, &first_desc, width, height)
                .map_err(|e| format!("staging texture: {e:?}"))?,
        );
    }
    let free_pool = Arc::new(Mutex::new(pool_vec));
    let (filled_tx, filled_rx) = sync_channel::<(ID3D11Texture2D, i64)>(STAGING_POOL);

    // Reuse the exact same encode thread as the WGC path.
    let (enc_ready_tx, enc_ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let encode_thread = {
        let capture_device = d3d_device.clone();
        let capture_context = context.clone();
        let encode_device = d3d_device.clone();
        let encode_context = context.clone();
        let shared = shared.clone();
        let free_pool = free_pool.clone();
        let clip = clip.clone();
        std::thread::Builder::new()
            .name("hako-encode".into())
            .spawn(move || {
                encode_thread(
                    capture_device, capture_context, encode_device, encode_context, vendor, width,
                    height, target_fps, enc_cfg, filled_rx, free_pool, shared, clip, enc_ready_tx,
                )
            })
            .map_err(|e| format!("spawn encode thread: {e}"))?
    };
    match enc_ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            let _ = encode_thread.join();
            return Err(e);
        }
        Err(_) => {
            let _ = encode_thread.join();
            return Err("encode thread exited before signalling readiness".into());
        }
    }

    // Frame-source loop: poll the hook, copy each shared backbuffer into a free
    // staging texture, and hand it to the encode thread — the same
    // `(ID3D11Texture2D, ts)` shape the WGC callback produces.
    let source_stop = stop.clone();
    let source_thread = {
        let shared = shared.clone();
        let free_pool = free_pool.clone();
        let device = d3d_device.clone();
        let context = context.clone();
        std::thread::Builder::new()
            .name("hako-hook-source".into())
            .spawn(move || {
                hook_source_loop(
                    hook, device, context, width, height, target_fps, filled_tx, free_pool, shared,
                    source_stop,
                )
            })
            .map_err(|e| format!("spawn hook source thread: {e}"))?
    };

    let audio = if clip.audio_track_count() > 0 {
        match AudioCapture::start(clip.clone(), audio, game_pid) {
            Some(a) => Some(a),
            None => {
                tracing::warn!("audio capture requested but could not start; recording video only");
                None
            }
        }
    } else {
        None
    };

    Ok(RunningHookPipeline {
        source_stop: stop.clone(),
        source_thread: Some(source_thread),
        encode_thread: Some(encode_thread),
        audio,
    })
}

/// The hook frame-source loop (analog of the WGC `FrameArrived` callback): pull a
/// shared backbuffer, copy its even sub-rect into a free staging texture, and send
/// it on. Owns the `RunningHook` so dropping at loop-end tears the hook down.
fn hook_source_loop(
    mut hook: RunningHook,
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    width: u32,
    height: u32,
    fps: u32,
    filled_tx: SyncSender<(ID3D11Texture2D, i64)>,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    let mut warned_copy = false;
    // Per-second throughput report straight to the log, so we can measure frame
    // delivery from the file while the game stays focused (the live UI stats are
    // unreliable here — alt-tabbing to read them makes the game throttle/stop
    // presenting, which is exactly the rate the hook can capture).
    let mut last_report = Instant::now();
    let (mut acq_window, mut sent_window) = (0u64, 0u64);
    // The hook gives no per-frame signal on the shtex path — it just keeps
    // overwriting the shared texture each present. So we *pace* ourselves to the
    // target fps and sample the latest backbuffer each tick (the encode thread's
    // constant-rate gap-fill smooths over any presents we sampled twice).
    let frame_interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next_tick = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if last_report.elapsed() >= Duration::from_secs(1) {
            tracing::info!(
                sampled = acq_window,
                handed = sent_window,
                "hook source: frames sampled in last ~1s"
            );
            acq_window = 0;
            sent_window = 0;
            last_report = Instant::now();
        }

        // Pace to the target frame interval.
        let now = Instant::now();
        if next_tick > now {
            std::thread::sleep(next_tick - now);
        } else {
            // Fell behind (or first iteration) — resync to avoid a burst.
            next_tick = now;
        }
        next_tick += frame_interval;

        let frame = match hook.acquire(&_device) {
            Ok(Some(f)) => {
                acq_window += 1;
                f
            }
            Ok(None) => {
                // Capture not initialized yet — wait for the hook's first present.
                continue;
            }
            Err(e) => {
                tracing::warn!("hook acquire failed, stopping source: {e}");
                break;
            }
        };
        let (shared_tex, ts) = frame;
        shared.arrived.fetch_add(1, Ordering::Relaxed);
        shared.width.store(width, Ordering::Relaxed);
        shared.height.store(height, Ordering::Relaxed);

        // Grab a free staging texture; none → encoder backpressure, drop.
        let staging = match free_pool.lock() {
            Ok(mut p) => p.pop(),
            Err(_) => None,
        };
        let Some(staging) = staging else {
            continue;
        };

        // GPU→GPU copy of the even sub-rect. The shared texture has no keyed
        // mutex (legacy share), so copy promptly before the game's next present.
        let copy = (|| -> WinResult<()> {
            let dst: ID3D11Resource = staging.cast()?;
            let src: ID3D11Resource = shared_tex.cast()?;
            let box_ = D3D11_BOX {
                left: 0,
                top: 0,
                front: 0,
                right: width,
                bottom: height,
                back: 1,
            };
            unsafe {
                context.CopySubresourceRegion(&dst, 0, 0, 0, 0, &src, 0, Some(&box_));
            }
            Ok(())
        })();
        if let Err(e) = copy {
            if !warned_copy {
                tracing::warn!(
                    "hook: shared-texture → staging copy failed (first occurrence; \
                     likely a format/size mismatch between the backbuffer and the \
                     staging pool): {e:?}"
                );
                warned_copy = true;
            }
            if let Ok(mut p) = free_pool.lock() {
                p.push(staging);
            }
            continue;
        }

        shared.last_handed_time.store(ts, Ordering::Relaxed);
        match filled_tx.try_send((staging, ts)) {
            Ok(()) => {
                shared.handed.fetch_add(1, Ordering::Relaxed);
                sent_window += 1;
            }
            Err(TrySendError::Full((tex, _))) | Err(TrySendError::Disconnected((tex, _))) => {
                if let Ok(mut p) = free_pool.lock() {
                    p.push(tex);
                }
            }
        }
    }

    // Dropping `hook` here signals Stop + releases the keepalive mutex, so the
    // injected DLL self-terminates. Dropping `filled_tx` ends the encode thread.
    drop(hook);
}

/// Map a TYPELESS (or sRGB) backbuffer format to the fully-typed UNORM format the
/// VideoProcessor input view requires. The hook shares the backbuffer as TYPELESS
/// (because `allow_srgb_alias` lets the consumer choose UNORM vs sRGB), but
/// `CreateVideoProcessorInputView` rejects TYPELESS — and copying TYPELESS→UNORM
/// is legal since they share a format family. Unknown formats pass through.
fn typed_capture_format(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_B8G8R8X8_TYPELESS => DXGI_FORMAT_B8G8R8X8_UNORM,
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => DXGI_FORMAT_R8G8B8A8_UNORM,
        other => other,
    }
}

/// Create a staging texture for `src`'s format family (even `width`/`height`),
/// usable as a `CopySubresourceRegion` destination and a VideoProcessor input.
/// TYPELESS/sRGB backbuffer formats are mapped to their typed UNORM equivalent so
/// the VideoProcessor accepts them (see [`typed_capture_format`]).
fn create_staging_like(
    device: &ID3D11Device,
    src: &D3D11_TEXTURE2D_DESC,
    width: u32,
    height: u32,
) -> WinResult<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: typed_capture_format(src.Format),
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut tex))?;
    }
    Ok(tex.expect("CreateTexture2D returned null staging texture"))
}

type CaptureObjects = (
    Direct3D11CaptureFramePool,
    windows::Graphics::Capture::GraphicsCaptureSession,
    i64,
);

/// Create the WGC frame pool + session for `item` on `device`, wiring the
/// FrameArrived callback. With `handoff = Some`, the callback copies + hands off
/// each frame to the encode thread; with `None` it only counts (used by tests).
fn setup_wgc(
    device: &ID3D11Device,
    item: GraphicsCaptureItem,
    target_fps: u32,
    shared: &Arc<Shared>,
    handoff: Option<Handoff>,
) -> WinResult<CaptureObjects> {
    let winrt_device = create_winrt_device(device)?;
    let size = item.Size()?;

    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        WGC_POOL_FRAMES,
        size,
    )?;
    let session = pool.CreateCaptureSession(&item)?;
    // Win11 niceties; ignore on older builds / unpackaged restrictions.
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);

    // Drop frames that arrive faster than the target interval (frame pacing).
    let interval_100ns: i64 = (10_000_000 / target_fps as i64) * 95 / 100;
    let shared = shared.clone();

    let handler = TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(
        move |frame_pool, _| -> WinResult<()> {
            let frame_pool = frame_pool.as_ref().expect("frame pool present");
            let frame = frame_pool.TryGetNextFrame()?;
            shared.arrived.fetch_add(1, Ordering::Relaxed);

            let time = frame.SystemRelativeTime()?.Duration;
            let content = frame.ContentSize()?;
            shared.width.store(content.Width.max(0) as u32, Ordering::Relaxed);
            shared.height.store(content.Height.max(0) as u32, Ordering::Relaxed);

            let last = shared.last_handed_time.load(Ordering::Relaxed);
            if last != 0 && time - last < interval_100ns {
                return Ok(()); // early frame — dropping recycles the texture
            }

            // Extract the captured BGRA texture (stays on the GPU).
            let surface = frame.Surface()?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
            let bgra: ID3D11Texture2D = unsafe { access.GetInterface()? };

            match &handoff {
                // No encode pipeline (tests): just count throughput.
                None => {
                    shared.last_handed_time.store(time, Ordering::Relaxed);
                    shared.handed.fetch_add(1, Ordering::Relaxed);
                }
                Some(h) => {
                    // Source must be at least our (even) capture size; otherwise
                    // the window resized smaller — drop until resize is handled.
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe { bgra.GetDesc(&mut desc) };
                    if desc.Width < h.width || desc.Height < h.height {
                        return Ok(());
                    }

                    // Grab a free staging texture; none → encoder backpressure, drop.
                    let staging = match h.free_pool.lock() {
                        Ok(mut p) => p.pop(),
                        Err(_) => None,
                    };
                    let Some(staging) = staging else {
                        return Ok(());
                    };

                    // GPU→GPU copy of the even sub-rect (no CPU readback). Must
                    // happen now — WGC recycles the frame's buffer after this.
                    let copy = (|| -> WinResult<()> {
                        let dst: ID3D11Resource = staging.cast()?;
                        let src: ID3D11Resource = bgra.cast()?;
                        let box_ = D3D11_BOX {
                            left: 0,
                            top: 0,
                            front: 0,
                            right: h.width,
                            bottom: h.height,
                            back: 1,
                        };
                        unsafe {
                            h.context.CopySubresourceRegion(
                                &dst, 0, 0, 0, 0, &src, 0, Some(&box_),
                            );
                        }
                        Ok(())
                    })();
                    if copy.is_err() {
                        if let Ok(mut p) = h.free_pool.lock() {
                            p.push(staging);
                        }
                        return Ok(());
                    }

                    shared.last_handed_time.store(time, Ordering::Relaxed);
                    match h.filled_tx.try_send((staging, time)) {
                        Ok(()) => {
                            shared.handed.fetch_add(1, Ordering::Relaxed);
                        }
                        // Channel full (shouldn't happen: pool bounds it) or the
                        // encode thread is gone — return the texture and drop.
                        Err(TrySendError::Full((tex, _)))
                        | Err(TrySendError::Disconnected((tex, _))) => {
                            if let Ok(mut p) = h.free_pool.lock() {
                                p.push(tex);
                            }
                        }
                    }
                }
            }
            Ok(())
        },
    );

    let token = pool.FrameArrived(&handler)?;
    session.StartCapture()?;
    Ok((pool, session, token))
}

/// Output (encode) dimensions for a captured `src_w`x`src_h` frame given an
/// optional resolution target box. Fits the source into the box **by height and
/// never upscales** (Medal's `MatchHeight`), preserving aspect ratio; both
/// results are even (NV12 is 4:2:0). A `None` target — or one at least as tall as
/// the source — yields the source size unchanged (native capture).
fn scaled_output(src_w: u32, src_h: u32, target: Option<(u32, u32)>) -> (u32, u32) {
    let even = |v: u32| (v & !1).max(2);
    match target {
        Some((_, target_h)) if src_h > 0 && target_h < src_h => {
            let factor = target_h as f64 / src_h as f64;
            let w = (src_w as f64 * factor).round() as u32;
            let h = (src_h as f64 * factor).round() as u32;
            (even(w), even(h))
        }
        _ => (even(src_w), even(src_h)),
    }
}

/// Encode thread: owns the Converter, Encoder, and NV12 ring.
/// Receives staging BGRA textures, converts to NV12, encodes, recycles staging.
///
/// Takes the capture device (for the `Converter`, which reads the capture-side
/// BGRA staging textures) and the encode device + vendor (for the `Encoder`)
/// separately — the dual-device groundwork for cross-adapter encode. On the
/// single-device fast path the caller passes the *same* device for both, so the
/// NV12 textures the converter produces are consumed directly by the encoder with
/// no cross-adapter copy (today's behavior, unchanged). A later phase makes the
/// devices differ and inserts the shared keyed-mutex NV12 hand-off between them.
#[allow(clippy::too_many_arguments)]
fn encode_thread(
    capture_device: ID3D11Device,
    capture_context: ID3D11DeviceContext,
    encode_device: ID3D11Device,
    encode_context: ID3D11DeviceContext,
    encode_vendor: device::Vendor,
    width: u32,
    height: u32,
    fps: u32,
    enc_cfg: EncodeSettings,
    filled_rx: Receiver<(ID3D11Texture2D, i64)>,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    shared: Arc<Shared>,
    clip: Arc<ClipBuffer>,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    // The encode thread must keep draining captured frames even while the game
    // pins the CPU, or the bounded hand-off channel backs up and WGC drops frames.
    crate::core::boost_current_thread_priority("encode");

    // Output (encode) size: native capture size, or downscaled to the configured
    // resolution target. The converter scales BGRA(native) → NV12(out) in its
    // existing GPU pass, so everything downstream (encoder, NV12 ring, clip meta)
    // works at the output size.
    let (out_w, out_h) = scaled_output(width, height, enc_cfg.target_res);
    if (out_w, out_h) != (width, height) {
        tracing::info!("scaling capture {width}x{height} -> {out_w}x{out_h}");
    }

    // The converter runs on the CAPTURE device (it reads the capture-side BGRA
    // staging textures via VideoProcessorBlt and writes NV12).
    let converter = match Converter::new(&capture_device, &capture_context, width, height, out_w, out_h) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("converter init: {e:?}")));
            return;
        }
    };
    // The encoder runs on the ENCODE device with the ENCODE adapter's vendor
    // (== capture on the single-device fast path).
    let mut encoder = match Encoder::new(
        &encode_device,
        &encode_context,
        encode_vendor,
        enc_cfg.codec,
        enc_cfg.bitrate_mbps,
        out_w,
        out_h,
        fps,
    ) {
        Ok(e) => e,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("encoder init: {e}")));
            return;
        }
    };
    // Publish muxing metadata now that the encoder (and its codec-config
    // extradata) exists, so the save path can write clips (mux.rs). codec_id comes
    // from the encoder, which may have fallen back from the requested codec. Size
    // is the encoded (output) size, not the captured one.
    clip.set_meta(ClipMeta {
        width: out_w,
        height: out_h,
        fps,
        codec_id: encoder.codec().av_codec_id(),
        extradata: encoder.extradata(),
    });
    let mut nv12_ring: Vec<ID3D11Texture2D> = Vec::with_capacity(NV12_RING);
    for _ in 0..NV12_RING {
        match converter.create_nv12_texture() {
            Ok(t) => nv12_ring.push(t),
            Err(e) => {
                let _ = ready_tx.send(Err(format!("nv12 ring: {e:?}")));
                return;
            }
        }
    }
    let _ = ready_tx.send(Ok(()));

    // Account for stats and store the compressed packets in the RAM ring. The
    // per-packet ring lock is uncontended on the hot path (only a save contends).
    let record = |pkts: Vec<EncodedPacket>| {
        if pkts.is_empty() {
            return;
        }
        shared
            .enc_packets
            .fetch_add(pkts.len() as u64, Ordering::Relaxed);
        let bytes: usize = pkts.iter().map(|p| p.data.len()).sum();
        shared.enc_bytes.fetch_add(bytes as u64, Ordering::Relaxed);
        for p in pkts {
            clip.push(p);
        }
    };

    let mut idx = 0usize;
    let mut clock = MasterClock::new(fps);

    // Constant-frame-rate padding. WGC only delivers a frame when the desktop
    // composition updates, so capture runs *below* the target under the DWM cap
    // — e.g. ~50 of 60. We fill the PTS gaps between real frames by
    // re-encoding the previous NV12 surface (what was on screen during the gap),
    // which the encoder turns into a tiny P-frame. The clip then plays as a clean
    // constant `fps` — matching Medal's duplicate-frame pacing — while PTS still
    // tracks wall-clock, so the duration is unchanged. `max_gap_fill` caps the
    // burst on a long static screen (e.g. alt-tabbed) to ~1s of duplicates.
    let max_gap_fill = fps as i64;
    let mut last_pts: Option<i64> = None;
    let mut prev_nv12: Option<ID3D11Texture2D> = None;
    let mut warned_convert = false;

    while let Ok((staging, ts)) = filled_rx.recv() {
        let nv12 = nv12_ring[idx % nv12_ring.len()].clone();
        idx += 1;

        let conv = converter.convert(&staging, &nv12);
        // The blt reads `staging` on the same ordered context, so it can be
        // recycled now — a later CopySubresourceRegion into it is serialized.
        if let Ok(mut p) = free_pool.lock() {
            p.push(staging);
        }
        if let Err(e) = conv {
            if !warned_convert {
                tracing::warn!(
                    "encode: BGRA→NV12 convert failed (first occurrence; the source \
                     format may be unsupported by the VideoProcessor input view): {e:?}"
                );
                warned_convert = true;
            }
            continue;
        }

        // PTS from the capture clock, scaled to the encoder time_base (1/fps).
        let mut pts = clock.pts(ts);
        // Publish the wall-clock anchor (first-frame tick) so the save path can
        // align absolute-tick audio against this video. Idempotent (OnceLock).
        if let Some(base) = clock.base_ticks() {
            clip.set_video_base(base);
        }

        if let Some(lp) = last_pts {
            if pts <= lp {
                // Capture jitter landed a frame at/under the last slot; nudge it
                // forward so PTS stays strictly increasing (the muxer requires it).
                pts = lp + 1;
            } else if let Some(prev) = &prev_nv12 {
                // Fill the gap since the last real frame with duplicates of the
                // previous surface so the output is constant-rate.
                let fill_until = pts.min(lp + 1 + max_gap_fill);
                let mut fill = lp + 1;
                while fill < fill_until {
                    match encoder.encode(prev, fill) {
                        Ok(pkts) => record(pkts),
                        Err(e) => tracing::warn!("gap-fill encode error: {e}"),
                    }
                    fill += 1;
                }
            }
        }

        match encoder.encode(&nv12, pts) {
            Ok(pkts) => record(pkts),
            Err(e) => tracing::warn!("encode error: {e}"),
        }
        last_pts = Some(pts);
        prev_nv12 = Some(nv12);
    }

    // Channel closed (capture stopped): flush the encoder and exit.
    if let Ok(pkts) = encoder.flush() {
        record(pkts);
    }
    tracing::info!("hako-encode thread exiting");
}

fn emit_loop(app: &AppHandle, target_fps: u32, stop: &Arc<AtomicBool>, shared: &Arc<Shared>) {
    let mut last_count = 0u64;
    let mut last_enc = 0u64;
    let mut last_bytes = 0u64;
    let mut last_at = Instant::now();
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(300));
        let now = Instant::now();
        let dt = (now - last_at).as_secs_f64();

        let count = shared.handed.load(Ordering::Relaxed);
        let enc = shared.enc_packets.load(Ordering::Relaxed);
        let bytes = shared.enc_bytes.load(Ordering::Relaxed);

        let rate = |delta: u64| {
            if dt > 0.0 {
                ((delta as f64 / dt) * 10.0).round() / 10.0
            } else {
                0.0
            }
        };
        let fps = rate(count - last_count);
        let encoded_fps = rate(enc - last_enc);
        let encoded_kbps = if dt > 0.0 {
            (((bytes - last_bytes) as f64 * 8.0 / 1000.0) / dt).round()
        } else {
            0.0
        };

        last_count = count;
        last_enc = enc;
        last_bytes = bytes;
        last_at = now;

        let stats = CaptureStats {
            fps,
            frames: count,
            arrived: shared.arrived.load(Ordering::Relaxed),
            width: shared.width.load(Ordering::Relaxed),
            height: shared.height.load(Ordering::Relaxed),
            target_fps,
            encoded_fps,
            encoded_frames: enc,
            encoded_kbps,
        };
        let _ = app.emit(events::CAPTURE_STATS, &stats);
    }
}

/// Allocate a BGRA staging texture: VideoProcessor input (RENDER_TARGET) + copy
/// destination. Same format WGC delivers (`B8G8R8A8_UNORM`).
fn create_bgra_staging(device: &ID3D11Device, width: u32, height: u32) -> WinResult<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device.CreateTexture2D(&desc, None, Some(&mut tex))?;
    }
    Ok(tex.expect("CreateTexture2D returned null staging texture"))
}

/// Wrap our D3D11 device as a WinRT `IDirect3DDevice` for the frame pool.
fn create_winrt_device(device: &ID3D11Device) -> WinResult<IDirect3DDevice> {
    let dxgi: IDXGIDevice = device.cast()?;
    let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
    inspectable.cast()
}

/// Create a `GraphicsCaptureItem` for a window via the interop factory.
fn create_item_for_window(hwnd: HWND) -> WinResult<GraphicsCaptureItem> {
    let interop: IGraphicsCaptureItemInterop =
        windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe { interop.CreateForWindow(hwnd) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_output_fits_by_height_and_never_upscales() {
        // Native (no target) is unchanged, just even-rounded.
        assert_eq!(scaled_output(1920, 1080, None), (1920, 1080));
        assert_eq!(scaled_output(1921, 1081, None), (1920, 1080));
        // 1440p source, 720p target → halved, aspect preserved.
        assert_eq!(scaled_output(2560, 1440, Some((1280, 720))), (1280, 720));
        // 16:9 1080p source into a 720p box.
        assert_eq!(scaled_output(1920, 1080, Some((1280, 720))), (1280, 720));
        // Ultrawide keeps aspect (width follows the height scale, may exceed box).
        assert_eq!(scaled_output(3440, 1440, Some((1280, 720))), (1720, 720));
        // Target taller than source → no upscale, stays native.
        assert_eq!(scaled_output(1280, 720, Some((1920, 1080))), (1280, 720));
        // Equal height → unchanged.
        assert_eq!(scaled_output(1280, 720, Some((1280, 720))), (1280, 720));
    }

    /// Exercises the WGC path (interop → device → frame pool → FrameArrived →
    /// D3D11 texture extraction) against real on-screen windows. No encode
    /// pipeline here (`handoff = None`) — that's covered by the encode tests.
    #[test]
    fn lists_and_captures_a_window() {
        let windows = list_windows();
        println!("enumerated {} windows", windows.len());
        assert!(!windows.is_empty(), "expected some visible windows");

        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }

        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter =
            device::default_capture_index(&gpus).map(|i| device::adapter_at(i).expect("adapter"));
        let (d3d_device, _ctx, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");

        // Some windows are minimized/occluded and yield no size — try several.
        let mut captured_any = false;
        for w in windows.iter().take(10) {
            let item = match create_item_for_window(HWND(w.hwnd as *mut c_void)) {
                Ok(i) => i,
                Err(_) => continue,
            };
            let shared = Arc::new(Shared::default());
            let (pool, session, token) = match setup_wgc(&d3d_device, item, 60, &shared, None) {
                Ok(objs) => objs,
                Err(_) => continue,
            };
            std::thread::sleep(Duration::from_millis(700));
            let arrived = shared.arrived.load(Ordering::Relaxed);
            let width = shared.width.load(Ordering::Relaxed);
            let height = shared.height.load(Ordering::Relaxed);
            let _ = pool.RemoveFrameArrived(token);
            let _ = session.Close();
            let _ = pool.Close();

            if arrived >= 1 && width > 0 && height > 0 {
                println!(
                    "captured '{}' {}x{} ({} frames in 0.7s)",
                    w.title, width, height, arrived
                );
                captured_any = true;
                break;
            }
        }

        unsafe {
            RoUninitialize();
        }
        assert!(captured_any, "no window produced a frame with a valid size");
    }

    /// Full live pipeline end-to-end: capture a real window → GPU copy → NV12
    /// convert → `h264_qsv` → compressed packets. Exercises the cross-thread
    /// hand-off, staging pool, NV12 ring, and PTS path that the unit tests don't.
    #[test]
    fn encodes_a_captured_window() {
        let windows = list_windows();
        assert!(!windows.is_empty(), "expected some visible windows");

        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }

        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let cap_index = device::default_capture_index(&gpus);
        let vendor = cap_index
            .and_then(|i| gpus.iter().find(|g| g.index == i))
            .map(|g| g.vendor)
            .unwrap_or(device::Vendor::Other);
        let adapter = cap_index.map(|i| device::adapter_at(i).expect("adapter"));
        let (d3d_device, context, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");

        let mut proved = false;
        for w in windows.iter().take(12) {
            let item = match create_item_for_window(HWND(w.hwnd as *mut c_void)) {
                Ok(i) => i,
                Err(_) => continue,
            };
            let size = match item.Size() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let width = (size.Width.max(0) as u32) & !1;
            let height = (size.Height.max(0) as u32) & !1;
            if width < 64 || height < 64 {
                continue;
            }

            // Wire the pipeline exactly like run_pipeline (minus AppHandle).
            let shared = Arc::new(Shared::default());
            let mut pool_vec = Vec::new();
            for _ in 0..STAGING_POOL {
                pool_vec.push(create_bgra_staging(&d3d_device, width, height).expect("staging"));
            }
            let free_pool = Arc::new(Mutex::new(pool_vec));
            let (filled_tx, filled_rx) = sync_channel::<(ID3D11Texture2D, i64)>(STAGING_POOL);
            let (rdy_tx, rdy_rx) = std::sync::mpsc::channel();
            let clip = ClipBuffer::new(60, 30, Vec::new());
            let enc = {
                let device = d3d_device.clone();
                let context = context.clone();
                let shared = shared.clone();
                let free_pool = free_pool.clone();
                let clip = clip.clone();
                let enc_cfg = EncodeSettings {
                    codec: crate::core::encode::VideoCodec::H264,
                    bitrate_mbps: 20,
                    target_res: None,
                };
                std::thread::spawn(move || {
                    encode_thread(
                        device.clone(), context.clone(), device, context, vendor, width, height,
                        60, enc_cfg, filled_rx, free_pool, shared, clip, rdy_tx,
                    )
                })
            };
            if !matches!(rdy_rx.recv(), Ok(Ok(()))) {
                let _ = enc.join();
                continue;
            }

            let handoff = Handoff {
                context: context.clone(),
                free_pool,
                filled_tx,
                width,
                height,
            };
            let (pool, session, token) =
                match setup_wgc(&d3d_device, item, 60, &shared, Some(handoff)) {
                    Ok(o) => o,
                    Err(_) => {
                        let _ = enc.join();
                        continue;
                    }
                };

            std::thread::sleep(Duration::from_millis(1500));

            let handed = shared.handed.load(Ordering::Relaxed);
            // Teardown: drop the closure (its filled_tx) so the encode thread
            // flushes and exits, then join it.
            let _ = pool.RemoveFrameArrived(token);
            let _ = session.Close();
            let _ = pool.Close();
            let _ = enc.join();
            let packets = shared.enc_packets.load(Ordering::Relaxed);
            let stats = clip.stats().expect("ring stats");

            if handed > 0 {
                println!(
                    "pipeline '{}' {}x{}: handed {} frames → {} encoded packets → ring {} pkts / {} kf / {:.2}s",
                    w.title,
                    width,
                    height,
                    handed,
                    packets,
                    stats.packets,
                    stats.keyframes,
                    stats.duration_secs
                );
                assert!(
                    packets > 0,
                    "captured {handed} frames but encoder produced no packets"
                );
                // The ring must hold the encoded packets, starting on an IDR.
                assert!(stats.packets > 0, "ring is empty after encoding {packets} packets");
                assert!(
                    stats.keyframes > 0,
                    "ring holds no keyframe — clips would have no IDR start"
                );

                // Prove the full save path: slice the ring and stream-copy to MP4.
                let out = std::env::temp_dir().join("hako_capture_clip.mp4");
                let _ = std::fs::remove_file(&out);
                let saved = clip.save_last(2, &out).expect("save_last");
                let size = std::fs::metadata(&saved.path).expect("clip file").len();
                println!(
                    "saved clip → {} ({} bytes, {:.2}s)",
                    saved.path.display(),
                    size,
                    saved.duration_secs
                );
                assert!(size > 0, "saved clip is empty");
                let _ = std::fs::remove_file(&out);

                proved = true;
                break;
            }
        }

        unsafe {
            RoUninitialize();
        }
        assert!(
            proved,
            "no window produced frames to encode (need a window with changing content)"
        );
    }
}
