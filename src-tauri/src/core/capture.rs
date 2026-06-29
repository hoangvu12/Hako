//! Game capture via an injected graphics hook: pull the game's shared backbuffer
//! at its real render rate → bounded hand-off → encode thread.
//!
//! Pipeline: the hook source loop ([`hook_source_loop`]) samples the latest
//! shared backbuffer and copies it into a pooled staging texture (a GPU→GPU copy
//! — no CPU readback), then hands that staging texture to a dedicated **encode
//! thread** over a bounded channel. The encode thread owns the `Converter`
//! (BGRA→NV12), the `Encoder` (`h264_qsv`), and a small NV12 texture ring. The
//! source loop only copies + sends; if no staging texture is free it drops the
//! frame (encoder backpressure). See [`crate::core::hook`] for the injection
//! details.

#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::core::{BOOL, Interface, Result as WinResult};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_TYPELESS, DXGI_FORMAT_B8G8R8X8_UNORM,
    DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible,
};

use crate::core::audio::{self, AudioCapture, AudioControl, AudioMeta};
use crate::settings::AudioConfig;
use crate::core::buffer::{AudioRing, BufferStats, PacketRing};
use crate::core::disk_buffer::DiskPacketRing;
use crate::core::clock::{MasterClock, TICKS_PER_SECOND};
use crate::core::convert::Converter;
use crate::core::device;
use crate::core::overlay_card;
use crate::core::encode::{EncodeSettings, EncodedPacket, Encoder};
use crate::core::hook::{HookCapture, RunningHook};
use crate::core::mux::{self, AudioClip, ClipMeta};
use crate::core::session::SessionWriter;
use crate::events;

/// Number of BGRA staging textures shared between the hook source loop and the
/// encode thread. Also bounds in-flight frames (backpressure: the source loop
/// drops when none are free). Small — we only need to cover channel + encoder
/// latency.
const STAGING_POOL: usize = 4;
/// NV12 textures the encode thread cycles through. Must exceed how many surfaces
/// the encoder holds asynchronously (`async_depth` ≈ 1–2) so a reused texture is
/// never still in flight.
const NV12_RING: usize = 6;

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
    /// Handed-off (captured + copied) frames per second, after FPS pacing.
    pub fps: f64,
    /// Total frames handed off to the encode thread since start.
    pub frames: u64,
    /// Total frames the hook delivered (before pacing) since start.
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
    /// True while the target window is minimized — the game stops presenting, so
    /// capture is necessarily frozen. Checked each tick in `hook_source_loop`.
    minimized: AtomicBool,
    /// True when we believe the captured content is frozen (minimized now, or the
    /// static watchdog tripped in Part B). Drives the honest "paused — minimized"
    /// recorder indicator instead of silently recording one stale frame.
    frozen: AtomicBool,
    /// SystemRelativeTime (100 ns) of the last frame whose content actually
    /// changed. Reserved for the Part B static watchdog; set on each live tick.
    last_fresh_time: AtomicI64,
    /// True when the in-frame freeze overlay ("tabbed out" card) should be drawn:
    /// the feature is currently **on** AND the capture is overlay-capable. Read
    /// per-frame by the encode thread (draw gate) and per-tick by the source loop
    /// (freeze-base snapshot + minimized keep-alive emit), so toggling the setting
    /// applies live via [`RunningCapture::set_freeze_overlay`].
    overlay_active: AtomicBool,
    /// True once the encode thread has confirmed the overlay can be drawn for this
    /// capture (format is D2D-targetable AND `FreezeOverlay` init succeeded). Fixed
    /// for the capture's life; gates the live `overlay_active` toggle so turning
    /// the feature on can't enable drawing on an incapable capture.
    overlay_capable: AtomicBool,
    /// Set by the source loop when the game changed its backbuffer resolution
    /// mid-capture (e.g. a 16:9 menu → a 4:3 stretched match). The recorder follows
    /// the game's native size, but a clip's dimensions are fixed for its lifetime
    /// (`ClipMeta` is a `OnceLock` and the buffer holds one resolution), so the
    /// capture thread tears the pipeline down and restarts it at the new size.
    resize_restart: AtomicBool,
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

/// The instant-replay video store: either a RAM ring or a disk-backed segment
/// ring, chosen per the `buffer_storage` setting. Both hold compressed packets
/// and expose the same push/slice/stats surface, so the rest of [`ClipBuffer`]
/// (and the save path) is identical regardless of backend.
enum VideoStore {
    Ram(Mutex<PacketRing>),
    Disk(Mutex<DiskPacketRing>),
}

impl VideoStore {
    fn push(&self, pkt: EncodedPacket) {
        match self {
            VideoStore::Ram(m) => {
                if let Ok(mut r) = m.lock() {
                    r.push(pkt);
                }
            }
            VideoStore::Disk(m) => {
                if let Ok(mut r) = m.lock() {
                    r.push(pkt);
                }
            }
        }
    }

