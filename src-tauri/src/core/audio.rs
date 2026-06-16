//! Desktop (WASAPI loopback) + microphone capture → mix → AAC.
//!
//! Two shared-mode WASAPI capture clients run on one dedicated thread:
//! - **Loopback** of the default render endpoint (`eRender`) — everything the
//!   user hears: the game, Discord, music. Shared-mode loopback can't be
//!   event-driven, so we poll (`GetNextPacketSize`/`GetBuffer`/`ReleaseBuffer`).
//! - **Microphone** of the default capture endpoint (`eCapture`). Optional —
//!   if it can't be opened we keep going with desktop audio only.
//!
//! Both are mixed into one 48 kHz stereo track and AAC-encoded, ready for
//! [`crate::core::mux`] to interleave alongside the H.264 video.
//!
//! ## Sync (the hard part)
//! Each `GetBuffer` reports the QPC time of its first sample (`pu64QPCPosition`).
//! WGC video frames carry `SystemRelativeTime`, also QPC-derived. Both are
//! placed on one **absolute 48 kHz sample timeline** keyed by QPC, so:
//! - **drift** between the two audio device clocks is absorbed (samples land at
//!   their true time, not at a naive running count), and
//! - **loopback silence** (which delivers *no packets*, not zeros) becomes a
//!   gap on the timeline that we fill with silence.
//!
//! The QPC unit (raw ticks vs 100 ns — the docs and the wild disagree) is
//! **auto-detected at runtime** ([`detect_qpc_scale`]) by comparing the first
//! reported position against `QueryPerformanceCounter`/`Frequency`, then
//! everything is converted to 100 ns ticks — the same unit as the video clock
//! (`clock::TICKS_PER_SECOND`) — so audio and video share one timeline.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use rusty_ffmpeg::ffi;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::core::capture::ClipBuffer;
use crate::core::clock::TICKS_PER_SECOND;
use crate::core::encode::{av_err, EncodedPacket};

/// Mixed-track sample rate. 48 kHz is the WASAPI shared-mode engine default and
/// the standard for AAC, so the common case needs no rate conversion.
const MIX_RATE: i32 = 48_000;
/// Mixed track is always stereo (game audio is; mic is upmixed if mono).
const MIX_CHANNELS: i32 = 2;
/// AAC target bitrate (stereo music/voice mix). Generous, like the video path.
const AAC_BITRATE: i64 = 160_000;
/// Poll cadence. WASAPI shared-mode period is ~10 ms; half that keeps latency
/// low without busy-waiting.
const POLL_MS: u64 = 5;
/// How far behind the newest sample we let the mixer lag before emitting a
/// block, so both sources have arrived (their packets land at slightly
/// different wall-clock instants). 200 ms is inaudible and well within sync.
const LATENCY_SAMPLES: i64 = MIX_RATE as i64 / 5;
/// A jump larger than this (on the absolute timeline) between a source's
/// expected position and where we last wrote is treated as a real gap →
/// resync + fill silence, rather than smoothed over (which would desync).
const GAP_SAMPLES: i64 = MIX_RATE as i64 / 20; // 50 ms

// FFmpeg error sentinels (mirrors encode.rs; AVERROR(e) == -e on every target).
const AVERROR_EAGAIN: i32 = -(ffi::EAGAIN as i32);
const AVERROR_EOF: i32 =
    -((b'E' as i32) | ((b'O' as i32) << 8) | ((b'F' as i32) << 16) | ((b' ' as i32) << 24));
/// `AV_CODEC_FLAG_GLOBAL_HEADER` — emit AudioSpecificConfig in `extradata`
/// (the MP4 `esds`) instead of in-band, so stream-copy muxing is decodable.
const AV_CODEC_FLAG_GLOBAL_HEADER: i32 = 1 << 22;

