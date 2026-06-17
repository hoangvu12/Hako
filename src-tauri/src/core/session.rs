//! Mode B full-match session writer + timeline index.
//!
//! While a Valorant match is INGAME, Hako records the **whole** match to a temp
//! MP4 so that — once the match ends and Riot publishes `match-details` — each
//! kill/multikill can be cut out of it (post-match detection; see
//! `valorant/reconcile.rs`). This is the "Mode B" companion to the always-on
//! instant-replay ring (`buffer.rs`, "Mode A").
//!
//! [`SessionWriter`] taps the **same** already-encoded [`EncodedPacket`] stream
//! that the encode thread feeds to [`crate::core::capture::ClipBuffer`] and
//! **stream-copies** it straight to disk — never re-encoding, the CPU only ever
//! touches compressed bytes (the golden rule). Unlike [`crate::core::mux`],
//! which takes a whole slice at once, this writes incrementally: open + header
//! on [`start`](SessionWriter::start), one `av_interleaved_write_frame` per
//! [`push`](SessionWriter::push)/[`push_audio`](SessionWriter::push_audio), and
//! trailer on [`finish`](SessionWriter::finish) — so a 30-minute match never
//! needs to sit in RAM.
//!
//! ## The timeline index
//! For every video packet written it records a [`TimelineIndex`] sample pairing
//! the packet's **wall-clock tick** (100 ns, the same QPC/`SystemRelativeTime`
//! domain as the round anchors from `valorant/log_watch.rs`) with its
//! **session-file PTS** (in `1/fps` units, rebased so the file starts at 0).
//! Post-match, `reconcile.rs` maps an event's reconstructed wall-clock through
//! this index to a session PTS, then a clip window is cut from the session file.
//!
//! The session video stream's time base is `1/fps`; the timeline PTS values are
//! in those same pre-mux units. A consumer that demuxes the session file (the
//! M4 cut pipeline) must rescale demuxed timestamps from the file's stream time
//! base back into `1/fps` before comparing against the timeline.

#![allow(dead_code)]

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Mutex;

use rusty_ffmpeg::ffi;

use crate::core::audio::AudioMeta;
use crate::core::clock::TICKS_PER_SECOND;
use crate::core::encode::{av_err, EncodedPacket};
use crate::core::mux::ClipMeta;
use crate::valorant::reconcile::TimelineIndex;

// Raw FFmpeg flag values (ABI-stable; mirrors `mux.rs` — avoids depending on the
// prebuilt binding exporting them as constants).
const AVFMT_NOFILE: i32 = 0x0001;
const AVIO_FLAG_WRITE: i32 = 2;
const AV_PKT_FLAG_KEY: i32 = 1;
/// Required trailing zero-padding on any buffer handed to FFmpeg as extradata.
const AV_INPUT_BUFFER_PADDING_SIZE: usize = 64;

/// Streaming full-match MP4 writer with a wall-clock↔PTS timeline.
///
/// Shared behind an `Arc` like [`crate::core::capture::ClipBuffer`]: the encode
/// thread calls [`push`](Self::push) (video) and the audio thread calls
/// [`push_audio`](Self::push_audio), both through the interior `Mutex` that
/// serializes access to the (non-thread-safe) FFmpeg output context.
pub struct SessionWriter {
    out_path: PathBuf,
    state: Mutex<SessionState>,
}

// SAFETY: every access to the raw FFmpeg pointers in `SessionState` goes through
// `state`'s `Mutex`, so they are never touched concurrently. The pointers own no
// thread-affine resources (the mp4 muxer is plain libavformat, not D3D/COM).
unsafe impl Send for SessionWriter {}
unsafe impl Sync for SessionWriter {}

struct SessionState {
    /// mp4 output context; null once finished/failed.
    ofmt: *mut ffi::AVFormatContext,
    /// Reusable packet handle for writes; freed on finish/drop.
    pkt: *mut ffi::AVPacket,

    fps: i32,
    /// `1/fps` — the units of incoming video `EncodedPacket::pts`.
    video_src_tb: ffi::AVRational,
    /// The video stream's time base after `write_header` (mp4 may rescale).
    video_dst_tb: ffi::AVRational,

    /// One entry per declared AAC stream (track 0 = master mix, 1..N = stems);
    /// empty for a video-only session. Indexed by the `track_idx` the audio
    /// thread pushes with.
    audio: Vec<AudioStream>,