    fn slice_last(&self, secs: u32) -> Vec<EncodedPacket> {
        match self {
            VideoStore::Ram(m) => m.lock().map(|r| r.slice_last(secs)).unwrap_or_default(),
            VideoStore::Disk(m) => m.lock().map(|mut r| r.slice_last(secs)).unwrap_or_default(),
        }
    }

    fn stats(&self) -> Option<BufferStats> {
        match self {
            VideoStore::Ram(m) => m.lock().ok().map(|r| r.stats()),
            VideoStore::Disk(m) => m.lock().ok().map(|r| r.stats()),
        }
    }
}

pub struct ClipBuffer {
    video: VideoStore,
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
    /// orchestrator installs/takes it on match start/end (`valorant::integration`).
    session: Mutex<Option<Arc<SessionWriter>>>,
}

impl ClipBuffer {
    /// New clip buffer. `audio_track_names` fixes the audio-track layout (one
    /// AAC ring per name, track 0 = master); empty ⇒ video-only.
    ///
    /// `disk_buffer_dir` selects the video backend: `None` keeps the instant-replay
    /// buffer in a RAM ring (the default); `Some(dir)` spools it to rolling segment
    /// files under `dir` (Medal's "Recording buffer: Disk"). If the disk ring can't
    /// be initialized, it falls back to RAM so capture still records. Audio always
    /// stays in RAM (the AAC rings are a few MB even for a long buffer).
    fn new(
        fps: u32,
        retention_secs: u32,
        audio_track_names: Vec<String>,
        disk_buffer_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        let audio_tracks = audio_track_names
            .into_iter()
            .map(|name| AudioTrack {
                name,
                ring: Mutex::new(AudioRing::new(retention_secs)),
                meta: OnceLock::new(),
            })
            .collect();
        let video = match disk_buffer_dir {
            Some(dir) => match DiskPacketRing::new(dir, fps, retention_secs) {
                Ok(d) => {
                    tracing::info!("recording buffer: disk ({retention_secs}s)");
                    VideoStore::Disk(Mutex::new(d))
                }
                Err(e) => {
                    tracing::warn!("disk buffer init failed ({e}); using RAM buffer");
                    VideoStore::Ram(Mutex::new(PacketRing::new(fps, retention_secs)))
                }
            },
            None => VideoStore::Ram(Mutex::new(PacketRing::new(fps, retention_secs))),
        };
        Arc::new(ClipBuffer {
            video,
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
    /// Mode-B session writer when a Valorant match is recording. `frozen` marks the
    /// frame as captured while frozen (minimized / stale swapchain) so the session
    /// writer can record frozen spans for the post-match cut to skip.
    fn push(&self, pkt: EncodedPacket, frozen: bool) {
        if let Some(session) = self.active_session() {
            // Reconstruct the packet's wall-clock tick from the shared video-base
            // anchor + its PTS — the same linear map the save path uses to place
            // audio against video. Both are known by the time packets flow.
            if let (Some(&base), Some(meta)) = (self.video_base.get(), self.meta.get()) {
                let fps = meta.fps.max(1) as i64;
                let wall = base + pkt.pts * TICKS_PER_SECOND / fps;
                session.push(&pkt, wall, frozen);
            }
        }
        self.video.push(pkt);
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
    /// skipped — so the caller must compare against [`Self::audio_track_count`]
    /// and wait until all planned tracks are present before snapshotting: when
    /// Hako is opened mid-game the audio thread may still be opening its
    /// (per-process loopback) inputs, and an empty/partial snapshot would declare
    /// a video-only session, leaving every auto-clip silent. See
    /// `valorant::integration::start_match`.
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
        let packets = self.video.slice_last(secs);
        if packets.is_empty() {
            return Err("buffer is empty — nothing to save".into());
        }
        // Duration tracks wall-clock via the PTS span, NOT frame_count/fps:
        // capture can run below target fps when the game renders slowly, so PTS
        // (1/fps units, derived from the capture clock) is the real
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
        self.video.stats()
    }
}

/// Handle to a running capture; drop or call `stop()` to tear it down.
pub struct RunningCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Shared clip buffer for the hotkey/command save path.
    clip: Arc<ClipBuffer>,
    /// Live capture metrics + liveness flags (shared with the source/encode
    /// threads). Read by the recorder-status snapshot for the honest indicator.
    shared: Arc<Shared>,
    /// The captured window — retained so a settings change (e.g. enabling the
    /// mic) can restart this capture against the same target to pick up the new
    /// audio/encode config (capture snapshots its config at start).
    hwnd: i64,
    /// Live audio control: push a layout-preserving change (volume/mute or a
    /// device swap) to the running audio thread without restarting capture
    /// (Medal's `AudioCaptureVolume` / `UpdateAudioCaptureAndProcessor` paths).
    /// Only valid for `layout_eq` configs; layout/encode changes restart instead.
    audio_control: Arc<AudioControl>,
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

    /// The captured window handle (for a config-change restart).
    pub fn hwnd(&self) -> i64 {
        self.hwnd
    }

    /// Apply a layout-preserving audio change (volume/mute or a device swap) to
    /// the live capture without a restart. The caller must have verified `cfg`
    /// keeps the same output-track layout + input kinds as the running config
    /// (`AudioConfig::layout_eq`) — only device identity, mono, or levels differ.
    pub fn reconfigure_audio(&self, cfg: AudioConfig) {
        self.audio_control.reconfigure(cfg);
    }

    /// Toggle the in-frame freeze overlay ("tabbed out" card) on the live capture
    /// without a restart — it's a per-frame flag. Clamped by overlay capability,
    /// so turning it on does nothing when the capture format can't be annotated.
    pub fn set_freeze_overlay(&self, on: bool) {
        let capable = self.shared.overlay_capable.load(Ordering::Acquire);
        self.shared
            .overlay_active
            .store(on && capable, Ordering::Release);
    }

    /// Whether a Valorant match is actively being recorded into this capture.
    /// A restart while true would orphan the in-progress session's buffer, so
    /// the settings path defers config-change restarts until the match ends.
    pub fn has_active_session(&self) -> bool {
        self.clip.active_session().is_some()
    }

    /// Whether we're capturing *fresh* frames right now — false when the game is
    /// minimized (or otherwise frozen), so the recorder can say "paused" instead
    /// of pretending a frozen frame is live footage.
    pub fn capturing_live(&self) -> bool {
        !self.shared.frozen.load(Ordering::Relaxed)
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
    find_window_by_title("VALORANT")
}

/// Find the first visible top-level window whose (trimmed) title matches `want`
/// case-insensitively, returning its HWND. The game-agnostic window detector each
/// [`crate::games`] integration uses to auto-start capture when its game appears
/// (Valorant → "VALORANT", League → "League of Legends (TM) Client"). The exact
/// (trimmed) compare avoids matching browser tabs like "VALORANT - YouTube".
pub fn find_window_by_title(want: &str) -> Option<i64> {
    let mut search = TitleSearch { want, found: 0 };
    // SAFETY: `search` outlives the EnumWindows call; the callback only writes it.
    unsafe {
        let _ = EnumWindows(
            Some(find_title_proc),
            LPARAM(&mut search as *mut TitleSearch as isize),
        );
    }
    (search.found != 0).then_some(search.found)
}

/// State threaded through [`find_title_proc`] via `EnumWindows`' `LPARAM`.
struct TitleSearch<'a> {
    want: &'a str,
    found: i64,
}

/// Find the first visible top-level window owned by a process whose name matches
/// any of `process_names` (case-insensitive), returning its HWND. Used when a
/// game's window title is unreliable/unknown but its executable name is certain
/// (Rematch → "RuntimeClient-Win64-Shipping.exe"). Two passes: resolve the target
/// PIDs via `sysinfo`, then enumerate windows and match the owning PID.
pub fn find_window_by_process(process_names: &[&str]) -> Option<i64> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: std::collections::HashSet<u32> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            p.name()
                .to_str()
                .map(|n| process_names.iter().any(|w| n.eq_ignore_ascii_case(w)))
                .unwrap_or(false)
        })
        .map(|(pid, _)| pid.as_u32())
        .collect();
    if pids.is_empty() {
        return None;
    }
    let mut search = ProcessSearch { pids: &pids, found: 0 };
    // SAFETY: `search` outlives the EnumWindows call; the callback only writes it.
    unsafe {
        let _ = EnumWindows(
            Some(find_process_proc),
            LPARAM(&mut search as *mut ProcessSearch as isize),
        );
    }
    (search.found != 0).then_some(search.found)
}