/// Everything the muxer needs to add the AAC stream that isn't per-packet:
/// built once when the AAC encoder opens. Mirrors [`crate::core::mux::ClipMeta`].
#[derive(Debug, Clone)]
pub struct AudioMeta {
    pub sample_rate: u32,
    pub channels: u32,
    /// AudioSpecificConfig from the encoder (`AV_CODEC_FLAG_GLOBAL_HEADER`).
    pub extradata: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// A running audio-capture session. Drop or [`stop`](Self::stop) to tear down.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Start desktop + mic capture, pushing AAC packets into `clip`'s audio ring
    /// and publishing [`AudioMeta`] once the encoder opens. Returns `None` if no
    /// usable capture device could be opened (caller proceeds video-only).
    ///
    /// Never blocks the caller meaningfully: setup happens on the audio thread
    /// and failures are logged — audio is best-effort relative to the recorder.
    pub fn start(clip: Arc<ClipBuffer>) -> Option<AudioCapture> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("hako-audio".into())
                .spawn(move || audio_thread(clip, stop))
                .ok()?
        };
        Some(AudioCapture {
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Thread entry
// ---------------------------------------------------------------------------

fn audio_thread(clip: Arc<ClipBuffer>, stop: Arc<AtomicBool>) {
    // Audio runs on its own COM apartment (MTA), independent of the capture
    // thread's WinRT init.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if let Err(e) = run_audio(&clip, &stop) {
        tracing::warn!("audio capture disabled: {e}");
    }
    unsafe {
        CoUninitialize();
    }
}

fn run_audio(clip: &Arc<ClipBuffer>, stop: &Arc<AtomicBool>) -> Result<(), String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;

        // Desktop audio is the priority; the mic is optional.
        let mut sources: Vec<Source> = Vec::new();
        match Source::open_loopback(&enumerator) {
            Ok(s) => sources.push(s),
            Err(e) => tracing::warn!("desktop-audio loopback unavailable: {e}"),
        }
        match Source::open_mic(&enumerator) {
            Ok(s) => sources.push(s),
            Err(e) => tracing::info!("microphone unavailable (continuing without): {e}"),
        }
        if sources.is_empty() {
            return Err("no audio capture devices could be opened".into());
        }

        let mut encoder = AacEncoder::new()?;
        clip.set_audio_meta(AudioMeta {
            sample_rate: MIX_RATE as u32,
            channels: MIX_CHANNELS as u32,
            extradata: encoder.extradata(),
        });
        let block = encoder.frame_size();

        for s in &sources {
            s.start()?;
        }

        let qpc_freq = {
            let mut f = 0i64;
            QueryPerformanceFrequency(&mut f).ok();
            f.max(1)
        };

        let mut mixer = Mixer::new(block);
        let mut scratch = Vec::<f32>::new();
        let mut zero = Vec::<u8>::new();

        while !stop.load(Ordering::Acquire) {
            let mut got_any = false;
            for src in &mut sources {
                got_any |= src.drain(&mut mixer, qpc_freq, &mut scratch, &mut zero);
            }
            // Emit any blocks both sources have now covered.
            for (samples, pts_ticks) in mixer.drain_ready(false) {
                for p in encoder.encode_block(&samples, pts_ticks)? {
                    clip.push_audio(p);
                }
            }
            if !got_any {
                std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
            }
        }

        // Stop devices, flush the remaining mixed tail, then flush the encoder.
        for s in &sources {
            let _ = s.audio_client.Stop();
        }
        for (samples, pts_ticks) in mixer.drain_ready(true) {
            for p in encoder.encode_block(&samples, pts_ticks)? {
                clip.push_audio(p);
            }
        }
        for p in encoder.flush()? {
            clip.push_audio(p);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// One WASAPI capture source (loopback or mic)
// ---------------------------------------------------------------------------

/// Sample layout of a source's `GetBuffer` data, parsed from its mix format.
#[derive(Clone, Copy)]
struct SrcFormat {
    rate: i32,
    channels: u16,
    /// `AVSampleFormat` for swr input (packed: FLT / S16 / S32). Shared mode is
    /// almost always float, but we detect to be safe.
    av_fmt: i32,
    /// Bytes per audio frame (`nBlockAlign`) — to size the silence scratch.
    block_align: u16,
}

struct Source {
    audio_client: IAudioClient,
    capture: IAudioCaptureClient,
    fmt: SrcFormat,
    swr: *mut ffi::SwrContext,
    /// Next absolute 48 kHz sample index this source will write to the mix.
    next_idx: i64,
    started: bool,
    label: &'static str,
}

impl Source {
    unsafe fn open_loopback(enumerator: &IMMDeviceEnumerator) -> Result<Source, String> {
        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("default render endpoint: {e}"))?;
        Source::activate(device, AUDCLNT_STREAMFLAGS_LOOPBACK, "desktop")
    }

    unsafe fn open_mic(enumerator: &IMMDeviceEnumerator) -> Result<Source, String> {
        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|e| format!("default capture endpoint: {e}"))?;
        Source::activate(device, 0, "mic")
    }

    /// Activate an `IAudioClient` in shared mode (optionally loopback), parse its
    /// mix format and build the resampler to the mixed 48 kHz stereo float track.
    unsafe fn activate(
        device: IMMDevice,
        stream_flags: u32,
        label: &'static str,
    ) -> Result<Source, String> {
        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioClient ({label}): {e}"))?;

        let wf = audio_client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat ({label}): {e}"))?;
        if wf.is_null() {
            return Err(format!("GetMixFormat returned null ({label})"));
        }
        let fmt = parse_format(wf);

        // 0 buffer duration/periodicity → engine default; loopback can't be
        // event-driven, so no EVENTCALLBACK flag — we poll.
        let init = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            stream_flags,
            0,
            0,
            wf,
            None,
        );
        CoTaskMemFree(Some(wf as *const _));
        init.map_err(|e| format!("IAudioClient::Initialize ({label}): {e}"))?;

        let capture: IAudioCaptureClient = audio_client
            .GetService()
            .map_err(|e| format!("GetService(IAudioCaptureClient) ({label}): {e}"))?;

        let swr = build_resampler(&fmt)
            .map_err(|e| format!("resampler ({label}): {e}"))?;

        Ok(Source {
            audio_client,
            capture,
            fmt,
            swr,
            next_idx: 0,
            started: false,
            label,
        })
    }

    unsafe fn start(&self) -> Result<(), String> {
        self.audio_client
            .Start()
            .map_err(|e| format!("IAudioClient::Start ({}): {e}", self.label))
    }

    /// Drain all currently-available packets into the mixer. Returns whether any
    /// packet was processed (so the loop can sleep when both sources are idle).
    unsafe fn drain(
        &mut self,
        mixer: &mut Mixer,
        qpc_freq: i64,
        scratch: &mut Vec<f32>,
        zero: &mut Vec<u8>,
    ) -> bool {
        let mut any = false;
        loop {
            let avail = match self.capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            if avail == 0 {
                break;
            }

            let mut data: *mut u8 = ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            let mut qpc_pos: u64 = 0;
            if self
                .capture
                .GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    None,
                    Some(&mut qpc_pos),
                )
                .is_err()
            {
                break;
            }

            if frames > 0 {
                let tick = mixer.qpc_to_ticks(qpc_pos, qpc_freq);
                let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let discontinuity =
                    (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;

                // Resample this packet (or an equivalent run of silence) to
                // 48 kHz stereo interleaved float.
                if silent || data.is_null() {
                    let bytes = frames as usize * self.fmt.block_align as usize;
                    if zero.len() < bytes {
                        zero.resize(bytes, 0);
                    }
                    self.resample(zero.as_ptr(), frames as i32, scratch);
                } else {
                    self.resample(data, frames as i32, scratch);
                }
                let out_frames = (scratch.len() / 2) as i64;

                // Place on the absolute timeline by QPC; resync on a real gap or
                // a driver-flagged discontinuity, else keep contiguous (avoids
                // pops from sub-millisecond jitter).
                let expected = mixer.tick_to_idx(tick);
                if !self.started
                    || discontinuity
                    || (expected - self.next_idx).abs() > GAP_SAMPLES
                {
                    self.next_idx = expected.max(mixer.mix_base);
                }
                self.started = true;

                mixer.add(self.next_idx, scratch);
                self.next_idx += out_frames;
                any = true;
            }

            if self.capture.ReleaseBuffer(frames).is_err() {
                break;
            }
        }
        any
    }

    /// swr_convert `in_frames` of source-format interleaved audio at `data` into
    /// `out` as 48 kHz stereo interleaved f32.
    unsafe fn resample(&self, data: *const u8, in_frames: i32, out: &mut Vec<f32>) {
        out.clear();
        if in_frames <= 0 {
            return;
        }
        // Upper bound on output frames (+slack for swr's internal buffering).
        let max_out =
            (in_frames as i64 * MIX_RATE as i64 / self.fmt.rate.max(1) as i64 + 1024) as i32;
        out.resize(max_out as usize * 2, 0.0);
        let out_planes: [*mut u8; 1] = [out.as_mut_ptr() as *mut u8];
        let in_planes: [*const u8; 1] = [data];
        let n = ffi::swr_convert(
            self.swr,
            out_planes.as_ptr(),
            max_out,
            in_planes.as_ptr(),
            in_frames,
        );
        if n < 0 {
            out.clear();
            return;
        }
        out.truncate(n as usize * 2);
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            if !self.swr.is_null() {
                ffi::swr_free(&mut self.swr);
            }
        }
    }
}

