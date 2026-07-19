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
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::core::{Interface, Result as WinResult};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_FLAG_DO_NOT_WAIT,
    D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM,
    DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_B8G8R8X8_TYPELESS, DXGI_FORMAT_B8G8R8X8_UNORM,
    DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use crate::core::audio::{self, AudioCapture, AudioControl, AudioMeta};
use crate::core::buffer::{AudioRing, BufferStats, PacketRing};
use crate::core::clock::{MasterClock, TICKS_PER_SECOND};
use crate::core::convert::{self, Converter};
use crate::core::cursor_overlay;
use crate::core::device;
use crate::core::disk_buffer::DiskPacketRing;
use crate::core::encode::{EncodeSettings, EncodedPacket, Encoder};
use crate::core::hook::{HookCapture, RunningHook};
use crate::core::wgc::{self, WgcCapture};
use crate::core::mux::{self, AudioClip, ClipMeta};
use crate::core::overlay_card;
use crate::core::session::SessionWriter;
use crate::events;
use crate::settings::AudioConfig;

/// Number of BGRA staging textures shared between the hook source loop and the
/// encode thread. Also bounds in-flight frames (backpressure: the source loop
/// drops when none are free). Small — we only need to cover channel + encoder
/// latency.
mod window;
pub use window::{
    find_valorant_window, find_window_by_process, find_window_by_title, is_window_minimized,
    list_windows, pid_for_hwnd, window_title, WindowTarget,
};
use window::cursor_screen_pos;

const STAGING_POOL: usize = 4;
/// NV12 textures the encode thread cycles through. Must exceed how many surfaces
/// the encoder holds asynchronously (`async_depth` ≈ 1–2) so a reused texture is
/// never still in flight.
const NV12_RING: usize = 6;