/// State threaded through [`find_process_proc`] via `EnumWindows`' `LPARAM`.
struct ProcessSearch<'a> {
    pids: &'a std::collections::HashSet<u32>,
    found: i64,
}

unsafe extern "system" fn find_process_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut ProcessSearch);
    // Only visible, titled top-level windows (skips the game's hidden helper
    // windows / splash so we latch the real render surface).
    if IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != 0 && search.pids.contains(&pid) {
            search.found = hwnd.0 as i64;
            return BOOL(0); // stop enumeration
        }
    }
    BOOL(1)
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

unsafe extern "system" fn find_title_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam.0 as *mut TitleSearch);
    if IsWindowVisible(hwnd).as_bool() {
        let len = GetWindowTextLengthW(hwnd);
        if len > 0 {
            let mut buf = vec![0u16; len as usize + 1];
            let n = GetWindowTextW(hwnd, &mut buf);
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if title.trim().eq_ignore_ascii_case(search.want) {
                    search.found = hwnd.0 as i64;
                    return BOOL(0); // stop enumeration
                }
            }
        }
    }
    BOOL(1)
}

// ===========================================================================
// Game-capture (graphics-hook injection) path — captures at the game's real
// render rate, above the desktop-composition cap.
// ===========================================================================

/// How long to wait for the injected hook to deliver its first frame before
/// giving up. If the game isn't presenting (minimized) or the DLL was blocked by
/// anti-cheat, we fail here rather than hang the start command.
const HOOK_FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(12);