/// Parse a `WAVEFORMATEX`(`EXTENSIBLE`) into the fields swr needs. Shared-mode
/// WASAPI is virtually always 32-bit float; PCM is handled defensively.
unsafe fn parse_format(wf: *const WAVEFORMATEX) -> SrcFormat {
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {00000003-0000-0010-8000-00aa00389b71}.
    // Hardcoded so we don't depend on the KernelStreaming feature.
    const SUBTYPE_FLOAT_D1: u32 = 0x0000_0003;
    const SUBTYPE_PCM_D1: u32 = 0x0000_0001;

    let w = &*wf;
    let channels = w.nChannels;
    let rate = w.nSamplesPerSec as i32;
    let bits = w.wBitsPerSample;
    let block_align = w.nBlockAlign;

    let is_float = if w.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        let ext = &*(wf as *const WAVEFORMATEXTENSIBLE);
        match ext.SubFormat.data1 {
            SUBTYPE_FLOAT_D1 => true,
            SUBTYPE_PCM_D1 => false,
            _ => bits == 32, // unknown subtype: assume float if 32-bit
        }
    } else {
        w.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
    };

    let av_fmt = if is_float {
        ffi::AV_SAMPLE_FMT_FLT
    } else if bits == 16 {
        ffi::AV_SAMPLE_FMT_S16
    } else {
        ffi::AV_SAMPLE_FMT_S32
    };

    SrcFormat {
        rate,
        channels,
        av_fmt,
        block_align,
    }
}