/// Live capture + encode throughput, emitted as the `capture-stats` event.
#[derive(Debug, Clone, Serialize)]
pub struct CaptureStats {
    /// Handed-off (captured + copied) frames per second, after FPS pacing.
    pub fps: f64,
    /// Total frames handed off to the encode thread since start.
    pub frames: u64,
    /// Total frames the hook delivered (before pacing) since start.
    pub arrived: u64,
    /// Total frames skipped as byte-identical duplicates (Part B dirty check).
    pub skipped_dup: u64,
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
    /// Frames skipped by the Part B dirty-frame check (byte-identical to the
    /// previous tick — the game presented slower than `target_fps`). The encode
    /// thread's CFR gap-fill duplicates the last frame, so output stays smooth.
    skipped_dup: AtomicU64,
    width: AtomicU32,
    height: AtomicU32,
    /// SystemRelativeTime (100 ns units) of the last handed frame, for the cap.
    last_handed_time: AtomicI64,
    /// Compressed packets produced by the encode thread.
    enc_packets: AtomicU64,
    /// Total compressed bytes produced (for the bitrate readout).
    enc_bytes: AtomicU64,
    /// Frames the hardware encoder rejected (`avcodec_send_frame` errors). Nonzero
    /// with `enc_packets == 0` means clips for this session are empty — surfaced
    /// once (not per frame) so a wedged encoder can't flood the log, and read by
    /// the health summary to flag the failure.
    enc_errors: AtomicU64,
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
    /// True when the mouse cursor should be composited onto live frames: the
    /// `record_cursor` feature is **on** AND the capture is overlay-capable. Read
    /// per-frame by the encode thread (cursor draw gate) and per-tick by the source
    /// loop (so a moving cursor over a static scene isn't dirty-skipped), so
    /// toggling applies live via [`RunningCapture::set_record_cursor`].
    cursor_active: AtomicBool,
    /// True once the encode thread confirmed the cursor overlay can be drawn for
    /// this capture (D2D-targetable format AND `CursorOverlay` init succeeded).
    /// Fixed for the capture's life; gates the live `cursor_active` toggle.
    cursor_capable: AtomicBool,
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

/// Build a fresh audio-track set (one AAC ring per name, track 0 = master), each
/// ring sized to `retention_secs`. Shared by [`ClipBuffer::new`] and the live
/// layout swap ([`ClipBuffer::reset_audio_tracks`]).
fn build_audio_tracks(names: &[String], retention_secs: u32) -> Vec<AudioTrack> {
    names
        .iter()
        .map(|name| AudioTrack {
            name: name.clone(),
            ring: Mutex::new(AudioRing::new(retention_secs)),
            meta: OnceLock::new(),
        })
        .collect()
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
    /// audio is disabled; track 0 is the master mix. The *set* is swapped
    /// atomically on an audio-track-*layout* change (Separate tracks, mic on/off,
    /// mode switch — see [`Self::reset_audio_tracks`]); readers load an `Arc`
    /// snapshot so a save that races the swap sees a whole old-or-new set, never a
    /// torn one. Only the audio thread writes it (during its own restart, when it
    /// isn't pushing), so the swap is uncontended.
    audio_tracks: RwLock<Arc<Vec<AudioTrack>>>,
    /// Retention window (seconds) the per-track AAC rings are sized to — retained
    /// so a live layout swap can size the new rings the same as the originals.
    audio_retention_secs: u32,
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
        let audio_tracks = RwLock::new(Arc::new(build_audio_tracks(&audio_track_names, retention_secs)));
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
            audio_retention_secs: retention_secs,
            video_base: OnceLock::new(),
            session: Mutex::new(None),
        })
    }

    /// Load the current audio-track set (an `Arc` snapshot). Cheap; readers hold
    /// the snapshot for the duration of one operation so a concurrent layout swap
    /// can't tear it.
    fn audio_tracks(&self) -> Arc<Vec<AudioTrack>> {
        self.audio_tracks.read().unwrap().clone()
    }

    /// Replace the audio-track layout live for an audio-only restart (Separate
    /// tracks, mic on/off, mode switch, device add/remove). The new rings are sized
    /// to the same retention window as the originals. Callers MUST also stop/start
    /// the audio thread around this and MUST NOT call it while a Mode-B session is
    /// teeing — the session declares its stream count/indices at match start and
    /// can't absorb a mid-file change (see `commands::apply_audio_layout_change`).
    pub fn reset_audio_tracks(&self, names: &[String]) {
        let fresh = Arc::new(build_audio_tracks(names, self.audio_retention_secs));
        *self.audio_tracks.write().unwrap() = fresh;
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
        if let Some(track) = self.audio_tracks().get(track_idx) {
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
        self.audio_tracks()
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
        self.audio_tracks()
            .iter()
            .filter_map(|t| t.meta.get().map(|m| (t.name.clone(), m.clone())))
            .collect()
    }

    /// Number of audio tracks (0 ⇒ video-only).
    pub fn audio_track_count(&self) -> usize {
        self.audio_tracks().len()
    }

    /// Publish the muxing metadata (once, when the encoder is ready).
    fn set_meta(&self, meta: ClipMeta) {
        let _ = self.meta.set(meta);
    }

    /// Publish output track `idx`'s AAC stream metadata (once, when its encoder
    /// opens). No-op for an out-of-range index.
    pub fn set_audio_track_meta(&self, idx: usize, meta: AudioMeta) {
        if let Some(track) = self.audio_tracks().get(idx) {
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
        let (lo, hi) = packets.iter().fold((i64::MAX, i64::MIN), |(lo, hi), p| {
            (lo.min(p.pts), hi.max(p.pts))
        });
        let span_pts = (hi - lo).max(0) + 1; // +1 for the last frame's own duration

        // Audio: slice EVERY output track's AAC ring over the same wall-clock
        // window. The video clip starts at the wall-clock tick of its first
        // packet, derived from the shared video base anchor (PTS is in 1/fps
        // units off it). Track 0 (master mix) comes first; with "Separate audio
        // tracks" on, tracks 1..N are the per-source stems — each becomes its own
        // named MP4 audio stream via `write_clip`.
        let fps = meta.fps.max(1) as i64;
        let video_base = self.video_base.get().copied();
        // One snapshot of the track set for the whole slice — a concurrent layout
        // swap can't change the count/indices out from under us mid-save.
        let audio_tracks = self.audio_tracks();
        let mut track_slices: Vec<(usize, Vec<EncodedPacket>)> = Vec::new();
        if let Some(base) = video_base {
            let start_ticks = base + lo * TICKS_PER_SECOND / fps;
            let end_ticks = base + hi * TICKS_PER_SECOND / fps;
            for (i, track) in audio_tracks.iter().enumerate() {
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
                let track = &audio_tracks[*i];
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
    /// Only valid for `layout_eq` configs; a layout change restarts audio-only
    /// (see [`Self::reconfigure_audio_layout`]); an encode change restarts capture.
    audio_control: Arc<AudioControl>,
    /// The running audio thread handle, shared with the capture thread (which
    /// started it and tears it down at end). An audio-track-*layout* change stops
    /// this thread, resizes the clip's track set, and starts a fresh one — the
    /// video hook/encoder/ring stay untouched, so the game never sees a re-hook.
    /// `None` when capturing video-only.
    audio: Arc<Mutex<Option<AudioCapture>>>,
    /// The captured game's process id — needed to re-derive the audio plan (the
    /// `specific_apps` "Game Audio" source) on a live layout change.
    game_pid: Option<u32>,
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

    /// Apply an audio-track-*layout* change (Separate tracks, mic on/off, mode
    /// switch, PC-audio device add/remove) by restarting **only** the audio
    /// subsystem: stop the audio thread, resize the clip's track set to the new
    /// plan, then start a fresh audio thread. The video hook, encoder, and ring are
    /// untouched — the game never sees a re-injection. Handles 0→N (audio was off),
    /// N→M, and N→0 (audio fully disabled).
    ///
    /// The caller MUST ensure no Mode-B session is teeing into the clip: a session
    /// declares its audio stream count/indices at match start and can't absorb a
    /// mid-file track-count change (see `commands::apply_audio_layout_change`).
    pub fn reconfigure_audio_layout(&self, cfg: AudioConfig) {
        let names = audio::planned_track_names(&cfg, self.game_pid);
        let mut slot = match self.audio.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        // Stop the old thread first so it can't push into rings we're about to drop.
        if let Some(mut a) = slot.take() {
            a.stop();
        }
        // Swap the clip's track set, then seed the new config so the fresh thread
        // reads it as its initial plan.
        self.clip.reset_audio_tracks(&names);
        self.audio_control.reconfigure(cfg);
        *slot = if self.clip.audio_track_count() > 0 {
            AudioCapture::start(self.clip.clone(), self.audio_control.clone(), self.game_pid)
        } else {
            None
        };
        tracing::info!(
            tracks = names.len(),
            "settings: applied live audio layout change (no capture restart)"
        );
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

    /// Toggle "record mouse cursor" on the live capture without a restart — it's a
    /// per-frame flag. Clamped by cursor capability, so turning it on does nothing
    /// when the capture format can't be annotated.
    pub fn set_record_cursor(&self, on: bool) {
        let capable = self.shared.cursor_capable.load(Ordering::Acquire);
        self.shared
            .cursor_active
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

/// Enumerate visible top-level windows with a title (for the "Add a game" picker).
///
/// Windows owned by a process we'd never record — the smart games, the non-game
/// blacklist (browsers, Discord, launchers, the shell…), and Hako itself — are
/// filtered out, so the picker only lists plausible targets (Medal's
/// `GetAllActiveWindows` does the same). This mirrors the exclusion the generic
/// scan and `add_custom_game` already apply, so a window shown here can actually
/// be added. Windows whose owning process can't be resolved are kept (benefit of
/// the doubt).

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
#[allow(clippy::too_many_arguments)]
pub fn start_hook(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    buffer_secs: u32,
    disk_buffer_dir: Option<PathBuf>,
    audio: AudioConfig,
    enc_cfg: EncodeSettings,
    dirty_frame_skip: bool,
) -> std::result::Result<RunningCapture, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::default());
    let target_fps = target_fps.clamp(1, 480);
    let game_pid = pid_for_hwnd(hwnd_raw);
    let track_names = audio::planned_track_names(&audio, game_pid);
    let clip = ClipBuffer::new(
        target_fps,
        buffer_secs.clamp(5, 600),
        track_names,
        disk_buffer_dir,
    );
    // Shared live-audio control: the audio thread reads its initial config here
    // and re-reads it on a pushed volume change (no restart).
    let audio_control = AudioControl::new(audio);
    // The audio thread handle, shared between the capture thread (which starts it
    // and tears it down) and `RunningCapture` (which restarts it audio-only on a
    // layout change). `None` until the pipeline starts it (video-only ⇒ stays None).
    let audio_slot: Arc<Mutex<Option<AudioCapture>>> = Arc::new(Mutex::new(None));

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let thread = {
        let stop = stop.clone();
        let shared = shared.clone();
        let clip = clip.clone();
        let audio_control = audio_control.clone();
        let audio_slot = audio_slot.clone();
        std::thread::Builder::new()
            .name("hako-capture-hook".into())
            .spawn(move || {
                hook_capture_thread(
                    app,
                    hwnd_raw,
                    target_fps,
                    adapter_index,
                    audio_control,
                    audio_slot,
                    game_pid,
                    stop,
                    shared,
                    clip,
                    enc_cfg,
                    dirty_frame_skip,
                    ready_tx,
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
            audio: audio_slot,
            game_pid,
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("hook capture thread exited before signalling readiness".into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn hook_capture_thread(
    app: AppHandle,
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio_control: Arc<AudioControl>,
    audio_slot: Arc<Mutex<Option<AudioCapture>>>,
    game_pid: Option<u32>,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
    dirty_frame_skip: bool,
    ready_tx: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    let started = Instant::now();
    match run_hook_pipeline(
        hwnd_raw,
        target_fps,
        adapter_index,
        audio_control,
        audio_slot,
        game_pid,
        &stop,
        &shared,
        clip,
        enc_cfg,
        dirty_frame_skip,
    ) {
        Err(e) => {
            let _ = ready_tx.send(Err(e));
        }
        Ok(mut running) => {
            let _ = ready_tx.send(Ok(()));
            emit_loop(&app, target_fps, &stop, &shared);
            // Snapshot whether the USER stopped the capture *before* teardown: the
            // pipeline shares its `stop` flag with this session handle, and
            // `teardown()` sets it to end the source/encode threads — so a read after
            // teardown always looks stopped and could never tell a user stop from an
            // internal restart request. (This was a latent bug: it silently disabled
            // every restart below, since `!stop.load()` was always false post-teardown.)
            let user_stopped = stop.load(Ordering::Acquire);
            running.teardown();
            log_capture_health(&app, target_fps, &shared, started.elapsed());
            // A restart is requested when the game changed resolution/format mid-
            // capture (source loop) or the hardware encoder wedged and needs a fresh
            // session (encode thread self-heal). Rebuild from a detached thread: the
            // restart's `stop_capture_with` joins THIS capture thread, so this thread
            // must be free to return first. `start_capture_with` re-detects the game's
            // current size/format and builds a fresh clip buffer + encoder. A pending
            // user stop wins (don't resurrect a capture the user ended).
            if shared.resize_restart.load(Ordering::Acquire) && !user_stopped {
                let app = app.clone();
                std::thread::spawn(move || {
                    crate::commands::stop_capture_with(&app);
                    match crate::commands::start_capture_with(&app, hwnd_raw, None, None) {
                        Ok(()) => tracing::info!(
                            "capture: pipeline rebuilt (resolution/format change or encoder recovery)"
                        ),
                        Err(e) => tracing::warn!("capture: pipeline rebuild failed: {e}"),
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
    /// The audio thread handle, shared with `RunningCapture` (which can restart it
    /// audio-only on a layout change). Teardown stops it through this slot.
    audio: Arc<Mutex<Option<AudioCapture>>>,
}

impl RunningHookPipeline {
    fn teardown(&mut self) {
        if let Some(mut a) = self.audio.lock().ok().and_then(|mut g| g.take()) {
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

#[allow(clippy::too_many_arguments)]
/// The capture frame source feeding [`hook_source_loop`]: either the injected
/// OBS graphics hook (primary — captures at the game's real render rate) or
/// Windows.Graphics.Capture (the no-injection fallback). Both hand back
/// `(ID3D11Texture2D, ts)` where `ts` is a 100-ns timestamp in the same clock
/// domain, so the staging→convert→encode pipeline downstream is identical for
/// both and doesn't know which one it's fed by.
///
/// WGC is used only when the hook can't do the job: injection was blocked
/// (anti-cheat), the hook injected but never presented a frame, or WGC was
/// explicitly selected. It never displaces the hook on the games where the hook
/// works, so it can't regress the existing capture path.
enum FrameSource {
    Hook(RunningHook),
    Wgc(WgcCapture),
}

impl FrameSource {
    /// Sample the latest frame. `device` is used by the hook path to reopen the
    /// shared backbuffer; WGC ignores it (its pool is bound at `start`).
    fn acquire(
        &mut self,
        device: &ID3D11Device,
    ) -> std::result::Result<Option<(ID3D11Texture2D, i64)>, String> {
        match self {
            FrameSource::Hook(h) => h.acquire(device),
            FrameSource::Wgc(w) => w.acquire().map_err(|e| format!("wgc acquire: {e}")),
        }
    }

    /// Ask the source to re-establish capture of the current swapchain. The hook
    /// re-hooks (recovering a stale shared texture). WGC has no stale-swapchain
    /// failure mode — it keeps delivering across the minimize / fullscreen
    /// transitions that freeze the hook — so this is a deliberate no-op for it.
    fn request_restart(&mut self) {
        if let FrameSource::Hook(h) = self {
            h.request_restart();
        }
    }

    /// Short label for logs/metrics.
    fn kind(&self) -> &'static str {
        match self {
            FrameSource::Hook(_) => "hook",
            FrameSource::Wgc(_) => "wgc",
        }
    }

    /// Whether this is the injected graphics hook (vs the WGC fallback). The
    /// freeze watchdog only re-hooks / escalates for the hook — WGC has no
    /// stale-swapchain failure mode.
    fn is_hook(&self) -> bool {
        matches!(self, FrameSource::Hook(_))
    }
}

/// Persisted across a pipeline rebuild: the HWND whose graphics hook was found
/// unable to sustain capture — it kept freezing without recovering after repeated
/// re-hooks (e.g. League of Legends' churny swapchain). The next
/// [`start_frame_source`] for that window skips the hook and goes straight to
/// WGC, which captures via the compositor and doesn't suffer the stale-shtex
/// freeze. Keyed by HWND so a *different* game still tries the (cheaper, higher-
/// rate) hook first. `0` = none. A rebuilt pipeline reads this to stay on WGC
/// instead of re-hooking → freezing → escalating in a loop.
static FORCE_WGC_HWND: AtomicI64 = AtomicI64::new(0);

/// Mark `hwnd_raw` as hook-hostile so subsequent captures of it use WGC.
fn force_wgc_for(hwnd_raw: i64) {
    FORCE_WGC_HWND.store(hwnd_raw, Ordering::Release);
}

/// Whether `hwnd_raw` was previously marked hook-hostile (see [`force_wgc_for`]).
fn wgc_forced_for(hwnd_raw: i64) -> bool {
    hwnd_raw != 0 && FORCE_WGC_HWND.load(Ordering::Acquire) == hwnd_raw
}

/// Bring up a capture frame source for `hwnd`: try the injected graphics hook
/// first (the primary, real-render-rate path), and fall back to
/// Windows.Graphics.Capture when the hook can't inject or never presents a frame
/// (anti-cheat-blocked or hook-incompatible games). Returns the source plus the
/// first delivered frame's `D3D11_TEXTURE2D_DESC`, which sizes the staging pool +
/// converter/encoder.
///
/// The WGC fallback delivers BGRA8 SDR, so besides covering un-hookable games it
/// also sidesteps the `h264_nvenc`-won't-open-on-a-10-bit-HDR-backbuffer failure
/// that leaves a game "detected" but never clipped.
fn start_frame_source(
    hwnd: HWND,
    target_fps: u32,
    d3d_device: &ID3D11Device,
    stop: &Arc<AtomicBool>,
    force_wgc: bool,
) -> std::result::Result<(FrameSource, D3D11_TEXTURE2D_DESC), String> {
    // 1) Primary: the injected OBS graphics hook — unless this window was already
    //    found hook-hostile (froze without recovering), in which case skip
    //    straight to WGC so we don't re-hook → freeze → escalate on every rebuild.
    if force_wgc {
        tracing::info!(
            "capture: hook previously could not sustain this window; using WGC directly"
        );
    } else {
        match HookCapture::start(hwnd, target_fps) {
            Ok(hook) => {
                let mut source = FrameSource::Hook(hook);
                match wait_first_frame(&mut source, d3d_device, stop) {
                    Ok(desc) => return Ok((source, desc)),
                    Err(e) => {
                        // Injected but no frame in time (minimized, or anti-cheat
                        // block). Drop the hook and try WGC, which needs no injection.
                        tracing::warn!(
                            "capture: graphics hook delivered no frame ({e}); trying WGC fallback"
                        );
                        drop(source);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "capture: graphics hook injection failed ({e}); trying WGC fallback"
                );
            }
        }
    }

    // 2) Fallback: Windows.Graphics.Capture (no injection, HDR-tolerant).
    if !wgc::is_supported() {
        return Err(
            "graphics hook unavailable and WGC is not supported on this OS \
             (needs Windows 10 1903+/build 18362)"
                .into(),
        );
    }
    let wgc = WgcCapture::start(hwnd, d3d_device).map_err(|e| format!("wgc start: {e}"))?;
    let mut source = FrameSource::Wgc(wgc);
    let desc = wait_first_frame(&mut source, d3d_device, stop)
        .map_err(|e| format!("wgc produced no first frame: {e}"))?;
    tracing::info!("capture: using WGC fallback frame source (graphics hook unavailable)");
    Ok((source, desc))
}

/// Poll a freshly-started [`FrameSource`] until it delivers its first frame,
/// returning that frame's texture desc. Times out per [`HOOK_FIRST_FRAME_TIMEOUT`]
/// and bails early if `stop` is set.
fn wait_first_frame(
    source: &mut FrameSource,
    d3d_device: &ID3D11Device,
    stop: &Arc<AtomicBool>,
) -> std::result::Result<D3D11_TEXTURE2D_DESC, String> {
    let deadline = Instant::now() + HOOK_FIRST_FRAME_TIMEOUT;
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("capture stopped before the source produced a frame".into());
        }
        if let Some((tex, _ts)) = source.acquire(d3d_device)? {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            // SAFETY: `tex` is a live texture the source just handed us.
            unsafe { tex.GetDesc(&mut desc) };
            return Ok(desc);
        }
        if Instant::now() >= deadline {
            return Err("no frame delivered within the timeout".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn run_hook_pipeline(
    hwnd_raw: i64,
    target_fps: u32,
    adapter_index: Option<u32>,
    audio_control: Arc<AudioControl>,
    audio_slot: Arc<Mutex<Option<AudioCapture>>>,
    game_pid: Option<u32>,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    clip: Arc<ClipBuffer>,
    enc_cfg: EncodeSettings,
    dirty_frame_skip: bool,
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

    // Bring up a frame source (steps 1–9): the injected graphics hook first,
    // falling back to Windows.Graphics.Capture if the hook can't inject or never
    // presents a frame (anti-cheat-blocked / hook-incompatible games). The first
    // delivered frame's desc sizes the staging pool + converter/encoder.
    let hwnd = HWND(hwnd_raw as *mut c_void);
    let (source, first_desc) =
        start_frame_source(hwnd, target_fps, &d3d_device, stop, wgc_forced_for(hwnd_raw))?;
    tracing::info!(source = source.kind(), "capture: frame source up");

    let width = first_desc.Width & !1;
    let height = first_desc.Height & !1;
    if width < 2 || height < 2 {
        return Err(format!("hook reported an unusable size ({width}x{height})"));
    }
    // The captured/staging format (TYPELESS/sRGB backbuffers mapped to their typed
    // UNORM equivalent). Threaded into the converter (input color space) and into
    // the source loop (a mid-session change to it means the game toggled HDR /
    // recreated its swapchain, so the pipeline must rebuild — like a size change).
    let src_format = typed_capture_format(first_desc.Format);
    let hdr = convert::is_hdr_format(src_format);
    tracing::info!(
        format = first_desc.Format.0,
        typed_format = src_format.0,
        hdr,
        width,
        height,
        "hook: first backbuffer frame — format/size ({})",
        if hdr {
            "HDR backbuffer — tone-mapping to SDR for encode"
        } else {
            "SDR backbuffer"
        }
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
    let (enc_ready_tx, enc_ready_rx) =
        std::sync::mpsc::channel::<std::result::Result<(), String>>();
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
                    capture_device,
                    capture_context,
                    encode_device,
                    encode_context,
                    vendor,
                    width,
                    height,
                    src_format,
                    target_fps,
                    enc_cfg,
                    overlay_capable,
                    hwnd_raw,
                    filled_rx,
                    free_pool,
                    shared,
                    clip,
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
                    source,
                    hwnd_raw,
                    device,
                    context,
                    width,
                    height,
                    src_format,
                    target_fps,
                    dirty_frame_skip,
                    filled_tx,
                    free_pool,
                    shared,
                    source_stop,
                )
            })
            .map_err(|e| format!("spawn hook source thread: {e}"))?
    };

    if clip.audio_track_count() > 0 {
        match AudioCapture::start(clip.clone(), audio_control, game_pid) {
            Some(a) => {
                if let Ok(mut slot) = audio_slot.lock() {
                    *slot = Some(a);
                }
            }
            None => {
                tracing::warn!("audio capture requested but could not start; recording video only");
            }
        }
    }

    Ok(RunningHookPipeline {
        source_stop: stop.clone(),
        source_thread: Some(source_thread),
        encode_thread: Some(encode_thread),
        audio: audio_slot,
    })
}

/// The frame-source loop: pull a backbuffer from the [`FrameSource`] (hook or
/// WGC), copy its even sub-rect into a free staging texture, and send it on.
/// Owns the source so dropping at loop-end tears it down.
#[allow(clippy::too_many_arguments)]
fn hook_source_loop(
    mut source: FrameSource,
    hwnd_raw: i64,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    width: u32,
    height: u32,
    // The typed backbuffer format the staging pool + converter were built for. A
    // persistent change to it (an HDR toggle / swapchain recreation entering a
    // match) means the pinned copy→convert→encode chain no longer matches, so we
    // restart the pipeline the same way a resolution change does.
    src_format: DXGI_FORMAT,
    fps: u32,
    dirty_frame_skip: bool,
    filled_tx: SyncSender<(ID3D11Texture2D, i64)>,
    free_pool: Arc<Mutex<Vec<ID3D11Texture2D>>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) {
    // Exempt from process-level EcoQoS set while hidden to tray: this thread does
    // the per-frame shared-backbuffer copy (and the static-frame watchdog), so it
    // must stay on a performance core throughout a match.
    crate::core::protect_thread_high_qos("hook-source");
    // Raise this process's GPU scheduling priority once, so our capture/convert/
    // encode GPU work isn't starved behind the game's render queue. Process-wide
    // and idempotent, but guarded by a `Once` so a pipeline restart (resolution
    // change re-enters this loop, see the resize-restart path below) doesn't re-run
    // it and spam the log. Best-effort — never affects capture on failure.
    static GPU_PRIORITY_ONCE: std::sync::Once = std::sync::Once::new();
    GPU_PRIORITY_ONCE.call_once(|| crate::core::gpu_priority::raise_gpu_priority(&device));
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

    // ── Part B: dirty-frame probe + static-frame watchdog state ──────────────
    // Both are driven by one non-blocking per-tick content hash (see `DirtyProbe`)
    // of the live shared texture:
    //  • Dirty-frame skip: if this tick's frame is byte-identical to the previous
    //    one (the game presented slower than `target_fps`), skip the copy/convert/
    //    encode entirely — the encoder's CFR gap-fill duplicates the last frame so
    //    output stays smooth. Cuts redundant GPU work when the GPU is most
    //    contended. Gated by the `dirty_frame_skip` setting.
    //  • Static-frame watchdog: a non-minimized freeze (e.g. a stale swapchain
    //    after a fullscreen↔borderless switch) is invisible on the shtex path.
    //    ~Once a second we compare the hash; if it stops changing while the window
    //    is visible and not minimized we flag `frozen` and (after a longer window,
    //    debounced) re-hook the swapchain. Mirrors Medal's numSameFrames + regen.
    // Both are skipped for tiny windows (<probe), which aren't the freeze case.
    const STATIC_SAMPLE: Duration = Duration::from_secs(1);
    const STATIC_FLAG_AFTER: Duration = Duration::from_secs(3);
    const STATIC_RESTART_AFTER: Duration = Duration::from_secs(5);
    const RESTART_DEBOUNCE: Duration = Duration::from_secs(10);
    // After this many debounced re-hooks fail to un-freeze the swapchain (~this
    // many × RESTART_DEBOUNCE seconds of an unrecoverable freeze), give up on the
    // hook for this window and switch to WGC. Conservative: healthy capture never
    // re-hooks repeatedly without content moving again, so this can't trip on a
    // transient stale-swapchain blip (which recovers on the first re-hook).
    const FREEZE_WGC_ESCALATION: u32 = 2;
    let watchdog_ok = width >= DirtyProbe::PROBE && height >= DirtyProbe::PROBE;
    // Run the per-tick dirty probe whenever the window is big enough to hash; the
    // skip *action* is additionally gated on the `dirty_frame_skip` setting.
    let mut probe = watchdog_ok.then(|| DirtyProbe::new(width, height));
    // Cap consecutive skips so a pathological all-static scene (or a hash
    // collision) still forces a real frame through. Matched to the encode thread's
    // gap-fill cap (`max_gap_fill = fps`, ~1s): after this many skips the next real
    // frame's PTS is exactly `fps` slots ahead, which the gap-fill fully back-fills
    // with duplicates — so the output stays gap-free CFR. A larger cap (e.g. 2·fps)
    // would outrun the gap-fill and leave a PTS hole.
    let max_skips: u64 = fps.max(1) as u64;
    let mut consecutive_skips: u64 = 0;
    // Skip state: hash of the previous tick's frame (one-tick latency; see
    // `DirtyProbe`). Distinct from the watchdog's 1 Hz `last_hash`.
    let mut last_dirty_hash: Option<u64> = None;
    // Last mouse position, tracked only while "record cursor" is active: a moving
    // cursor over a byte-identical (static) backbuffer must still push a fresh
    // frame, or the composited pointer would freeze until the scene next changes.
    let mut last_cursor_pos: Option<(i32, i32)> = None;
    // Static watchdog state (sampled at STATIC_SAMPLE cadence).
    let mut last_hash: Option<u64> = None;
    let mut last_change = Instant::now();
    let mut last_static_sample = Instant::now();
    let mut last_restart: Option<Instant> = None;
    // Consecutive hook re-hook attempts since content last moved. If re-hooking
    // repeatedly fails to un-freeze the swapchain, the hook can't capture this
    // game's presentation → escalate to WGC. Reset whenever content moves again.
    let mut hook_restarts_since_progress: u32 = 0;
    let mut same_frames: u64 = 0;
    let mut warned_static = false;

    // Reconfigure-follow: confirm a new backbuffer size *or format* persists for a
    // few frames before restarting the pipeline to match, so a transient swapchain
    // blip (alt-tab, fullscreen↔borderless toggle, a one-frame recreate) doesn't
    // bounce capture. Mismatched frames are dropped until then. Format is tracked
    // alongside size because an SDR↔HDR flip (10-bit HDR toggled on entering a
    // match) recreates the swapchain with the *same* dimensions but a new format —
    // which the pinned staging pool / converter can't consume, so the copy or the
    // encode silently breaks until we rebuild at the new format.
    const RESIZE_CONFIRM_FRAMES: u32 = 8;
    let mut reconfig_pending: Option<(u32, u32, DXGI_FORMAT)> = None;
    let mut reconfig_frames: u32 = 0;

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
            source.request_restart();
            last_restart = Some(Instant::now());
            tracing::info!("capture: game restored — re-hooking swapchain, resuming live capture");
        }

        let frame = match source.acquire(&device) {
            Ok(Some(f)) => f,
            Ok(None) => {
                // Capture not initialized yet — wait for the source's first frame.
                continue;
            }
            Err(e) => {
                tracing::warn!("frame source acquire failed, stopping source: {e}");
                break;
            }
        };
        let (shared_tex, ts) = frame;
        shared.arrived.fetch_add(1, Ordering::Relaxed);
        shared.width.store(width, Ordering::Relaxed);
        shared.height.store(height, Ordering::Relaxed);

        // Follow the game's resolution *and* backbuffer format. Either can switch
        // mid-capture: a 16:9 menu → a 4:3 stretched match reopens a differently-
        // sized backbuffer, and toggling HDR (or a fullscreen transition entering a
        // match) reopens a differently-*formatted* one (e.g. 8-bit BGRA → 10-bit
        // R10G10B10A2). We record at whatever the game renders rather than padding
        // or mis-converting it. The clip's dimensions/format are fixed once it opens
        // (the buffer holds one config), so we can't change the output in place —
        // restart the pipeline at the new one. Confirm it holds for a few frames
        // first (a transient swapchain blip shouldn't bounce capture), and drop the
        // mismatched frames meanwhile — copying the old-sized box out of a resized
        // texture leaves a stale strip, and copying across a changed format is a
        // silent no-op (CopySubresourceRegion returns void) that would otherwise
        // wedge the encoder on garbage.
        let (live_w, live_h, live_fmt) = {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { shared_tex.GetDesc(&mut d) };
            (d.Width & !1, d.Height & !1, typed_capture_format(d.Format))
        };
        let size_ok = live_w >= 2 && live_h >= 2;
        let reconfigured =
            size_ok && (live_w != width || live_h != height || live_fmt != src_format);
        if reconfigured {
            match reconfig_pending {
                Some((pw, ph, pf)) if pw == live_w && ph == live_h && pf == live_fmt => {
                    reconfig_frames += 1
                }
                _ => {
                    reconfig_pending = Some((live_w, live_h, live_fmt));
                    reconfig_frames = 1;
                }
            }
            if reconfig_frames >= RESIZE_CONFIRM_FRAMES {
                tracing::info!(
                    from_w = width,
                    from_h = height,
                    from_format = src_format.0,
                    to_w = live_w,
                    to_h = live_h,
                    to_format = live_fmt.0,
                    to_hdr = convert::is_hdr_format(live_fmt),
                    "capture: backbuffer reconfigured mid-capture (size and/or format) \
                     — restarting to match"
                );
                shared.resize_restart.store(true, Ordering::Release);
                break;
            }
            continue;
        } else if reconfig_pending.is_some() {
            reconfig_pending = None;
            reconfig_frames = 0;
        }

        // ── Part B: per-tick dirty probe → static watchdog + duplicate skip ──
        // One non-blocking content hash of the live `shared_tex` drives both. It
        // reflects the *previous* tick's frame (one-tick latency; see DirtyProbe),
        // which is exactly right for skipping the second copy of a held frame.
        let cur_hash = probe
            .as_mut()
            .and_then(|p| p.sample(&device, &context, &shared_tex));

        // Static-freeze watchdog: sample the hash ~once a second. Runs *before* any
        // duplicate-skip `continue` below, so a genuine multi-second freeze is still
        // detected while frames are being skipped. `last_fresh_time`/`frozen` are
        // driven by content change, not mere handoff.
        if watchdog_ok && last_static_sample.elapsed() >= STATIC_SAMPLE {
            if let Some(hash) = cur_hash {
                last_static_sample = Instant::now();
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
                            // Forgive one prior re-hook per *recovered freeze*, rather
                            // than clearing the counter outright. The old hard reset
                            // made the WGC escalation unreachable for the failure it
                            // exists for: League's hook doesn't freeze permanently, it
                            // freezes, recovers for a few seconds on the re-hook, then
                            // refreezes — so every cycle zeroed the counter. Decaying
                            // per episode (not per healthy sample, which would zero it
                            // just as fast) still forgives an isolated blip, while a
                            // hook that refreezes faster than it recovers escalates.
                            hook_restarts_since_progress =
                                hook_restarts_since_progress.saturating_sub(1);
                        }
                        shared.frozen.store(false, Ordering::Relaxed);
                        warned_static = false;
                    }
                } else {
                    // All patches byte-identical — gameplay/menus always animate, so
                    // a static frame strongly implies a frozen capture.
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
                                "capture: content static while window visible — \
                                 capture appears frozen (stale swapchain?)"
                            );
                            warned_static = true;
                        }
                        // Escalate to a hook re-hook after a longer window, debounced
                        // so we don't spam restarts. The hook re-runs capture_init →
                        // re-signals HookReady → acquire() reopens the texture. Only
                        // the hook has a stale-swapchain to re-hook; WGC keeps
                        // delivering across the transitions that freeze the hook, so a
                        // static WGC frame means the *game* is idle, not a capture bug.
                        if source.is_hook()
                            && stuck >= STATIC_RESTART_AFTER
                            && last_restart.map_or(true, |t| t.elapsed() >= RESTART_DEBOUNCE)
                        {
                            // If re-hooking has already failed to recover the swapchain
                            // a couple of times, the hook simply can't capture this
                            // game's presentation (e.g. League's churny swapchain that
                            // wedges NVENC and leaves clips empty). Switch to WGC: mark
                            // the window hook-hostile and rebuild via the existing
                            // resize/restart path, which comes back up on WGC.
                            if hook_restarts_since_progress >= FREEZE_WGC_ESCALATION
                                && wgc::is_supported()
                            {
                                tracing::warn!(
                                    attempts = hook_restarts_since_progress,
                                    "capture: hook could not recover a frozen swapchain after \
                                     repeated re-hooks — switching to WGC fallback"
                                );
                                force_wgc_for(hwnd_raw);
                                shared.resize_restart.store(true, Ordering::Release);
                                break;
                            }
                            tracing::warn!(
                                "capture: requesting hook restart to recover frozen swapchain"
                            );
                            source.request_restart();
                            hook_restarts_since_progress += 1;
                            last_restart = Some(Instant::now());
                            // Re-arm the change clock so we give the re-init a beat
                            // before re-evaluating (avoids back-to-back restarts).
                            last_change = Instant::now();
                        }
                    }
                }
            }
            // `cur_hash == None` (map still-drawing / probe not yet primed) → leave
            // the 1 Hz clock un-advanced and retry next tick, so a real freeze isn't
            // masked by a missed sample.
        }

        // Duplicate-frame skip (Part B): when the setting is on and this frame is
        // byte-identical to the previous tick, skip the copy/convert/encode — the
        // encode thread's CFR gap-fill duplicates the last frame so output stays
        // smooth. Skipping here (before the pool pop) means a skipped tick never
        // touches the staging pool. The cap forces a real frame through every
        // ~2 s as insurance against a hash collision or an all-static scene.
        if dirty_frame_skip {
            // When recording the cursor, a pointer move over an otherwise static
            // frame must break the skip so the encode thread redraws it at the new
            // spot. No-op (and no syscall cost) when the feature is off.
            let cursor_moved = if shared.cursor_active.load(Ordering::Relaxed) {
                let pos = cursor_screen_pos();
                let moved = last_cursor_pos.is_some() && last_cursor_pos != pos;
                last_cursor_pos = pos;
                moved
            } else {
                last_cursor_pos = None;
                false
            };
            match cur_hash {
                Some(hash)
                    if !cursor_moved
                        && last_dirty_hash == Some(hash)
                        && consecutive_skips < max_skips =>
                {
                    consecutive_skips += 1;
                    shared.skipped_dup.fetch_add(1, Ordering::Relaxed);
                    // `last_dirty_hash` stays put (same content); loop back.
                    continue;
                }
                Some(hash) => {
                    consecutive_skips = 0;
                    last_dirty_hash = Some(hash);
                }
                None => {
                    // Unknown (still-drawing / not primed) → treat as changed.
                    consecutive_skips = 0;
                }
            }
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

        // Anchor for synthesizing minimized keep-alive timestamps, and a throttled
        // snapshot of the live frame for the freeze overlay to re-emit. The snapshot
        // is a cheap GPU→GPU copy a few times a second; no-op when the overlay is
        // inactive. `staging` is still owned here (handed off just below).
        last_ts = ts;
        last_ts_at = Instant::now();
        if shared.overlay_active.load(Ordering::Relaxed)
            && last_base_snap.elapsed() >= BASE_SNAPSHOT
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

    // Dropping the source here signals Stop + releases the keepalive mutex (hook)
    // or stops the WGC session, so the injected DLL self-terminates / the pool is
    // released. Dropping `filled_tx` ends the encode thread.
    drop(source);
}

/// Map a TYPELESS (or sRGB) backbuffer format to the fully-typed UNORM format the
/// VideoProcessor input view requires. The hook shares the backbuffer as TYPELESS
/// (because `allow_srgb_alias` lets the consumer choose UNORM vs sRGB), but
/// `CreateVideoProcessorInputView` rejects TYPELESS — and copying TYPELESS→UNORM
/// is legal since they share a format family. Unknown formats pass through.
fn typed_capture_format(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_B8G8R8A8_TYPELESS | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
            DXGI_FORMAT_B8G8R8A8_UNORM
        }
        DXGI_FORMAT_B8G8R8X8_TYPELESS => DXGI_FORMAT_B8G8R8X8_UNORM,
        DXGI_FORMAT_R8G8B8A8_TYPELESS | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
            DXGI_FORMAT_R8G8B8A8_UNORM
        }
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

/// Part B dirty-frame probe: a non-blocking per-tick content hash of the live
/// shared texture, used to both skip re-encoding byte-identical frames and drive
/// the static-freeze watchdog.
///
/// It hashes **3** small patches (center + one at ¼ and one at ¾ of each axis) so
/// a static center while the rest of the screen moves (loading screens, a menu
/// with only a corner spinner) isn't mistaken for a duplicate. All three sit
/// side-by-side in one `3·PROBE`-wide readback texture, so it's still one copy +
/// one map.
///
/// To avoid the GPU sync a blocking `Map`-after-copy would cost every tick, it
/// keeps a **ping-pong pair** of CPU-readable staging textures: each tick it
/// copies the patches into one buffer and maps the *other* (the copy issued last
/// tick) with `MAP_DO_NOT_WAIT`. That yields a hash with one tick of latency and
/// zero pipeline stalls. `WAS_STILL_DRAWING` (or any error / an un-primed buffer)
/// returns `None`, which callers treat as "changed" — never stall, never skip on
/// uncertainty.
struct DirtyProbe {
    /// Two `(PATCHES·PROBE)×PROBE` readback textures, ping-ponged.
    bufs: [Option<ID3D11Texture2D>; 2],
    /// Which buffer to write this tick; the other is mapped/read.
    idx: usize,
    /// Whether each buffer holds a valid prior copy yet.
    written: [bool; 2],
    /// Patch top-left source coords, clamped to fit inside the frame.
    patches: [(u32, u32); Self::PATCHES as usize],
}

impl DirtyProbe {
    const PROBE: u32 = 64;
    const PATCHES: u32 = 3;

    /// Precompute the patch layout for a `width`×`height` frame. Only valid when
    /// both dimensions are ≥ [`Self::PROBE`] (callers gate on `watchdog_ok`).
    fn new(width: u32, height: u32) -> Self {
        let p = Self::PROBE;
        let cx = |x: u32| x.min(width - p);
        let cy = |y: u32| y.min(height - p);
        let patches = [
            (cx((width - p) / 2), cy((height - p) / 2)), // center
            (cx(width / 4), cy(height / 4)),             // upper-left quadrant
            (cx(width * 3 / 4), cy(height * 3 / 4)),     // lower-right quadrant
        ];
        Self {
            bufs: [None, None],
            idx: 0,
            written: [false, false],
            patches,
        }
    }

    /// Copy this tick's patches into the write buffer, then read back the *other*
    /// buffer (last tick's copy) non-blockingly and FNV-1a hash it. Returns the
    /// hash of the previous tick's frame, or `None` if it isn't readable yet
    /// (still-drawing / not primed / any D3D failure) — best-effort, never breaks
    /// the loop.
    fn sample(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        src: &ID3D11Texture2D,
    ) -> Option<u64> {
        let probe = Self::PROBE;
        let stride = probe * Self::PATCHES;
        // Lazily build both readback textures, matching `src`'s exact format so the
        // copies are same-format blits.
        if self.bufs[0].is_none() {
            let mut src_desc = D3D11_TEXTURE2D_DESC::default();
            unsafe { src.GetDesc(&mut src_desc) };
            let desc = D3D11_TEXTURE2D_DESC {
                Width: stride,
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
            for b in self.bufs.iter_mut() {
                let mut tex: Option<ID3D11Texture2D> = None;
                if unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }.is_err() {
                    return None;
                }
                *b = tex;
            }
        }

        let write = self.idx;
        let read = 1 - self.idx;

        // Copy the 3 patches side-by-side into the write buffer.
        let write_res: ID3D11Resource = self.bufs[write].as_ref()?.cast().ok()?;
        let src_res: ID3D11Resource = src.cast().ok()?;
        for (i, &(px, py)) in self.patches.iter().enumerate() {
            let box_ = D3D11_BOX {
                left: px,
                top: py,
                front: 0,
                right: px + probe,
                bottom: py + probe,
                back: 1,
            };
            unsafe {
                context.CopySubresourceRegion(
                    &write_res,
                    0,
                    i as u32 * probe,
                    0,
                    0,
                    &src_res,
                    0,
                    Some(&box_),
                );
            }
        }
        self.written[write] = true;
        self.idx = read;

        // Read back the other buffer (last tick's copy). Not primed yet → None.
        if !self.written[read] {
            return None;
        }
        let read_res: ID3D11Resource = self.bufs[read].as_ref()?.cast().ok()?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // Non-blocking: never stall the source loop on the GPU copy.
        let hr = unsafe {
            context.Map(
                &read_res,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            )
        };
        if hr.is_err() {
            // WAS_STILL_DRAWING (or any error) → "changed" (None). Never wait.
            return None;
        }

        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let row_bytes = (stride * 4) as usize;
        unsafe {
            let base = mapped.pData as *const u8;
            for row in 0..probe as usize {
                let row_ptr = base.add(row * mapped.RowPitch as usize);
                for &b in std::slice::from_raw_parts(row_ptr, row_bytes) {
                    hash ^= b as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            context.Unmap(&read_res, 0);
        }
        Some(hash)
    }
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
    // Captured (staging) texture format — selects the converter's input color
    // space so an HDR backbuffer is tone-mapped rather than mislabeled as SDR.
    src_format: DXGI_FORMAT,
    fps: u32,
    enc_cfg: EncodeSettings,
    overlay_capable: bool,
    hwnd_raw: i64,
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
    let converter = match Converter::new(
        &capture_device,
        &capture_context,
        width,
        height,
        out_w,
        out_h,
        src_format,
    ) {
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

    // In-frame mouse-cursor compositor ("record cursor"). Same D2D-targetable
    // requirement as the freeze overlay (both draw onto the BGRA staging texture),
    // so it's *capable* exactly when `overlay_capable`; whether it draws is the
    // live `cursor_active` toggle (the `record_cursor` setting). The Game Capture
    // path shares the game's backbuffer, which lacks the Windows hardware cursor,
    // so this stamps the live pointer on. Non-fatal on failure.
    let cursor = if overlay_capable {
        match cursor_overlay::CursorOverlay::new(&capture_device) {
            Ok(c) => {
                shared.cursor_capable.store(true, Ordering::Release);
                shared
                    .cursor_active
                    .store(enc_cfg.record_cursor, Ordering::Release);
                Some(c)
            }
            Err(e) => {
                tracing::warn!("cursor overlay init failed; clips won't show the pointer: {e}");
                None
            }
        }
    } else {
        None
    };
    let mut warned_cursor = false;

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
    // Encode-error accounting. A wedged hardware encoder can reject *every* frame
    // (e.g. after an unhandled backbuffer/HDR change, or a driver hiccup); logging
    // each one buried a real 20 MB / 162k-line session in one identical warning and
    // silently produced empty clips. Track ok/err counts, warn once, then only
    // rarely, and shout once when it's clear the whole session is being dropped.
    let mut enc_ok: u64 = 0;
    let mut enc_err: u64 = 0;
    // Consecutive failures with no success between them. Reset on any encoded frame.
    // A sustained run means the hardware encoder is *wedged* (a swapchain recreation
    // entering a match, or a driver hiccup, left it rejecting every submit — NVENC
    // returns EINVAL once then EAGAIN forever because a failed `avcodec_send_frame`
    // never drained the queue). Unlike `declared_dead` (gated on lifetime `enc_ok ==
    // 0`), this catches a wedge that strikes *after* a long healthy stretch — the
    // real-world case: encoded fine for 7 min, then a mid-match swapchain recreate
    // wedged it and produced zero clips for the rest of the session.
    let mut consecutive_enc_err: u64 = 0;
    let mut warned_encode = false;
    let mut declared_dead = false;
    // Wedge recovery is tiered. Tier 1 reopens *just* the encoder in place, which
    // keeps the frame source, the clip buffer and — critically — the session
    // writer's timeline alive; a full pipeline rebuild discards the accumulated
    // `(wallclock, pts)` samples, and an event that lands on a timeline that was
    // torn down mid-match can't be placed at all (`TimelineIndex::pts_at_within`
    // returns None), so the match is silently cut to zero clips. Tier 2 fires only
    // when in-place reopens stop holding, which means the *input* is the problem
    // (the hook handing over a churny/stale swapchain) rather than the encoder
    // session: mark the window hook-hostile and rebuild on WGC.
    //
    // Reopens since the encoder last sustained a healthy run. Reset when a reopen
    // demonstrably held, so unrelated wedges hours apart don't accumulate.
    let mut encoder_reopens: u32 = 0;
    // `enc_ok` as of the last in-place reopen — the baseline for "did it hold?".
    let mut enc_ok_at_reopen: u64 = 0;
    // A reopen held if the encoder produced ~5s of good frames before wedging again.
    let healthy_run = (fps as u64).max(30) * 5;
    // Set when `note_encode_error` decides the encoder is wedged; acted on at the
    // end of the iteration so we never swap the encoder out mid gap-fill burst.
    let mut wedged = false;

    while let Ok((staging, ts)) = filled_rx.recv() {
        let nv12 = nv12_ring[idx % nv12_ring.len()].clone();
        idx += 1;

        // Frozen frame → stamp the "tabbed out" card onto the staging texture
        // before it's converted/encoded. Drawn here (not in the source loop) so a
        // single path covers both freeze cases: the static-watchdog freeze (live
        // staging frames keep flowing) and the minimized keep-alive emit (the
        // source loop re-sends the last frame). Gap-fill then repeats the carded
        // NV12, so the whole frozen stretch stays annotated.
        let frozen_now = shared.frozen.load(Ordering::Relaxed);
        if let Some(o) = &overlay {
            if frozen_now && shared.overlay_active.load(Ordering::Relaxed) {
                if let Err(e) = o.draw(&staging) {
                    if !warned_overlay {
                        tracing::warn!("freeze overlay draw failed (first occurrence): {e}");
                        warned_overlay = true;
                    }
                }
            }
        }
        // Live frame → stamp the mouse cursor on top of the captured backbuffer
        // (the hardware cursor isn't in it). Skipped while frozen: the "tabbed out"
        // card owns those frames, and the last-known cursor position would be stale.
        if let Some(c) = &cursor {
            if !frozen_now && shared.cursor_active.load(Ordering::Relaxed) {
                if let Err(e) = c.draw(&staging, hwnd_raw, width, height) {
                    if !warned_cursor {
                        tracing::warn!("cursor overlay draw failed (first occurrence): {e}");
                        warned_cursor = true;
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
                        Ok(pkts) => {
                            enc_ok += 1;
                            consecutive_enc_err = 0;
                            record(pkts);
                        }
                        Err(e) => {
                            wedged |= note_encode_error(
                                &e,
                                "gap-fill encode",
                                &shared,
                                enc_ok,
                                fps,
                                &mut enc_err,
                                &mut consecutive_enc_err,
                                &mut warned_encode,
                                &mut declared_dead,
                            );
                        }
                    }
                    fill += 1;
                }
            }
        }

        match encoder.encode(&nv12, pts) {
            Ok(pkts) => {
                enc_ok += 1;
                consecutive_enc_err = 0;
                record(pkts);
            }
            Err(e) => {
                wedged |= note_encode_error(
                    &e,
                    "encode",
                    &shared,
                    enc_ok,
                    fps,
                    &mut enc_err,
                    &mut consecutive_enc_err,
                    &mut warned_encode,
                    &mut declared_dead,
                );
            }
        }
        last_pts = Some(pts);
        prev_nv12 = Some(nv12);

        // Wedged encoder → recover. Tier 1 (reopen in place) preserves the session
        // timeline; tier 2 (rebuild on WGC) is the escalation when reopening stops
        // working. See the `encoder_reopens` declaration above.
        if wedged {
            wedged = false;
            // A reopen that produced a sustained healthy run before wedging again
            // did its job — this is a fresh, unrelated wedge, so don't let it count
            // toward escalating away from the hook.
            if enc_ok.saturating_sub(enc_ok_at_reopen) >= healthy_run {
                encoder_reopens = 0;
            }
            if encoder_reopens >= ENCODER_REOPEN_ESCALATION {
                tracing::error!(
                    reopens = encoder_reopens,
                    "capture: the encoder wedged again right after {encoder_reopens} in-place \
                     reopens — the frames themselves are the problem (the graphics hook is \
                     handing over a swapchain the encoder won't accept), not the encoder \
                     session. Marking this window hook-hostile and rebuilding on WGC."
                );
                if wgc::is_supported() {
                    force_wgc_for(hwnd_raw);
                }
                shared.resize_restart.store(true, Ordering::Release);
                break;
            }
            match reopen_encoder(
                &encode_device,
                &encode_context,
                encode_vendor,
                &enc_cfg,
                out_w,
                out_h,
                fps,
                &clip,
            ) {
                Ok(fresh) => {
                    encoder = fresh;
                    encoder_reopens += 1;
                    enc_ok_at_reopen = enc_ok;
                    consecutive_enc_err = 0;
                    // The fresh encoder opens a new bitstream; its first frame is an
                    // IDR, so packets already in the ring splice cleanly onto it.
                    // Drop the duplicate-source surface so gap-fill doesn't re-encode
                    // a pre-reopen frame across the discontinuity.
                    prev_nv12 = None;
                    tracing::info!(
                        reopens = encoder_reopens,
                        "capture: reopened the hardware encoder in place — capture, clip buffer \
                         and the session timeline are untouched, so the match keeps recording"
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "capture: could not reopen the encoder in place ({e}) — falling back to a \
                         full pipeline rebuild on WGC"
                    );
                    if wgc::is_supported() {
                        force_wgc_for(hwnd_raw);
                    }
                    shared.resize_restart.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }

    // Channel closed (capture stopped): flush the encoder and exit.
    if let Ok(pkts) = encoder.flush() {
        record(pkts);
    }
    if enc_err > 0 {
        tracing::warn!(
            encoded_ok = enc_ok,
            encode_errors = enc_err,
            "hako-encode thread exiting with encode errors — some or all frames were dropped"
        );
    }
    tracing::info!("hako-encode thread exiting");
}

/// Record and rate-limit a hardware-encode failure, and self-heal a wedged encoder.
///
/// A wedged encoder can reject every frame; without rate-limiting that buried one
/// session in 162k identical warnings (20 MB) while silently producing empty
/// clips. This warns once, then only every 600th error, and — the first time it's
/// clear the whole session is being dropped (many errors, zero successes) — logs a
/// single ERROR naming the likely cause. `enc_errors` on `Shared` also lets the
/// health summary flag the failure.
///
/// Beyond logging, a sustained run of `consecutive` failures (no encoded frame
/// between them) means the encoder is genuinely wedged — commonly a mid-match
/// swapchain recreation (a game entering a match / toggling HDR or fullscreen)
/// that left NVENC returning EINVAL-then-EAGAIN with no way to drain. Returns
/// `true` exactly once per wedge episode so the caller can recover; the caller
/// owns the recovery *policy* (reopen in place vs. rebuild on WGC). Firing on the
/// exact threshold rather than latching means a wedge that recurs after a
/// successful recovery (which resets `consecutive`) is reported again. This fires
/// even after a long healthy stretch (it counts a *run*, not lifetime successes),
/// unlike the zero-successes check.
#[allow(clippy::too_many_arguments)]
#[must_use]
fn note_encode_error(
    err: &str,
    context: &str,
    shared: &Shared,
    enc_ok: u64,
    fps: u32,
    enc_err: &mut u64,
    consecutive: &mut u64,
    warned: &mut bool,
    declared_dead: &mut bool,
) -> bool {
    *enc_err += 1;
    *consecutive += 1;
    shared.enc_errors.fetch_add(1, Ordering::Relaxed);
    if !*warned {
        tracing::warn!(
            "{context} error (first occurrence; identical errors are rate-limited): {err}"
        );
        *warned = true;
    } else if *enc_err % 600 == 0 {
        tracing::warn!(
            total_encode_errors = *enc_err,
            "{context} still failing (rate-limited): {err}"
        );
    }
    if !*declared_dead && enc_ok == 0 && *enc_err >= 120 {
        tracing::error!(
            dropped_frames = *enc_err,
            "capture: the hardware encoder has rejected every frame so far and produced none — \
             clips for this session will be EMPTY. This typically follows a backbuffer format / HDR \
             change the encoder couldn't consume, or a GPU/driver encode failure. The pipeline \
             self-heals below; if it persists the encoder or driver is refusing the input."
        );
        *declared_dead = true;
    }
    // Self-heal a wedged encoder: ~1s of unbroken failures (≥30 even at low fps) is
    // never normal steady state — `encode()` drains after every successful send, so
    // a healthy pipeline won't accumulate consecutive send failures. `==` (not `>=`)
    // reports each episode exactly once: `consecutive` only ever increments by one
    // per call and is reset to 0 by any encoded frame.
    let heal_threshold = (fps as u64).max(30);
    if *consecutive == heal_threshold {
        tracing::error!(
            consecutive_errors = *consecutive,
            "capture: hardware encoder wedged (every frame rejected for ~1s straight) — recovering \
             with a fresh encoder session. Usually follows a mid-match swapchain recreation \
             (entering a match / HDR or fullscreen toggle) that put NVENC in a bad state; without \
             this, clips stay empty for the rest of the session."
        );
        return true;
    }
    false
}

/// How many in-place encoder reopens may fail to hold before we stop blaming the
/// encoder session and blame the frames being fed to it. Past this, the graphics
/// hook is handing over a swapchain the encoder won't accept (League of Legends
/// recreates its swapchain constantly, and each recreation wedged NVENC within
/// seconds), so the window is marked hook-hostile and the pipeline rebuilds on WGC
/// — which delivers BGRA8 SDR through the compositor and sidesteps the failure.
const ENCODER_REOPEN_ESCALATION: u32 = 2;

/// Build a replacement [`Encoder`] with the *same* parameters as the wedged one,
/// for in-place recovery that leaves capture and the session timeline running.
///
/// The clip buffer's [`ClipMeta`] is a `OnceLock` published when the first encoder
/// opened, and every packet already in the ring was produced against it — so a
/// replacement is only safe if it agrees on codec, dimensions and codec-config
/// record. `Encoder::new` can fall back across codecs when one won't open, so this
/// is a real possibility, not a formality: if the fresh encoder disagrees, splicing
/// its packets onto the ring would produce a corrupt clip. Reject it and let the
/// caller do a full rebuild, which allocates a fresh buffer and meta.
#[allow(clippy::too_many_arguments)]
fn reopen_encoder(
    encode_device: &ID3D11Device,
    encode_context: &ID3D11DeviceContext,
    encode_vendor: device::Vendor,
    enc_cfg: &EncodeSettings,
    out_w: u32,
    out_h: u32,
    fps: u32,
    clip: &ClipBuffer,
) -> std::result::Result<Encoder, String> {
    let fresh = Encoder::new(
        encode_device,
        encode_context,
        encode_vendor,
        enc_cfg.codec,
        enc_cfg.bitrate_mbps,
        out_w,
        out_h,
        fps,
    )?;
    // No meta published yet ⇒ nothing in the ring was encoded against it, so any
    // encoder is acceptable (this can't normally happen: meta is set right after
    // the first encoder opens, before a frame is ever submitted).
    let Some(meta) = clip.clip_meta() else {
        return Ok(fresh);
    };
    let codec_id = fresh.codec().av_codec_id();
    if codec_id != meta.codec_id
        || fresh.width() != meta.width
        || fresh.height() != meta.height
        || fresh.extradata() != meta.extradata
    {
        return Err(format!(
            "replacement encoder doesn't match the stream already in the clip buffer \
             (codec {codec_id} vs {}, {}x{} vs {}x{}, extradata {} vs {} bytes)",
            meta.codec_id,
            fresh.width(),
            fresh.height(),
            meta.width,
            meta.height,
            fresh.extradata().len(),
            meta.extradata.len()
        ));
    }
    Ok(fresh)
}

fn log_capture_health(app: &AppHandle, target_fps: u32, shared: &Shared, elapsed: Duration) {
    let secs = elapsed.as_secs_f64().max(0.001);
    let arrived = shared.arrived.load(Ordering::Relaxed);
    let handed = shared.handed.load(Ordering::Relaxed);
    let skipped = shared.skipped_dup.load(Ordering::Relaxed);
    let encoded = shared.enc_packets.load(Ordering::Relaxed);
    let enc_errors = shared.enc_errors.load(Ordering::Relaxed);
    let bytes = shared.enc_bytes.load(Ordering::Relaxed);
    let elapsed_label = format!("{secs:.1}");
    let encoded_fps = encoded as f64 / secs;
    let handed_fps = handed as f64 / secs;
    let handed_fps_label = format!("{handed_fps:.1}");
    let encoded_fps_label = format!("{encoded_fps:.1}");
    let skipped_pct = if arrived > 0 {
        skipped as f64 * 100.0 / arrived as f64
    } else {
        0.0
    };
    let mbits = bytes as f64 * 8.0 / 1_000_000.0;
    let skipped_pct_label = format!("{skipped_pct:.1}");
    let mbits_label = format!("{mbits:.1}");
    let bg_governor = app
        .try_state::<crate::commands::SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.pause_background_while_gaming))
        .unwrap_or(true);
    let overlay_active = shared.overlay_active.load(Ordering::Relaxed);

    tracing::info!(
        target_fps,
        elapsed_secs = %elapsed_label,
        arrived,
        handed,
        encoded,
        handed_fps = %handed_fps_label,
        encoded_fps = %encoded_fps_label,
        duplicate_skip_pct = %skipped_pct_label,
        encoded_mbits = %mbits_label,
        encode_errors = enc_errors,
        background_governor = bg_governor,
        overlay_active,
        "capture health summary"
    );

    // Frames arriving but nothing (or almost nothing) encoding is the empty-clip
    // signature — flag it distinctly from a merely slow encoder so the log points
    // at the encode path rather than looking healthy.
    if secs >= 5.0 && enc_errors > 0 && encoded == 0 {
        tracing::error!(
            enc_errors,
            handed,
            "capture health: the encoder produced ZERO frames while rejecting {enc_errors} — \
             clips will be empty (likely an HDR/format the encoder can't consume)"
        );
    }

    if secs >= 5.0 && encoded > 0 && encoded_fps < target_fps as f64 * 0.80 {
        tracing::warn!(
            target_fps,
            encoded_fps = %encoded_fps_label,
            "capture health: encoder output was well below target; lower FPS/resolution/bitrate if clips look choppy"
        );
    }
    if target_fps > 120 {
        tracing::warn!(
            target_fps,
            "capture health: very high capture FPS can compete with the game; 60 FPS is the safest low-overhead default"
        );
    }
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
            skipped_dup: shared.skipped_dup.load(Ordering::Relaxed),
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

    /// A sustained run of encode failures after a healthy stretch (the real
    /// 7-min-then-wedge case: `enc_ok` large, not zero) must report the wedge once
    /// ~1s of unbroken failures accumulates — and a single success in between must
    /// reset the run so a transient blip never triggers recovery.
    #[test]
    fn wedged_encoder_reports_once_after_a_healthy_stretch() {
        let shared = Shared::default();
        let fps = 60u32;
        let heal_at = (fps as u64).max(30);
        let enc_ok = 20_000u64; // long healthy stretch already encoded
        let (mut enc_err, mut consec) = (0u64, 0u64);
        let (mut warned, mut dead) = (false, false);
        let mut note = |consec: &mut u64, enc_err: &mut u64| {
            note_encode_error(
                "avcodec_send_frame(nvenc): Resource temporarily unavailable",
                "encode",
                &shared,
                enc_ok,
                fps,
                enc_err,
                consec,
                &mut warned,
                &mut dead,
            )
        };

        // A transient burst that stops short of the threshold, then a success,
        // must NOT report a wedge (consecutive resets on the encoded frame).
        for _ in 0..(heal_at - 1) {
            assert!(!note(&mut consec, &mut enc_err), "no wedge before threshold");
        }
        consec = 0; // an encoded frame landed → run resets

        // Now a solid wedge: threshold consecutive failures with no success. It
        // must report exactly once, on the threshold hit.
        let mut reports = 0;
        for _ in 0..(heal_at * 3) {
            if note(&mut consec, &mut enc_err) {
                reports += 1;
            }
        }
        assert_eq!(reports, 1, "a wedge episode must be reported exactly once");

        // Recovery landed a frame (consecutive resets); a *later* wedge is a fresh
        // episode and must be reported again — the old latch suppressed this, so a
        // second wedge in one session went unrecovered.
        consec = 0;
        let mut reports = 0;
        for _ in 0..(heal_at * 3) {
            if note(&mut consec, &mut enc_err) {
                reports += 1;
            }
        }
        assert_eq!(reports, 1, "a wedge recurring after recovery must report again");

        // `enc_ok` never hit zero, so the empty-clip cold-start path stays quiet.
        assert!(!dead, "declared_dead is for zero-success sessions, not this one");
    }

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