/// Start capture for a window: inject the OBS-derived `graphics-hook` into the
/// game, pull the shared backbuffer at the game's real render rate, and run it
/// through the `Converter` → `Encoder` → clip-buffer pipeline. Returns a
/// [`RunningCapture`] handle that the save path and capture state use.
///
/// This is the app's only capture backend; it injects into the target process.
pub fn start_hook(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    buffer_secs: u32,
    disk_buffer_dir: Option<PathBuf>,
    audio: AudioConfig,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    let target_fps = target_fps.clamp(1, 480);
    let game_pid = pid_for_hwnd(hwnd_raw);
    let track_names = audio::planned_track_names(&audio, game_pid);
    let clip = ClipBuffer::new(target_fps, buffer_secs.clamp(5, 600), track_names, disk_buffer_dir);
    // Shared live-audio control: the audio thread reads its initial config here
    // and re-reads it on a pushed volume change (no restart).
    let audio_control = AudioControl::new(audio);

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let thread = {
        let stop = stop.clone();
        let shared = shared.clone();
        let clip = clip.clone();
        let audio_control = audio_control.clone();
        std::thread::Builder::new()
            .name("hako-capture-hook".into())
            .spawn(move || {
                hook_capture_thread(
                    app, hwnd_raw, target_fps, adapter_index, audio_control, game_pid, stop,
                    shared, clip, enc_cfg, ready_tx,
                )
            })
            .map_err(|e| format!("failed to spawn hook capture thread: {e}"))?
    };

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(RunningCapture {
            stop,
            thread: Some(thread),
            clip,
            shared,
            hwnd: hwnd_raw,
            audio_control,
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
    audio_control: Arc<AudioControl>,
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
        audio_control,
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
            // The source loop asks for a restart when the game changed resolution
            // mid-capture, so the new clip records at the game's native size. Do it
            // from a detached thread: the restart's `stop_capture_with` joins THIS
            // capture thread, so this thread must be free to return first.
            // `start_capture_with` re-detects the new size and builds a fresh clip
            // buffer. A pending user stop wins (don't resurrect a stopped capture).
            if shared.resize_restart.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                let app = app.clone();
                std::thread::spawn(move || {
                    crate::commands::stop_capture_with(&app);
                    match crate::commands::start_capture_with(&app, hwnd_raw, None, None) {
                        Ok(()) => tracing::info!(
                            "capture: restarted at the game's new resolution"
                        ),
                        Err(e) => tracing::warn!(
                            "capture: restart after resolution change failed: {e}"
                        ),
                    }
                });
            }
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
    audio_control: Arc<AudioControl>,
    game_pid: Option<u32>,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
) -> std::result::Result<RunningHookPipeline, String> {
    // The hook copies the game's backbuffer into a *legacy* shared texture on the
    // GPU the game RENDERS on (the dGPU on a hybrid/Optimus laptop), and a legacy
    // shared handle can only be reopened via `OpenSharedResource` on that same
    // adapter. So default to the preferred (highest-VRAM = discrete) GPU, NOT the
    // display-owning one — on an Optimus laptop the internal panel is driven by
    // the iGPU, which would make the shared-texture open fail. An explicit
    // `adapter_index` still wins.
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

    // The in-frame freeze overlay needs a D2D-targetable staging format (the same
    // typed format the converter reads). When the game renders in one D2D can't
    // target, we skip the overlay (and the source loop's keep-alive emit) rather
    // than ship un-annotated duplicate frames.
    // The overlay is *capable* whenever the format is D2D-targetable; whether it's
    // actually drawn is the live `overlay_active` toggle (the `freeze_overlay`
    // setting), applied without a restart. We build the overlay resources up front
    // when capable — even if the feature starts off — so it can be toggled on
    // mid-capture.
    let overlay_format = typed_capture_format(first_desc.Format);
    let overlay_capable = overlay_card::format_supported(overlay_format);
    if enc_cfg.freeze_overlay && !overlay_capable {
        tracing::info!(
            format = overlay_format.0,
            "freeze overlay off: capture format is not Direct2D-targetable"
        );
    }

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

    // The shared encode thread: convert → encode → clip buffer.
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
                    height, target_fps, enc_cfg, overlay_capable, filled_rx, free_pool, shared, clip,
                    enc_ready_tx,
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
    // staging texture, and hand it to the encode thread as an
    // `(ID3D11Texture2D, ts)` pair.
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
                    hook, hwnd_raw, device, context, width, height, target_fps, filled_tx,
                    free_pool, shared, source_stop,
                )
            })
            .map_err(|e| format!("spawn hook source thread: {e}"))?
    };

    let audio = if clip.audio_track_count() > 0 {
        match AudioCapture::start(clip.clone(), audio_control, game_pid) {
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

/// The hook frame-source loop: pull a shared backbuffer, copy its even sub-rect
/// into a free staging texture, and send it on. Owns the `RunningHook` so
/// dropping at loop-end tears the hook down.
fn hook_source_loop(
    mut hook: RunningHook,
    hwnd_raw: i64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    width: u32,
    height: u32,
    fps: u32,
    filled_tx: SyncSender<(ID3D11Texture2D, i64)>,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    // Exempt from process-level EcoQoS set while hidden to tray: this thread does
    // the per-frame shared-backbuffer copy (and the static-frame watchdog), so it
    // must stay on a performance core throughout a match.
    crate::core::protect_thread_high_qos("hook-source");
    let mut warned_copy = false;
    // One-time WARN when the session first goes frozen, so the log isn't silent
    // while the game is minimized (it would otherwise keep logging healthy-looking
    // "frames sampled=N handed=N" off a stale texture). Reset when it recovers.
    let mut warned_frozen = false;
    // The hook gives no per-frame signal on the shtex path — it just keeps
    // overwriting the shared texture each present. So we *pace* ourselves to the
    // target fps and sample the latest backbuffer each tick (the encode thread's
    // constant-rate gap-fill smooths over any presents we sampled twice).
    let frame_interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next_tick = Instant::now();

    // ── Part B: static-frame watchdog state ─────────────────────────────────
    // A non-minimized freeze (e.g. capturing a stale swapchain after a
    // fullscreen↔borderless switch) is invisible on the shtex path — there's no
    // liveness signal. So ~once a second we hash a small center patch of the
    // freshly-copied frame; if it stops changing while the window is visible and
    // not minimized, we flag `frozen` and (after a longer window, debounced) ask
    // the hook to re-hook the swapchain. Mirrors Medal's numSameFrames + regen.
    // Skipped for tiny windows (<probe), which aren't the freeze case.
    const PROBE: u32 = 64;
    const STATIC_SAMPLE: Duration = Duration::from_secs(1);
    const STATIC_FLAG_AFTER: Duration = Duration::from_secs(3);
    const STATIC_RESTART_AFTER: Duration = Duration::from_secs(5);
    const RESTART_DEBOUNCE: Duration = Duration::from_secs(10);
    let watchdog_ok = width >= PROBE && height >= PROBE;
    let mut readback: Option<ID3D11Texture2D> = None;
    let mut last_hash: Option<u64> = None;
    let mut last_change = Instant::now();
    let mut last_static_sample = Instant::now();
    let mut last_restart: Option<Instant> = None;
    let mut same_frames: u64 = 0;
    let mut warned_static = false;

    // Resize-follow: confirm a new backbuffer size persists for a few frames before
    // restarting the pipeline at it, so a transient swapchain blip (alt-tab,
    // fullscreen↔borderless toggle, a one-frame recreate) doesn't bounce capture.
    // Mismatched frames are dropped until then.
    const RESIZE_CONFIRM_FRAMES: u32 = 8;
    let mut resize_pending: Option<(u32, u32)> = None;
    let mut resize_frames: u32 = 0;

    // ── Freeze-overlay keep-alive ────────────────────────────────────────────
    // While minimized the game stops presenting, so the source loop normally just
    // skips — leaving the encoder to hold the last live frame. When the freeze
    // overlay is active we instead keep a recent snapshot of the live frame and,
    // during a minimize, re-emit it at a low rate so the encode thread can stamp
    // the "tabbed out" card on it (it sees `frozen`). The emitted frame carries a
    // wall-clock-synthesized timestamp so PTS keeps advancing in step with QPC and
    // the real timeline resumes seamlessly when the game returns. Both the snapshot
    // and the emit are gated on `overlay_active`, so this is a no-op otherwise.
    const BASE_SNAPSHOT: Duration = Duration::from_millis(250);
    const MINIMIZE_EMIT: Duration = Duration::from_millis(250);
    let mut freeze_base: Option<ID3D11Texture2D> = None;
    let mut last_base_snap = Instant::now() - BASE_SNAPSHOT;
    let mut last_minimize_emit = Instant::now() - MINIMIZE_EMIT;
    // Timestamp (100-ns ticks) of the last live frame, and when we observed it —
    // the anchor for synthesizing minimized keep-alive timestamps.
    let mut last_ts: i64 = 0;
    let mut last_ts_at = Instant::now();

    while !stop.load(Ordering::Acquire) {
        // Pace to the target frame interval.
        let now = Instant::now();
        if next_tick > now {
            std::thread::sleep(next_tick - now);
        } else {
            // Fell behind (or first iteration) — resync to avoid a burst.
            next_tick = now;
        }
        next_tick += frame_interval;

        // Minimized (e.g. alt-tabbed out of exclusive fullscreen) → the game has
        // stopped presenting, so the shared texture only holds the last frame. The
        // shtex path has no liveness signal, so re-copying it would silently
        // record a frozen stretch. Mark frozen, skip the frame, and let the encode
        // thread's constant-rate gap-fill repeat the previous real frame.
        if is_window_minimized(hwnd_raw) {
            shared.minimized.store(true, Ordering::Relaxed);
            shared.frozen.store(true, Ordering::Relaxed);
            if !warned_frozen {
                tracing::warn!(
                    "capture: game minimized/not presenting — frames frozen until it returns"
                );
                warned_frozen = true;
            }
            // Keep the timeline alive with the "tabbed out" card: re-emit the last
            // live frame at a low rate (the encode thread draws the card because
            // `frozen` is set; the encoder's gap-fill smooths the rest to CFR).
            if shared.overlay_active.load(Ordering::Relaxed)
                && last_minimize_emit.elapsed() >= MINIMIZE_EMIT
            {
                if let Some(base) = &freeze_base {
                    if let Some(staging) = free_pool.lock().ok().and_then(|mut p| p.pop()) {
                        let copied = (|| -> WinResult<()> {
                            let dst: ID3D11Resource = staging.cast()?;
                            let src: ID3D11Resource = base.cast()?;
                            unsafe {
                                context.CopySubresourceRegion(&dst, 0, 0, 0, 0, &src, 0, None);
                            }
                            Ok(())
                        })();
                        if copied.is_ok() {
                            // Wall-clock-aligned synthetic tick (QPC advances during
                            // the minimize, so this stays in step and the real ts
                            // resumes monotonically on restore).
                            let synth = last_ts
                                + (last_ts_at.elapsed().as_secs_f64() * TICKS_PER_SECOND as f64)
                                    as i64;
                            last_minimize_emit = Instant::now();
                            match filled_tx.try_send((staging, synth)) {
                                Ok(()) => {
                                    shared.handed.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(TrySendError::Full((tex, _)))
                                | Err(TrySendError::Disconnected((tex, _))) => {
                                    if let Ok(mut p) = free_pool.lock() {
                                        p.push(tex);
                                    }
                                }
                            }
                        } else if let Ok(mut p) = free_pool.lock() {
                            p.push(staging);
                        }
                    }
                }
            }
            continue;
        }
        if shared.minimized.swap(false, Ordering::Relaxed) {
            // Just came back on screen — clear the frozen flag and re-arm the WARN.
            shared.frozen.store(false, Ordering::Relaxed);
            warned_frozen = false;
            // Re-arm the static watchdog too: a long minimized gap left `last_change`
            // stale, which would otherwise instantly re-flag a freeze before the
            // first post-restore sample proves content is moving again.
            last_change = Instant::now();
            last_hash = None;
            warned_static = false;
            // Returning from minimize/alt-tab is exactly when the game tends to
            // recreate its swapchain (notably when leaving exclusive fullscreen), so
            // the shared texture we hold may now be stale. Proactively re-hook now
            // instead of waiting up to STATIC_RESTART_AFTER for the static watchdog
            // to notice — this mirrors how Overwolf's native engine reacts to its
            // GameExclusiveModeChangedEvent with ForceCaptureChangeRehook. Debounced
            // via `last_restart` so a restore + an immediate static sample can't
            // fire two restarts back to back.
            hook.request_restart();
            last_restart = Some(Instant::now());
            tracing::info!("capture: game restored — re-hooking swapchain, resuming live capture");
        }

        let frame = match hook.acquire(&device) {
            Ok(Some(f)) => f,
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

        // Follow the game's resolution. It can switch mid-capture (a 16:9 menu → a
        // 4:3 stretched match), reopening a differently-sized backbuffer; we record
        // at whatever size the game renders rather than padding it. The clip's
        // dimensions are fixed once it opens (the buffer holds one resolution), so
        // we can't change the output in place — restart the pipeline at the new
        // size. Confirm it holds for a few frames first (a transient swapchain blip
        // shouldn't bounce capture), and drop the mismatched frames meanwhile —
        // copying the old-sized box out of a resized texture is what leaves a stale
        // strip.
        let (live_w, live_h) = {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { shared_tex.GetDesc(&mut d) };
            (d.Width & !1, d.Height & !1)
        };
        if live_w >= 2 && live_h >= 2 && (live_w != width || live_h != height) {
            match resize_pending {
                Some((pw, ph)) if pw == live_w && ph == live_h => resize_frames += 1,
                _ => {
                    resize_pending = Some((live_w, live_h));
                    resize_frames = 1;
                }
            }
            if resize_frames >= RESIZE_CONFIRM_FRAMES {
                tracing::info!(
                    from_w = width,
                    from_h = height,
                    to_w = live_w,
                    to_h = live_h,
                    "capture: game changed resolution mid-capture — restarting to record at new size"
                );
                shared.resize_restart.store(true, Ordering::Release);
                break;
            }
            continue;
        } else if resize_pending.is_some() {
            resize_pending = None;
            resize_frames = 0;
        }

        // Grab a free staging texture; none → encoder backpressure, drop.
        let staging = match free_pool.lock() {
            Ok(mut p) => p.pop(),
            Err(_) => None,
        };
        let Some(staging) = staging else {
            continue;
        };

        // GPU→GPU copy of the even sub-rect (sizes match here — a differing live
        // size is handled by the resize-follow restart above). The shared texture
        // has no keyed mutex (legacy share), so copy promptly before the game's
        // next present.
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

        // Part B static-frame watchdog: ~once a second, hash a center patch of
        // the freshly-copied `staging` (we still own it here, before handoff).
        // `last_fresh_time`/`frozen` are driven by *content change*, not mere
        // handoff, so a non-minimized stale stretch is detected and recovered.
        if watchdog_ok && last_static_sample.elapsed() >= STATIC_SAMPLE {
            last_static_sample = Instant::now();
            if let Some(hash) =
                probe_center_hash(&device, &context, &staging, &mut readback, width, height, PROBE)
            {
                if last_hash != Some(hash) {
                    // Content moved — capture is genuinely live.
                    last_hash = Some(hash);
                    last_change = Instant::now();
                    same_frames = 0;
                    shared.last_fresh_time.store(ts, Ordering::Relaxed);
                    // Clear a watchdog-set freeze. Don't override the minimize gate,
                    // which owns `frozen` while the window is iconic.
                    if !shared.minimized.load(Ordering::Relaxed) {
                        if warned_static {
                            tracing::info!("capture: content moving again — freeze cleared");
                        }
                        shared.frozen.store(false, Ordering::Relaxed);
                        warned_static = false;
                    }
                } else {
                    // Byte-identical center patch — Valorant gameplay/menus always
                    // animate, so a static patch strongly implies a frozen capture.
                    same_frames += 1;
                    let stuck = last_change.elapsed();
                    let visible =
                        unsafe { IsWindowVisible(HWND(hwnd_raw as *mut c_void)).as_bool() };
                    if stuck >= STATIC_FLAG_AFTER
                        && visible
                        && !shared.minimized.load(Ordering::Relaxed)
                    {
                        shared.frozen.store(true, Ordering::Relaxed);
                        if !warned_static {
                            tracing::warn!(
                                stuck_secs = stuck.as_secs(),
                                same_frames,
                                "capture: center patch static while window visible — \
                                 capture appears frozen (stale swapchain?)"
                            );
                            warned_static = true;
                        }
                        // Escalate to a hook re-hook after a longer window, debounced
                        // so we don't spam restarts. The hook re-runs capture_init →
                        // re-signals HookReady → acquire() reopens the texture.
                        if stuck >= STATIC_RESTART_AFTER
                            && last_restart.map_or(true, |t| t.elapsed() >= RESTART_DEBOUNCE)
                        {
                            tracing::warn!(
                                "capture: requesting hook restart to recover frozen swapchain"
                            );
                            hook.request_restart();
                            last_restart = Some(Instant::now());
                            // Re-arm the change clock so we give the re-init a beat
                            // before re-evaluating (avoids back-to-back restarts).
                            last_change = Instant::now();
                        }
                    }
                }
            }
        }

        // Anchor for synthesizing minimized keep-alive timestamps, and a throttled
        // snapshot of the live frame for the freeze overlay to re-emit. The snapshot
        // is a cheap GPU→GPU copy a few times a second; no-op when the overlay is
        // inactive. `staging` is still owned here (handed off just below).
        last_ts = ts;
        last_ts_at = Instant::now();
        if shared.overlay_active.load(Ordering::Relaxed) && last_base_snap.elapsed() >= BASE_SNAPSHOT
        {
            if freeze_base.is_none() {
                // Same desc as the staging pool (BGRA, RENDER_TARGET), so it's a
                // valid copy source/target and matches what the overlay draws on.
                let mut bd = D3D11_TEXTURE2D_DESC::default();
                unsafe { staging.GetDesc(&mut bd) };
                let mut tex: Option<ID3D11Texture2D> = None;
                if unsafe { device.CreateTexture2D(&bd, None, Some(&mut tex)) }.is_ok() {
                    freeze_base = tex;
                }
            }
            if let Some(base) = &freeze_base {
                let snap = (|| -> WinResult<()> {
                    let dst: ID3D11Resource = base.cast()?;
                    let src: ID3D11Resource = staging.cast()?;
                    unsafe {
                        context.CopySubresourceRegion(&dst, 0, 0, 0, 0, &src, 0, None);
                    }
                    Ok(())
                })();
                if snap.is_ok() {
                    last_base_snap = Instant::now();
                }
            }
        }

        shared.last_handed_time.store(ts, Ordering::Relaxed);
        match filled_tx.try_send((staging, ts)) {
            Ok(()) => {
                shared.handed.fetch_add(1, Ordering::Relaxed);
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

/// Part B static-frame watchdog primitive: hash a `probe`×`probe` patch from the
/// center of `src` (a freshly-copied frame). Lazily creates a CPU-readable
/// staging texture in `readback` (same format as `src`), GPU-copies the center
/// sub-rect into it, maps it, and FNV-1a hashes the rows. Returns `None` (and
/// leaves capture untouched) on any D3D failure — the watchdog is best-effort and
/// must never break the frame loop. Assumes a 4-byte BGRA/RGBA pixel (all formats
/// the hook path supports); `RowPitch` handles any padding.
fn probe_center_hash(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    src: &ID3D11Texture2D,
    readback: &mut Option<ID3D11Texture2D>,
    width: u32,
    height: u32,
    probe: u32,
) -> Option<u64> {
    // Lazily build the readback texture, matching `src`'s exact format so the
    // copy is a same-format blit.
    if readback.is_none() {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { src.GetDesc(&mut src_desc) };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: probe,
            Height: probe,
            MipLevels: 1,
            ArraySize: 1,
            Format: src_desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        if unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }.is_err() {
            return None;
        }
        *readback = tex;
    }
    let dst = readback.as_ref()?;

    // Copy the center sub-rect (src → readback).
    let cx = (width - probe) / 2;
    let cy = (height - probe) / 2;
    let box_ = D3D11_BOX {
        left: cx,
        top: cy,
        front: 0,
        right: cx + probe,
        bottom: cy + probe,
        back: 1,
    };
    let dst_res: ID3D11Resource = dst.cast().ok()?;
    let src_res: ID3D11Resource = src.cast().ok()?;
    unsafe {
        context.CopySubresourceRegion(&dst_res, 0, 0, 0, 0, &src_res, 0, Some(&box_));
    }

    // Map and FNV-1a hash the patch rows, then unmap.
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    if unsafe { context.Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.is_err() {
        return None;
    }
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let row_bytes = (probe * 4) as usize;
    unsafe {
        let base = mapped.pData as *const u8;
        for row in 0..probe as usize {
            let row_ptr = base.add(row * mapped.RowPitch as usize);
            for &b in std::slice::from_raw_parts(row_ptr, row_bytes) {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        context.Unmap(&dst_res, 0);
    }
    Some(hash)
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
    overlay_capable: bool,
    filled_rx: Receiver<(ID3D11Texture2D, i64)>,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    shared: Arc<Shared>,
    clip: Arc<ClipBuffer>,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    // The encode thread must keep draining captured frames even while the game
    // pins the CPU, or the bounded hand-off channel backs up and the source loop
    // drops frames.
    crate::core::boost_current_thread_priority("encode");
    // Exempt from any process-level EcoQoS set while hidden to tray, so the convert
    // + hardware-encode path is never parked on an efficiency core mid-match.
    crate::core::protect_thread_high_qos("encode");

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
    // In-frame freeze overlay (the "tabbed out" card). Built on the capture device
    // — it draws onto the BGRA staging textures, which the convert reads. Built
    // whenever the capture is overlay-capable (not just when the feature is on),
    // so it can be toggled live; drawing is gated per-frame on `overlay_active`.
    // Built before readiness so `overlay_capable`/`overlay_active` are settled
    // before the source loop spawns (it gates its keep-alive emit on the latter).
    // A failure here is non-fatal: we just don't annotate frozen frames.
    let overlay = if overlay_capable {
        match overlay_card::FreezeOverlay::new(&capture_device) {
            Ok(o) => {
                shared.overlay_capable.store(true, Ordering::Release);
                shared
                    .overlay_active
                    .store(enc_cfg.freeze_overlay, Ordering::Release);
                Some(o)
            }
            Err(e) => {
                tracing::warn!("freeze overlay init failed; frozen frames won't be annotated: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut warned_overlay = false;

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
        // Tag these packets with the current liveness so the session writer can
        // mark frozen spans (minimized / stale-swapchain) for the cut to skip.
        let frozen = shared.frozen.load(Ordering::Relaxed);
        for p in pkts {
            clip.push(p, frozen);
        }
    };

    let mut idx = 0usize;
    let mut clock = MasterClock::new(fps);

    // Constant-frame-rate padding. The hook source loop samples the latest
    // backbuffer at the target interval, so when the game renders *below* the
    // target (e.g. a menu, or a static screen) some ticks repeat the same frame.
    // We fill the PTS gaps between real frames by
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

        // Frozen frame → stamp the "tabbed out" card onto the staging texture
        // before it's converted/encoded. Drawn here (not in the source loop) so a
        // single path covers both freeze cases: the static-watchdog freeze (live
        // staging frames keep flowing) and the minimized keep-alive emit (the
        // source loop re-sends the last frame). Gap-fill then repeats the carded
        // NV12, so the whole frozen stretch stays annotated.
        if let Some(o) = &overlay {
            if shared.frozen.load(Ordering::Relaxed)
                && shared.overlay_active.load(Ordering::Relaxed)
            {
                if let Err(e) = o.draw(&staging) {
                    if !warned_overlay {
                        tracing::warn!("freeze overlay draw failed (first occurrence): {e}");
                        warned_overlay = true;
                    }
                }
            }
        }

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
    // 300 ms while the dashboard is visible; backed off to 1 s while the main
    // window is hidden to tray (gameplay), where the emit is skipped anyway.
    let mut interval_ms = 300u64;
    // Also break when the source loop asks for a resize restart, so the capture
    // thread can relaunch the pipeline at the game's new resolution.
    while !stop.load(Ordering::Acquire) && !shared.resize_restart.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(interval_ms));
        // While hidden to tray during a match, nothing consumes capture-stats and
        // the renderer is suspended — so skip the per-tick IPC serialize + cross-
        // process post entirely. Baselines below still advance each tick, so the
        // first sample after the window reopens isn't a huge-dt spike.
        let visible = app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(true);
        interval_ms = if visible { 300 } else { 1000 };
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
        if visible {
            let _ = app.emit(events::CAPTURE_STATS, &stats);
        }
    }
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

}