/// swr context: source (its rate/channels/format) → 48 kHz stereo interleaved
/// f32 (packed FLT, easy additive mixing).
unsafe fn build_resampler(fmt: &SrcFormat) -> Result<*mut ffi::SwrContext, String> {
    let mut out_layout: ffi::AVChannelLayout = std::mem::zeroed();
    let mut in_layout: ffi::AVChannelLayout = std::mem::zeroed();
    ffi::av_channel_layout_default(&mut out_layout, MIX_CHANNELS);
    ffi::av_channel_layout_default(&mut in_layout, fmt.channels.max(1) as i32);

    let mut swr: *mut ffi::SwrContext = ptr::null_mut();
    let r = ffi::swr_alloc_set_opts2(
        &mut swr,
        &out_layout,
        ffi::AV_SAMPLE_FMT_FLT,
        MIX_RATE,
        &in_layout,
        fmt.av_fmt,
        fmt.rate.max(1),
        0,
        ptr::null_mut(),
    );
    ffi::av_channel_layout_uninit(&mut out_layout);
    ffi::av_channel_layout_uninit(&mut in_layout);
    if r < 0 || swr.is_null() {
        return Err(format!("swr_alloc_set_opts2: {}", av_err(r)));
    }
    let r = ffi::swr_init(swr);
    if r < 0 {
        ffi::swr_free(&mut swr);
        return Err(format!("swr_init: {}", av_err(r)));
    }
    Ok(swr)
}

// ---------------------------------------------------------------------------
// Mixer: additive accumulator on an absolute 48 kHz sample timeline
// ---------------------------------------------------------------------------