    /// PTS of the first video packet (capture-clock relative); session PTS is
    /// rebased off this so the file starts at 0.
    first_video_pts: Option<i64>,
    /// Last session video PTS written, to keep PTS strictly increasing.
    last_video_pts: i64,
    /// Wall-clock tick of the first video packet — the origin audio rebases on.
    session_start_ticks: Option<i64>,

    timeline: TimelineIndex,
    video_count: u64,
    audio_count: u64,
    finished: bool,
}

struct AudioStream {
    index: i32,
    sample_rate: i64,
    src_tb: ffi::AVRational,
    dst_tb: ffi::AVRational,
    /// Last session audio PTS (samples) written for this stream, kept strictly
    /// increasing per stream.
    last_pts: i64,
}

impl SessionWriter {
    /// Open `out_path` as an mp4 and write the header, declaring an H.264 video
    /// stream (`meta`) and one named AAC stream per entry in `audio_tracks`
    /// (track 0 = master mix; empty ⇒ video-only).
    ///
    /// All streams must be declared before any packet, so the audio-track layout
    /// is fixed here: pass every published `(name, AudioMeta)` (the audio thread
    /// then pushes packets tagged with the matching track index).
    pub fn start(
        out_path: &Path,
        meta: &ClipMeta,
        audio_tracks: &[(String, AudioMeta)],
    ) -> Result<SessionWriter, String> {
        let path_str = out_path.to_str().ok_or("session path is not valid UTF-8")?;
        let c_path = CString::new(path_str).map_err(|_| "session path contains NUL")?;
        let c_mp4 = CString::new("mp4").unwrap();
        let fps = meta.fps.max(1) as i32;

        unsafe {
            let mut ofmt: *mut ffi::AVFormatContext = ptr::null_mut();
            let r = ffi::avformat_alloc_output_context2(
                &mut ofmt,
                ptr::null_mut(),
                c_mp4.as_ptr(),
                c_path.as_ptr(),
            );
            if r < 0 || ofmt.is_null() {
                return Err(format!("avformat_alloc_output_context2: {}", av_err(r)));
            }

            // Build streams + header inside a closure so any failure tears the
            // context back down before returning.
            match Self::open_inner(ofmt, &c_path, meta, audio_tracks, fps) {
                Ok(state) => Ok(SessionWriter {
                    out_path: out_path.to_path_buf(),
                    state: Mutex::new(state),
                }),
                Err(e) => {
                    if !(*ofmt).pb.is_null() && ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
                        ffi::avio_closep(&mut (*ofmt).pb);
                    }
                    ffi::avformat_free_context(ofmt);
                    Err(e)
                }
            }
        }
    }

    /// Declare the streams, open the file, and write the mp4 header. On success
    /// returns the populated [`SessionState`]; the caller frees `ofmt` on `Err`.
    unsafe fn open_inner(
        ofmt: *mut ffi::AVFormatContext,
        c_path: &CString,
        meta: &ClipMeta,
        audio_tracks: &[(String, AudioMeta)],
        fps: i32,
    ) -> Result<SessionState, String> {
        // Stream 0: H.264 video, time base = 1/fps (our packet PTS units).
        let st_v = ffi::avformat_new_stream(ofmt, ptr::null());
        if st_v.is_null() {
            return Err("avformat_new_stream(video) failed".into());
        }
        (*st_v).id = 0;
        (*st_v).time_base = ffi::AVRational { num: 1, den: fps };
        let par = (*st_v).codecpar;
        (*par).codec_type = ffi::AVMEDIA_TYPE_VIDEO;
        (*par).codec_id = ffi::AV_CODEC_ID_H264;
        (*par).width = meta.width as i32;
        (*par).height = meta.height as i32;
        (*par).format = ffi::AV_PIX_FMT_NV12; // informational for a copy stream
        set_extradata(par, &meta.extradata)?;

        // Streams 1..N (optional): one named AAC track each (track 0 = master mix,
        // the rest are stems). A track is declared only when its meta carries the
        // AudioSpecificConfig the mp4 esds box needs, so a stem whose encoder
        // never produced one is skipped.
        let title_key = CString::new("title").unwrap();
        let handler_key = CString::new("handler_name").unwrap();
        let mut audio: Vec<(*mut ffi::AVStream, i64)> = Vec::new();
        for (name, a) in audio_tracks {
            if a.extradata.is_empty() {
                continue;
            }
            let st_a = ffi::avformat_new_stream(ofmt, ptr::null());
            if st_a.is_null() {
                return Err("avformat_new_stream(audio) failed".into());
            }
            let sr = a.sample_rate.max(1) as i32;
            (*st_a).id = (audio.len() as i32) + 1;
            (*st_a).time_base = ffi::AVRational { num: 1, den: sr };
            let apar = (*st_a).codecpar;
            (*apar).codec_type = ffi::AVMEDIA_TYPE_AUDIO;
            (*apar).codec_id = ffi::AV_CODEC_ID_AAC;
            (*apar).sample_rate = sr;
            (*apar).format = ffi::AV_SAMPLE_FMT_FLTP; // informational for copy
            ffi::av_channel_layout_default(&mut (*apar).ch_layout, a.channels.max(1) as i32);
            set_extradata(apar, &a.extradata)?;
            // Label the stream so the editor can show the track name.
            // `handler_name` (the trak `hdlr` box) survives the MP4 round-trip
            // most reliably; we also set `title` (udta) for tools reading that.
            if !name.is_empty() {
                if let Ok(c_name) = CString::new(name.as_str()) {
                    ffi::av_dict_set(&mut (*st_a).metadata, handler_key.as_ptr(), c_name.as_ptr(), 0);
                    ffi::av_dict_set(&mut (*st_a).metadata, title_key.as_ptr(), c_name.as_ptr(), 0);
                }
            }
            audio.push((st_a, sr as i64));
        }

        // Open the file and write the header (mp4 is not a file-less format).
        if ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
            let r = ffi::avio_open(&mut (*ofmt).pb, c_path.as_ptr(), AVIO_FLAG_WRITE);
            if r < 0 {
                return Err(format!("avio_open: {}", av_err(r)));
            }
        }
        let r = ffi::avformat_write_header(ofmt, ptr::null_mut());
        if r < 0 {
            return Err(format!("avformat_write_header: {}", av_err(r)));
        }

        let pkt = ffi::av_packet_alloc();
        if pkt.is_null() {
            return Err("av_packet_alloc failed".into());
        }

        // `write_header` may have adjusted the stream time bases.
        let video_dst_tb = (*st_v).time_base;
        let audio = audio
            .into_iter()
            .map(|(st_a, sr)| AudioStream {
                index: (*st_a).index,
                sample_rate: sr,
                src_tb: ffi::AVRational { num: 1, den: sr as i32 },
                dst_tb: (*st_a).time_base,
                last_pts: i64::MIN,
            })
            .collect();

        Ok(SessionState {
            ofmt,
            pkt,
            fps,
            video_src_tb: ffi::AVRational { num: 1, den: fps },
            video_dst_tb,
            audio,
            first_video_pts: None,
            last_video_pts: i64::MIN,
            session_start_ticks: None,
            timeline: TimelineIndex::new(),
            video_count: 0,
            audio_count: 0,
            finished: false,
        })
    }

    /// Append one already-encoded **video** packet, stream-copied to the session
    /// file, and record its timeline sample. `wallclock_ticks` is the packet's
    /// capture timestamp (100 ns, `SystemRelativeTime`/QPC domain) — the same
    /// clock the round anchors use. No-op once finished.
    pub fn push(&self, pkt: &EncodedPacket, wallclock_ticks: i64) {
        if let Ok(mut s) = self.state.lock() {
            s.write_video(pkt, wallclock_ticks);
        }
    }

    /// Append one already-encoded **AAC** packet for output track `track_idx`
    /// (PTS in absolute 100 ns ticks, the audio ring's unit) to that track's
    /// stream. Rebased against the session start; anything before the first video
    /// frame is dropped. No-op if the session has no stream for that track, or
    /// once finished.
    pub fn push_audio(&self, track_idx: usize, pkt: &EncodedPacket) {
        if let Ok(mut s) = self.state.lock() {
            s.write_audio(track_idx, pkt);
        }
    }

    /// Finalize the mp4 (write trailer, close IO) and return the session path and
    /// its timeline index. Idempotent: later calls return the same path/timeline
    /// without rewriting. Errors only if the trailer write fails.
    pub fn finish(&self) -> Result<(PathBuf, TimelineIndex), String> {
        let mut s = self.state.lock().map_err(|_| "session writer poisoned")?;
        s.finalize()?;
        Ok((self.out_path.clone(), s.timeline.clone()))
    }

    /// Number of video / audio packets written so far.
    pub fn counts(&self) -> (u64, u64) {
        self.state
            .lock()
            .map(|s| (s.video_count, s.audio_count))
            .unwrap_or((0, 0))
    }

    /// The session file path (valid even after `finish`).
    pub fn path(&self) -> &Path {
        &self.out_path
    }
}