/// Accumulates resampled stereo samples from all sources on one timeline anchored
/// at `epoch_ticks` (100 ns). `mix` holds interleaved L,R from absolute frame
/// index `mix_base`; sources add into it, and full `block`-sized frames are
/// drained out once both sources have caught up.
struct Mixer {
    mix: VecDeque<f32>,
    mix_base: i64,
    /// QPC time (100 ns ticks) of absolute frame index 0. Set by the first
    /// packet of any source.
    epoch_ticks: Option<i64>,
    /// Detected conversion from raw `QPCPosition` to 100 ns ticks (see
    /// [`detect_qpc_scale`]). `None` until the first packet calibrates it.
    qpc_is_100ns: Option<bool>,
    /// Highest absolute index any source has written up to — the mixer never
    /// emits past this minus latency, so late sources still get mixed in.
    high_water: i64,
    block: usize,
}

impl Mixer {
    fn new(block: usize) -> Mixer {
        Mixer {
            mix: VecDeque::new(),
            mix_base: 0,
            epoch_ticks: None,
            qpc_is_100ns: None,
            high_water: 0,
            block: block.max(1),
        }
    }

    /// Convert a raw `QPCPosition` to 100 ns ticks, calibrating the unit on first
    /// use against `QueryPerformanceCounter`/`Frequency` (so we don't depend on
    /// whether the driver reports raw ticks or pre-converted 100 ns).
    fn qpc_to_ticks(&mut self, qpc_pos: u64, qpc_freq: i64) -> i64 {
        let is_100ns = *self
            .qpc_is_100ns
            .get_or_insert_with(|| detect_qpc_scale(qpc_pos, qpc_freq));
        if is_100ns {
            qpc_pos as i64
        } else {
            (qpc_pos as i128 * TICKS_PER_SECOND as i128 / qpc_freq.max(1) as i128) as i64
        }
    }

    /// Absolute 48 kHz frame index for a 100 ns tick (anchoring the epoch on
    /// first call).
    fn tick_to_idx(&mut self, tick: i64) -> i64 {
        let epoch = *self.epoch_ticks.get_or_insert(tick);
        ((tick - epoch) as i128 * MIX_RATE as i128 / TICKS_PER_SECOND as i128) as i64
    }

    /// Additively mix `samples` (interleaved stereo f32) starting at absolute
    /// frame index `at`.
    fn add(&mut self, at: i64, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let n = samples.len() / 2;
        // Trim anything already drained (older than mix_base).
        let (at, skip) = if at < self.mix_base {
            (self.mix_base, (self.mix_base - at) as usize)
        } else {
            (at, 0)
        };
        if skip >= n {
            return;
        }
        let start = (at - self.mix_base) as usize; // in frames
        let needed = (start + (n - skip)) * 2;
        if self.mix.len() < needed {
            self.mix.resize(needed, 0.0);
        }
        for i in skip..n {
            let di = (start + (i - skip)) * 2;
            if let Some(v) = self.mix.get_mut(di) {
                *v += samples[i * 2];
            }
            if let Some(v) = self.mix.get_mut(di + 1) {
                *v += samples[i * 2 + 1];
            }
        }
        self.high_water = self.high_water.max(at + (n - skip) as i64);
    }

    /// Pull out every full `block`-frame chunk that both sources have caught up
    /// past (or all remaining when `final_flush`). Each chunk is paired with the
    /// 100 ns tick of its first sample for muxing.
    fn drain_ready(&mut self, final_flush: bool) -> Vec<(Vec<f32>, i64)> {
        let mut out = Vec::new();
        let epoch = match self.epoch_ticks {
            Some(e) => e,
            None => return out,
        };
        let limit = if final_flush {
            self.high_water
        } else {
            self.high_water - LATENCY_SAMPLES
        };
        while self.mix_base + self.block as i64 <= limit && self.mix.len() >= self.block * 2 {
            let mut chunk = Vec::with_capacity(self.block * 2);
            for _ in 0..self.block * 2 {
                chunk.push(self.mix.pop_front().unwrap_or(0.0));
            }
            let pts_ticks =
                epoch + (self.mix_base as i128 * TICKS_PER_SECOND as i128 / MIX_RATE as i128) as i64;
            out.push((chunk, pts_ticks));
            self.mix_base += self.block as i64;
        }
        out
    }
}

/// Decide whether `qpc_pos` is already in 100 ns units or raw QPC ticks by
/// comparing both interpretations of "now" against the live performance counter.
/// On modern hardware `qpc_freq` is often exactly 10 MHz, making the two
/// identical — but on TSC-derived counters it isn't, so we must check.
fn detect_qpc_scale(qpc_pos: u64, qpc_freq: i64) -> bool {
    let mut now_raw = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut now_raw);
    }
    let now_secs = now_raw as f64 / qpc_freq.max(1) as f64;
    let as_100ns = qpc_pos as f64 / TICKS_PER_SECOND as f64;
    let as_raw = qpc_pos as f64 / qpc_freq.max(1) as f64;
    // The packet time must be very close to "now" (it's the just-read position).
    (as_100ns - now_secs).abs() <= (as_raw - now_secs).abs()
}

// ---------------------------------------------------------------------------
// AAC encoder
// ---------------------------------------------------------------------------

/// FFmpeg native AAC encoder, fed exactly `frame_size` stereo samples per call.
struct AacEncoder {
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    /// Monotonic sample-count PTS handed to the encoder (it wants sample units).
    sample_pts: i64,
    /// Ticks of each fed block, popped in order to stamp output packets — the
    /// encoder's small constant delay preserves order, so FIFO mapping keeps the
    /// real wall-clock tick on every packet.
    pending_ticks: VecDeque<i64>,
    /// Last tick assigned to an output packet, so a trailing flush/padding packet
    /// (emitted after the queue drains) continues the timeline instead of
    /// jumping back to the encoder's raw sample pts.
    last_tick: i64,
    /// Wall-clock span of one encoded frame, in 100 ns ticks.
    block_ticks: i64,
}

impl AacEncoder {
    fn new() -> Result<AacEncoder, String> {
        unsafe {
            let codec = ffi::avcodec_find_encoder(ffi::AV_CODEC_ID_AAC);
            if codec.is_null() {
                return Err("AAC encoder not found in linked FFmpeg".into());
            }
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3(aac) failed".into());
            }
            (*ctx).sample_fmt = ffi::AV_SAMPLE_FMT_FLTP;
            (*ctx).sample_rate = MIX_RATE;
            (*ctx).bit_rate = AAC_BITRATE;
            ffi::av_channel_layout_default(&mut (*ctx).ch_layout, MIX_CHANNELS);
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: MIX_RATE,
            };
            (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER;

            let r = ffi::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err(format!("avcodec_open2(aac): {}", av_err(r)));
            }

            let mut frame_size = (*ctx).frame_size;
            if frame_size <= 0 {
                frame_size = 1024; // AAC LC default if the encoder didn't set it
            }
            (*ctx).frame_size = frame_size;

            let frame = ffi::av_frame_alloc();
            if frame.is_null() {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err("av_frame_alloc(aac) failed".into());
            }
            (*frame).nb_samples = frame_size;
            (*frame).format = ffi::AV_SAMPLE_FMT_FLTP;
            ffi::av_channel_layout_default(&mut (*frame).ch_layout, MIX_CHANNELS);
            let r = ffi::av_frame_get_buffer(frame, 0);
            if r < 0 {
                let mut f = frame;
                ffi::av_frame_free(&mut f);
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err(format!("av_frame_get_buffer(aac): {}", av_err(r)));
            }

            let packet = ffi::av_packet_alloc();
            if packet.is_null() {
                let mut f = frame;
                ffi::av_frame_free(&mut f);
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err("av_packet_alloc(aac) failed".into());
            }