impl SessionState {
    fn write_video(&mut self, pkt: &EncodedPacket, wallclock_ticks: i64) {
        if self.finished || self.ofmt.is_null() {
            return;
        }
        let first = *self.first_video_pts.get_or_insert(pkt.pts);
        if self.session_start_ticks.is_none() {
            self.session_start_ticks = Some(wallclock_ticks);
        }
        // Rebase to a 0-based, strictly-increasing session timeline (the muxer
        // requires monotonic DTS; capture is VFR so rounding can collide).
        let mut spts = pkt.pts - first;
        if spts <= self.last_video_pts {
            spts = self.last_video_pts + 1;
        }
        self.last_video_pts = spts;

        // Timeline pairs the true capture wall-clock with the session PTS.
        self.timeline.push(wallclock_ticks, spts);

        let ok = unsafe {
            write_packet(
                self.ofmt,
                self.pkt,
                0,
                spts,
                self.video_src_tb,
                self.video_dst_tb,
                pkt.keyframe,
                &pkt.data,
            )
        };
        match ok {
            Ok(()) => self.video_count += 1,
            Err(e) => tracing::warn!("session: video write failed: {e}"),
        }
    }

    fn write_audio(&mut self, track_idx: usize, pkt: &EncodedPacket) {
        if self.finished || self.ofmt.is_null() {
            return;
        }
        // Need the session origin (first video frame) before audio can be placed.
        let Some(start) = self.session_start_ticks else {
            return;
        };
        let Some(audio) = self.audio.get_mut(track_idx) else {
            return; // no stream for this track (video-only, or out of range)
        };
        let rel_ticks = pkt.pts - start;
        if rel_ticks < 0 {
            return; // before the clip start — drop (small leading silence)
        }
        let mut pts_samples =
            (rel_ticks as i128 * audio.sample_rate as i128 / TICKS_PER_SECOND as i128) as i64;
        if pts_samples <= audio.last_pts {
            pts_samples = audio.last_pts + 1;
        }
        audio.last_pts = pts_samples;

        let (index, src_tb, dst_tb) = (audio.index, audio.src_tb, audio.dst_tb);
        let ok = unsafe {
            write_packet(
                self.ofmt, self.pkt, index, pts_samples, src_tb, dst_tb, true, &pkt.data,
            )
        };
        match ok {
            Ok(()) => self.audio_count += 1,
            Err(e) => tracing::warn!("session: audio write failed: {e}"),
        }
    }