            Ok(AacEncoder {
                ctx,
                frame,
                packet,
                sample_pts: 0,
                pending_ticks: VecDeque::new(),
                last_tick: 0,
                block_ticks: frame_size as i64 * TICKS_PER_SECOND / MIX_RATE as i64,
            })
        }
    }

    fn frame_size(&self) -> usize {
        unsafe { (*self.ctx).frame_size.max(1) as usize }
    }

    fn extradata(&self) -> Vec<u8> {
        unsafe {
            let c = &*self.ctx;
            if c.extradata.is_null() || c.extradata_size <= 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
        }
    }

    /// Encode one block of `frame_size` interleaved-stereo samples whose first
    /// sample is at `pts_ticks` (100 ns). Returns packets the encoder produced.
    fn encode_block(&mut self, block: &[f32], pts_ticks: i64) -> Result<Vec<EncodedPacket>, String> {
        let n = self.frame_size();
        unsafe {
            let r = ffi::av_frame_make_writable(self.frame);
            if r < 0 {
                return Err(format!("av_frame_make_writable(aac): {}", av_err(r)));
            }
            // De-interleave into the FLTP planes (data[0]=L, data[1]=R).
            let l = (*self.frame).data[0] as *mut f32;
            let rch = (*self.frame).data[1] as *mut f32;
            for i in 0..n {
                let (lv, rv) = match (block.get(i * 2), block.get(i * 2 + 1)) {
                    (Some(&a), Some(&b)) => (a, b),
                    _ => (0.0, 0.0),
                };
                *l.add(i) = lv;
                *rch.add(i) = rv;
            }
            (*self.frame).pts = self.sample_pts;
            self.sample_pts += n as i64;
            self.pending_ticks.push_back(pts_ticks);

            let r = ffi::avcodec_send_frame(self.ctx, self.frame);
            if r < 0 {
                return Err(format!("avcodec_send_frame(aac): {}", av_err(r)));
            }
            self.drain()
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, String> {
        unsafe {
            let r = ffi::avcodec_send_frame(self.ctx, ptr::null());
            if r < 0 && r != AVERROR_EOF {
                return Err(format!("avcodec_send_frame(aac flush): {}", av_err(r)));
            }
            self.drain()
        }
    }

    unsafe fn drain(&mut self) -> Result<Vec<EncodedPacket>, String> {
        let mut out = Vec::new();
        loop {
            let r = ffi::avcodec_receive_packet(self.ctx, self.packet);
            if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                break;
            }
            if r < 0 {
                return Err(format!("avcodec_receive_packet(aac): {}", av_err(r)));
            }
            let pkt = &*self.packet;
            let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize).to_vec();
            // Stamp with the real wall-clock tick of the matching input block; a
            // trailing flush packet (queue empty) continues from the last tick.
            let tick = self
                .pending_ticks
                .pop_front()
                .unwrap_or(self.last_tick + self.block_ticks);
            self.last_tick = tick;
            out.push(EncodedPacket {
                data,
                pts: tick,
                dts: tick,
                keyframe: true, // every AAC frame is independently decodable
            });
            ffi::av_packet_unref(self.packet);
        }
        Ok(out)
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
        }
    }
}

/// Test helper: encode `secs` of silence to real AAC packets (ticks starting at
/// 0). Deterministic and device-free, so the muxer's audio path can be tested
/// headlessly. Used by `mux.rs` tests.
#[cfg(test)]
pub(crate) fn encode_silence_aac(secs: f64) -> (AudioMeta, Vec<EncodedPacket>) {
    let mut enc = AacEncoder::new().expect("aac encoder");
    let meta = AudioMeta {
        sample_rate: MIX_RATE as u32,
        channels: MIX_CHANNELS as u32,
        extradata: enc.extradata(),
    };
    let block = enc.frame_size();
    let nblocks = (secs * MIX_RATE as f64 / block as f64) as i64;
    let silence = vec![0f32; block * 2];
    let mut packets = Vec::new();
    for i in 0..nblocks {
        let pts_ticks = i * block as i64 * TICKS_PER_SECOND / MIX_RATE as i64;
        packets.extend(enc.encode_block(&silence, pts_ticks).expect("encode_block"));
    }
    packets.extend(enc.flush().expect("flush"));
    (meta, packets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// ADTS sampling-frequency index for the standard rates.
    fn freq_index(rate: u32) -> u8 {
        match rate {
            96000 => 0,
            88200 => 1,
            64000 => 2,
            48000 => 3,
            44100 => 4,
            32000 => 5,
            24000 => 6,
            22050 => 7,
            16000 => 8,
            12000 => 9,
            11025 => 10,
            8000 => 11,
            _ => 3,
        }
    }

    /// Prepend a 7-byte ADTS header to each raw AAC-LC frame so the concatenated
    /// bytes are a playable `.aac` file (our encoder emits raw frames for MP4, no
    /// ADTS). Lets a human verify the *sound* — the part no assertion can check.
    fn write_adts(path: &std::path::Path, meta: &AudioMeta, packets: &[EncodedPacket]) {
        let fi = freq_index(meta.sample_rate);
        let ch = meta.channels as u8;
        let mut out = Vec::new();
        for p in packets {
            let frame_len = (p.data.len() + 7) as u32;
            // profile=AAC LC (object type 2 → 1), protection absent.
            out.push(0xFF);
            out.push(0xF1);
            out.push(((1 << 6) | (fi << 2) | ((ch >> 2) & 1)) as u8);
            out.push((((ch & 3) << 6) as u32 | (frame_len >> 11)) as u8);
            out.push(((frame_len >> 3) & 0xFF) as u8);
            out.push((((frame_len & 7) << 5) | 0x1F) as u8);
            out.push(0xFC);
            out.extend_from_slice(&p.data);
        }
        std::fs::write(path, out).expect("write aac");
    }

    /// Live capture of desktop audio (+ mic) for a few seconds → AAC. **Play
    /// something audible while running this**, then listen to the printed `.aac`.
    /// In silence WASAPI loopback delivers no packets, so we don't assert a
    /// packet count — we assert the pipeline is sound (encoder opens, produces
    /// valid ASC extradata, and any packets carry monotonic ticks).
    #[test]
    fn captures_desktop_audio_to_aac() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let result = (|| -> Result<(), String> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                        .map_err(|e| format!("enumerator: {e}"))?;
                let mut sources = Vec::new();
                match Source::open_loopback(&enumerator) {
                    Ok(s) => sources.push(s),
                    Err(e) => println!("loopback unavailable: {e}"),
                }
                match Source::open_mic(&enumerator) {
                    Ok(s) => sources.push(s),
                    Err(e) => println!("mic unavailable: {e}"),
                }
                if sources.is_empty() {
                    return Err("no audio devices".into());
                }

                let mut encoder = AacEncoder::new()?;
                let meta = AudioMeta {
                    sample_rate: MIX_RATE as u32,
                    channels: MIX_CHANNELS as u32,
                    extradata: encoder.extradata(),
                };
                assert!(
                    !meta.extradata.is_empty(),
                    "AAC encoder produced no AudioSpecificConfig (GLOBAL_HEADER off?)"
                );
                let block = encoder.frame_size();
                println!("aac frame_size={block}, asc={} bytes", meta.extradata.len());

                for s in &sources {
                    s.start()?;
                }
                let qpc_freq = {
                    let mut f = 0i64;
                    QueryPerformanceFrequency(&mut f).ok();
                    f.max(1)
                };

                let mut mixer = Mixer::new(block);
                let mut scratch = Vec::new();
                let mut zero = Vec::new();
                let mut packets: Vec<EncodedPacket> = Vec::new();

                let start = Instant::now();
                while start.elapsed().as_secs_f64() < 4.0 {
                    let mut any = false;
                    for src in &mut sources {
                        any |= src.drain(&mut mixer, qpc_freq, &mut scratch, &mut zero);
                    }
                    for (samples, pts) in mixer.drain_ready(false) {
                        packets.extend(encoder.encode_block(&samples, pts)?);
                    }
                    if !any {
                        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                    }
                }
                for s in &sources {
                    let _ = s.audio_client.Stop();
                }
                for (samples, pts) in mixer.drain_ready(true) {
                    packets.extend(encoder.encode_block(&samples, pts)?);
                }
                packets.extend(encoder.flush()?);

                println!("captured {} AAC packets", packets.len());
                // Ticks must be non-decreasing (FIFO mapping preserves order).
                for w in packets.windows(2) {
                    assert!(w[1].pts >= w[0].pts, "audio ticks went backwards");
                }
                if !packets.is_empty() {
                    let out = std::env::temp_dir().join("hako_audio_test.aac");
                    write_adts(&out, &meta, &packets);
                    println!("WROTE PLAYABLE AUDIO → {} (play it to verify sound)", out.display());
                } else {
                    println!("no audio captured (was anything playing?) — pipeline still OK");
                }
                Ok(())
            }
        })();
        unsafe {
            CoUninitialize();
        }
        result.expect("audio capture pipeline");
    }
}