    fn finalize(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let mut result = Ok(());
        unsafe {
            if !self.ofmt.is_null() {
                // Only write a trailer if a header was written and at least the
                // streams exist; av_write_trailer flushes any interleave buffer.
                let r = ffi::av_write_trailer(self.ofmt);
                if r < 0 {
                    result = Err(format!("av_write_trailer: {}", av_err(r)));
                }
                if !(*self.ofmt).pb.is_null()
                    && ((*(*self.ofmt).oformat).flags & AVFMT_NOFILE) == 0
                {
                    ffi::avio_closep(&mut (*self.ofmt).pb);
                }
                ffi::avformat_free_context(self.ofmt);
                self.ofmt = ptr::null_mut();
            }
            if !self.pkt.is_null() {
                ffi::av_packet_free(&mut self.pkt);
            }
        }
        result
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        // If the caller never called finish() (e.g. capture torn down on error),
        // still release the FFmpeg resources. Best-effort trailer.
        let _ = self.finalize();
    }
}

/// Copy `bytes` into a freshly-allocated FFmpeg-owned extradata buffer (with the
/// mandatory trailing padding) and attach it to `par`. Mirrors `mux::set_extradata`.
unsafe fn set_extradata(par: *mut ffi::AVCodecParameters, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let size = bytes.len();
    let buf = ffi::av_mallocz(size + AV_INPUT_BUFFER_PADDING_SIZE) as *mut u8;
    if buf.is_null() {
        return Err("av_mallocz(extradata) failed".into());
    }
    ptr::copy_nonoverlapping(bytes.as_ptr(), buf, size);
    (*par).extradata = buf;
    (*par).extradata_size = size as i32;
    Ok(())
}

/// Write one already-encoded packet through `av_interleaved_write_frame`,
/// reusing `pkt`. `pts` is in `src_tb` units and rescaled to `dst_tb`; with no
/// B-frames `dts == pts`. The interleaver owns and unrefs `pkt`'s buffer.
#[allow(clippy::too_many_arguments)]
unsafe fn write_packet(
    ofmt: *mut ffi::AVFormatContext,
    pkt: *mut ffi::AVPacket,
    stream_index: i32,
    pts: i64,
    src_tb: ffi::AVRational,
    dst_tb: ffi::AVRational,
    keyframe: bool,
    data: &[u8],
) -> Result<(), String> {
    if ffi::av_new_packet(pkt, data.len() as i32) < 0 {
        return Err("av_new_packet failed".into());
    }
    ptr::copy_nonoverlapping(data.as_ptr(), (*pkt).data, data.len());
    (*pkt).pts = pts;
    (*pkt).dts = pts; // no B-frames in either stream ⇒ dts == pts
    (*pkt).stream_index = stream_index;
    (*pkt).flags = if keyframe { AV_PKT_FLAG_KEY } else { 0 };
    ffi::av_packet_rescale_ts(pkt, src_tb, dst_tb);

    let r = ffi::av_interleaved_write_frame(ofmt, pkt);
    // av_interleaved_write_frame unrefs pkt on both success and failure.
    if r < 0 {
        return Err(format!("av_interleaved_write_frame: {}", av_err(r)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::convert::Converter;
    use crate::core::device;
    use crate::core::encode::Encoder;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    /// Reopen an mp4 and return (stream count, [(codec_type, codec_id, title)],
    /// video packet count) — proves the session file is a valid, demuxable MP4
    /// and exposes each stream's `title` so multi-track names can be checked.
    unsafe fn probe(path: &Path) -> (u32, Vec<(i32, u32, Option<String>)>, usize) {
        let c = CString::new(path.to_str().unwrap()).unwrap();
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        let r = ffi::avformat_open_input(&mut ic, c.as_ptr(), ptr::null_mut(), ptr::null_mut());
        assert!(r >= 0, "avformat_open_input: {}", av_err(r));
        let r = ffi::avformat_find_stream_info(ic, ptr::null_mut());
        assert!(r >= 0, "avformat_find_stream_info: {}", av_err(r));

        let nb = (*ic).nb_streams;
        let read_label = |metadata: *mut ffi::AVDictionary| -> Option<String> {
            let read = |key: &str| -> Option<String> {
                let k = CString::new(key).unwrap();
                let e = ffi::av_dict_get(metadata, k.as_ptr(), ptr::null(), 0);
                if e.is_null() {
                    None
                } else {
                    Some(std::ffi::CStr::from_ptr((*e).value).to_string_lossy().into_owned())
                }
            };
            read("title").or_else(|| {
                read("handler_name")
                    .filter(|h| h != "SoundHandler" && h != "VideoHandler" && !h.is_empty())
            })
        };
        let mut streams = Vec::new();
        let mut video_index = -1i32;
        for i in 0..nb as isize {
            let st = *(*ic).streams.offset(i);
            let par = (*st).codecpar;
            let title = read_label((*st).metadata);
            streams.push(((*par).codec_type, (*par).codec_id, title));
            if (*par).codec_type == ffi::AVMEDIA_TYPE_VIDEO {
                video_index = (*st).index;
            }
        }
        let mut pkt = ffi::av_packet_alloc();
        let mut vcount = 0usize;
        while ffi::av_read_frame(ic, pkt) >= 0 {
            if (*pkt).stream_index == video_index {
                vcount += 1;
            }
            ffi::av_packet_unref(pkt);
        }
        ffi::av_packet_free(&mut pkt);
        ffi::avformat_close_input(&mut ic);
        (nb, streams, vcount)
    }

    /// Encode ~1.5 s of synthetic video, stream it through `SessionWriter`
    /// incrementally with synthetic wall-clock ticks, finish, and reopen — proves
    /// the streaming mux produces a real MP4 and the timeline maps wall-clock to
    /// the rebased session PTS.
    #[test]
    fn writes_session_and_builds_timeline() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();
        let conv = Converter::new(&d3d_device, &ctx, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");

        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            extradata: enc.extradata(),
        };
        assert!(!meta.extradata.is_empty(), "no avcC extradata");

        // The session "opens" at this wall-clock tick; the first video packet's
        // capture timestamp. We feed packets at the same offset their PTS implies.
        let base_ticks = 5_000_000_000i64; // arbitrary QPC origin
        let out = std::env::temp_dir().join("hako_session_test.mp4");
        let _ = std::fs::remove_file(&out);

        let writer = SessionWriter::start(&out, &meta, &[]).expect("start session");

        // Encode 90 frames (~1.5 s) and stream each packet straight through,
        // tagging it with a wall-clock tick derived from its PTS (as the encode
        // thread would from the capture clock).
        let mut pushed = 0u64;
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            for p in enc.encode(&nv12, i).expect("encode") {
                let wall = base_ticks + p.pts * TICKS_PER_SECOND / fps as i64;
                writer.push(&p, wall);
                pushed += 1;
            }
        }
        for p in enc.flush().expect("flush") {
            let wall = base_ticks + p.pts * TICKS_PER_SECOND / fps as i64;
            writer.push(&p, wall);
            pushed += 1;
        }
        assert!(pushed > 0, "encoder produced no packets");

        let (path, timeline) = writer.finish().expect("finish");
        assert_eq!(path, out);
        assert!(!timeline.is_empty(), "timeline has no samples");

        // The session starts at PTS 0: the first frame's wall-clock maps to ~0.
        let at_start = timeline.pts_at(base_ticks).expect("pts at start");
        assert_eq!(at_start, 0, "session must start at PTS 0");

        // One second in (1e7 ticks past base) maps near PTS = fps (1/fps units).
        let one_sec = timeline.pts_at(base_ticks + TICKS_PER_SECOND).expect("pts at +1s");
        assert!(
            (one_sec - fps as i64).abs() <= 2,
            "expected ~{} PTS one second in, got {}",
            fps,
            one_sec
        );

        let size = std::fs::metadata(&out).expect("session file").len();
        assert!(size > 0, "session mp4 is empty");

        unsafe {
            let (nb, streams, vcount) = probe(&out);
            println!("session probe: {nb} stream(s), {streams:?}, {vcount} video packets");
            assert_eq!(nb, 1, "expected a single video stream");
            assert!(
                streams
                    .iter()
                    .any(|(t, id, _)| *t == ffi::AVMEDIA_TYPE_VIDEO && *id == ffi::AV_CODEC_ID_H264),
                "missing H.264 video stream"
            );
            assert_eq!(vcount, pushed as usize, "demuxed packet count != pushed");
        }
        let _ = std::fs::remove_file(&out);
    }

    /// Stream synthetic video + real (silent) AAC through the writer and confirm
    /// the finished session has both a video and an audio stream. Exercises the
    /// two-stream live-interleave path and the audio tick→sample rebasing.
    #[test]
    fn writes_session_with_audio() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();
        let conv = Converter::new(&d3d_device, &ctx, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");
        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            extradata: enc.extradata(),
        };

        let (ameta, apackets) = crate::core::audio::encode_silence_aac(1.5);
        assert!(!apackets.is_empty(), "no AAC packets produced");

        // Video packets carry capture wall-clock; audio packets carry absolute
        // ticks from 0. Anchor the session so audio (ticks ≥ 0) lands after the
        // first video frame (also tick 0).
        let base_ticks = 0i64;
        let out = std::env::temp_dir().join("hako_session_av_test.mp4");
        let _ = std::fs::remove_file(&out);

        let audio_tracks = [("All Audio".to_string(), ameta.clone())];
        let writer = SessionWriter::start(&out, &meta, &audio_tracks).expect("start session");

        // Establish the session origin with the first video frame, then interleave
        // the rest of the video with all the audio (as separate threads would).
        let mut vpackets = Vec::new();
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            vpackets.extend(enc.encode(&nv12, i).expect("encode"));
        }
        vpackets.extend(enc.flush().expect("flush"));
        assert!(!vpackets.is_empty());

        // Push the first video packet to set the origin, then push all audio, then
        // the remaining video — a deliberately interleaved order.
        let first = &vpackets[0];
        writer.push(first, base_ticks + first.pts * TICKS_PER_SECOND / fps as i64);
        for ap in &apackets {
            writer.push_audio(0, ap);
        }
        for vp in &vpackets[1..] {
            writer.push(vp, base_ticks + vp.pts * TICKS_PER_SECOND / fps as i64);
        }

        let (_path, _timeline) = writer.finish().expect("finish");
        let (vc, ac) = writer.counts();
        println!("session counts: {vc} video, {ac} audio");
        assert_eq!(vc as usize, vpackets.len());
        assert!(ac > 0, "no audio packets written");

        unsafe {
            let (nb, streams, _vcount) = probe(&out);
            println!("A/V session probe: {nb} stream(s), {streams:?}");
            assert_eq!(nb, 2, "expected video + audio streams");
            assert!(
                streams
                    .iter()
                    .any(|(t, id, _)| *t == ffi::AVMEDIA_TYPE_VIDEO && *id == ffi::AV_CODEC_ID_H264),
                "missing H.264 video stream"
            );
            assert!(
                streams
                    .iter()
                    .any(|(t, id, _)| *t == ffi::AVMEDIA_TYPE_AUDIO && *id == ffi::AV_CODEC_ID_AAC),
                "missing AAC audio stream"
            );
        }
        let _ = std::fs::remove_file(&out);
    }

    /// Stream synthetic video + **two** named AAC tracks (master + a stem) through
    /// the writer, each pushed with its own track index, and confirm the finished
    /// session has 3 streams with the right titles. Exercises the multi-track
    /// live-interleave + per-stream PTS bookkeeping (Phase 4 session muxing).
    #[test]
    fn writes_session_with_two_audio_tracks() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();
        let conv = Converter::new(&d3d_device, &ctx, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");
        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            extradata: enc.extradata(),
        };

        let (m_meta, m_pkts) = crate::core::audio::encode_silence_aac(1.5);
        let (s_meta, s_pkts) = crate::core::audio::encode_silence_aac(1.5);
        assert!(!m_pkts.is_empty() && !s_pkts.is_empty(), "no AAC packets");

        let base_ticks = 0i64;
        let out = std::env::temp_dir().join("hako_session_2track_test.mp4");
        let _ = std::fs::remove_file(&out);

        let audio_tracks = [
            ("All Audio".to_string(), m_meta.clone()),
            ("Microphone".to_string(), s_meta.clone()),
        ];
        let writer = SessionWriter::start(&out, &meta, &audio_tracks).expect("start session");

        let mut vpackets = Vec::new();
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            vpackets.extend(enc.encode(&nv12, i).expect("encode"));
        }
        vpackets.extend(enc.flush().expect("flush"));
        assert!(!vpackets.is_empty());

        // First video frame sets the origin, then interleave both audio tracks.
        let first = &vpackets[0];
        writer.push(first, base_ticks + first.pts * TICKS_PER_SECOND / fps as i64);
        for ap in &m_pkts {
            writer.push_audio(0, ap);
        }
        for ap in &s_pkts {
            writer.push_audio(1, ap);
        }
        for vp in &vpackets[1..] {
            writer.push(vp, base_ticks + vp.pts * TICKS_PER_SECOND / fps as i64);
        }

        writer.finish().expect("finish");
        let (_vc, ac) = writer.counts();
        assert!(ac > 0, "no audio packets written");

        unsafe {
            let (nb, streams, _vcount) = probe(&out);
            println!("2-track session probe: {nb} stream(s), {streams:?}");
            assert_eq!(nb, 3, "expected video + 2 audio streams");
            let names: Vec<String> = streams
                .iter()
                .filter(|(t, _, _)| *t == ffi::AVMEDIA_TYPE_AUDIO)
                .filter_map(|(_, _, title)| title.clone())
                .collect();
            assert!(
                names.iter().any(|n| n == "All Audio"),
                "missing 'All Audio' track, got {names:?}"
            );
            assert!(
                names.iter().any(|n| n == "Microphone"),
                "missing 'Microphone' track, got {names:?}"
            );
        }
        let _ = std::fs::remove_file(&out);
    }
}
