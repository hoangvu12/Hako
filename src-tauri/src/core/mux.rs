//! MP4 stream-copy clip writer.
//!
//! Takes an IDR-aligned run of already-encoded H.264 packets (from
//! [`crate::core::buffer::PacketRing::slice_last`]) and writes them to an `.mp4`
//! by **stream copy** — `av_interleaved_write_frame`, never re-encoding. The CPU
//! only ever touches the compressed bytes (golden rule).
//!
//! The encoder runs with `AV_CODEC_FLAG_GLOBAL_HEADER`, so SPS/PPS live in
//! [`crate::core::encode::Encoder::extradata`] (avcC) and are written once into
//! the MP4 sample description here; the slice's packets carry only frame NALs.
//! PTS/DTS are rebased so the first packet starts at 0 (a clip is independent of
//! where it sat in the ring). No B-frames ⇒ `dts == pts`.
//!
//! Padding (−8s/+4s) and multi-kill window merging belong to the Valorant
//! auto-clip path; this module is the raw "given packets, write a file"
//! primitive that both the hotkey save and the auto-clipper build on.

#![allow(dead_code)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use rusty_ffmpeg::ffi;

use crate::core::audio::AudioMeta;
use crate::core::encode::{av_err, EncodedPacket};

/// Everything the muxer needs that isn't carried per-packet. Built once from the
/// encoder when capture starts (dimensions + avcC extradata) and reused for
/// every clip in the session.
#[derive(Debug, Clone)]
pub struct ClipMeta {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// FFmpeg `AV_CODEC_ID_*` of the encoded video stream (H.264/HEVC/AV1) — set
    /// from the encoder so the muxer declares the right stream type.
    pub codec_id: u32,
    /// Codec config record from the encoder (`Encoder::extradata`): avcC for
    /// H.264, hvcC for HEVC, av1C for AV1 (all produced by GLOBAL_HEADER).
    pub extradata: Vec<u8>,
}

// Raw FFmpeg flag values (ABI-stable; avoids depending on the prebuilt binding
// exporting them as constants).
const AVFMT_NOFILE: i32 = 0x0001;
const AVIO_FLAG_WRITE: i32 = 2;
const AV_PKT_FLAG_KEY: i32 = 1;
/// Required trailing zero-padding on any buffer handed to FFmpeg as extradata.
const AV_INPUT_BUFFER_PADDING_SIZE: usize = 64;

/// One audio track to interleave alongside the video as its own AAC stream.
///
/// `packets` are AAC frames whose PTS is in **absolute 100 ns QPC ticks** (the
/// audio ring's unit). `clip_start_ticks` is the wall-clock tick the clip starts
/// at (the first video frame); audio is rebased against it and anything before
/// it is dropped, so every stream shares PTS 0. `name` is written as the MP4
/// stream `title` so the editor can label tracks; track 0 is the master mix.
pub struct AudioClip<'a> {
    pub meta: &'a AudioMeta,
    pub name: &'a str,
    pub packets: &'a [EncodedPacket],
    pub clip_start_ticks: i64,
}

/// Write `packets` (in encode order, starting on a keyframe) to an MP4 at `path`
/// via stream copy, interleaving each `audio` track as its own named AAC stream
/// (track 0 = master mix; empty ⇒ video-only). `meta` supplies the video
/// dimensions + avcC extradata.
///
/// Returns the number of **video** packets written on success.
pub fn write_clip(
    path: &Path,
    meta: &ClipMeta,
    packets: &[EncodedPacket],
    audio: &[AudioClip<'_>],
) -> std::result::Result<usize, String> {
    if packets.is_empty() {
        return Err("no packets to write".into());
    }
    let path_str = path.to_str().ok_or("output path is not valid UTF-8")?;
    let c_path = CString::new(path_str).map_err(|_| "output path contains NUL")?;
    let c_mp4 = CString::new("mp4").unwrap();

    unsafe {
        // Output context bound to the mp4 muxer + target file.
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

        // Do the work in a closure so we can always run teardown afterwards.
        let result = write_inner(ofmt, &c_path, meta, packets, audio);

        // Teardown regardless of outcome: close IO then free the context (which
        // frees the stream + codecpar->extradata we allocated).
        if !(*ofmt).pb.is_null() && ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
            ffi::avio_closep(&mut (*ofmt).pb);
        }
        ffi::avformat_free_context(ofmt);

        result
    }
}

/// One packet ready to write, on a common ordering axis (100 ns ticks from the
/// clip start) so video + audio interleave correctly regardless of input order.
struct WriteOp<'a> {
    /// Sort key: time from clip start in 100 ns ticks.
    order: i64,
    stream_index: i32,
    /// PTS/DTS in the packet's natural time base before rescale.
    pts: i64,
    src_tb: ffi::AVRational,
    dst_tb: ffi::AVRational,
    keyframe: bool,
    data: &'a [u8],
}

/// Copy `bytes` into a freshly-allocated FFmpeg-owned extradata buffer (with the
/// mandatory trailing padding) and attach it to `par`.
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

/// Inner writer: assumes `ofmt` is allocated; never frees it (caller does).
unsafe fn write_inner(
    ofmt: *mut ffi::AVFormatContext,
    c_path: &CString,
    meta: &ClipMeta,
    packets: &[EncodedPacket],
    audio: &[AudioClip<'_>],
) -> std::result::Result<usize, String> {
    let fps = meta.fps.max(1) as i32;

    // Stream 0: H.264 video, time base = 1/fps (our packet PTS units).
    let st_v = ffi::avformat_new_stream(ofmt, ptr::null());
    if st_v.is_null() {
        return Err("avformat_new_stream(video) failed".into());
    }
    (*st_v).id = 0;
    (*st_v).time_base = ffi::AVRational { num: 1, den: fps };
    let par = (*st_v).codecpar;
    (*par).codec_type = ffi::AVMEDIA_TYPE_VIDEO;
    (*par).codec_id = meta.codec_id;
    // codec_tag left 0 so the MP4 muxer picks the right fourcc (avc1/hvc1/av01).
    (*par).width = meta.width as i32;
    (*par).height = meta.height as i32;
    (*par).format = ffi::AV_PIX_FMT_NV12; // informational for a copy stream
    set_extradata(par, &meta.extradata)?;

    // Streams 1..N (optional): one named AAC track each. Track 0 (first in the
    // slice) is the master mix; the rest are stems when "Separate audio tracks"
    // is on. A track is only declared when it has packets + the
    // AudioSpecificConfig the mp4 esds needs, so a silent/empty stem is skipped.
    // We hold the AVStream pointer per track so its (possibly write_header-
    // adjusted) time base can be read back when building the write ops.
    let title_key = CString::new("title").unwrap();
    let handler_key = CString::new("handler_name").unwrap();
    let mut audio_streams: Vec<(*mut ffi::AVStream, &AudioClip<'_>)> = Vec::new();
    for a in audio {
        if a.packets.is_empty() || a.meta.extradata.is_empty() {
            continue;
        }
        let st_a = ffi::avformat_new_stream(ofmt, ptr::null());
        if st_a.is_null() {
            return Err("avformat_new_stream(audio) failed".into());
        }
        (*st_a).id = (audio_streams.len() as i32) + 1;
        (*st_a).time_base = ffi::AVRational {
            num: 1,
            den: a.meta.sample_rate.max(1) as i32,
        };
        let apar = (*st_a).codecpar;
        (*apar).codec_type = ffi::AVMEDIA_TYPE_AUDIO;
        (*apar).codec_id = ffi::AV_CODEC_ID_AAC;
        (*apar).sample_rate = a.meta.sample_rate as i32;
        (*apar).format = ffi::AV_SAMPLE_FMT_FLTP; // informational for copy
        ffi::av_channel_layout_default(&mut (*apar).ch_layout, a.meta.channels.max(1) as i32);
        set_extradata(apar, &a.meta.extradata)?;
        // Label the stream so the editor can show the track name. `handler_name`
        // (the trak `hdlr` box) survives the MP4 round-trip most reliably; we
        // also set `title` (udta) for tools that read that instead.
        if !a.name.is_empty() {
            if let Ok(c_name) = CString::new(a.name) {
                ffi::av_dict_set(
                    &mut (*st_a).metadata,
                    handler_key.as_ptr(),
                    c_name.as_ptr(),
                    0,
                );
                ffi::av_dict_set(
                    &mut (*st_a).metadata,
                    title_key.as_ptr(),
                    c_name.as_ptr(),
                    0,
                );
            }
        }
        audio_streams.push((st_a, a));
    }

    // Open the file (mp4 is not a file-less format, but check the flag anyway).
    if ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
        let r = ffi::avio_open(&mut (*ofmt).pb, c_path.as_ptr(), AVIO_FLAG_WRITE);
        if r < 0 {
            return Err(format!("avio_open: {}", av_err(r)));
        }
    }

    // `movflags=faststart` runs a second pass on trailer-write that relocates the
    // `moov` atom to the front of the file. Without it `moov` lands at the end, so
    // a player (and our range-streaming protocol) must fetch the tail before it
    // can start — costing startup + first-seek latency in the editor.
    let mut hdr_opts: *mut ffi::AVDictionary = ptr::null_mut();
    let mov_k = CString::new("movflags").unwrap();
    let mov_v = CString::new("faststart").unwrap();
    ffi::av_dict_set(&mut hdr_opts, mov_k.as_ptr(), mov_v.as_ptr(), 0);
    let r = ffi::avformat_write_header(ofmt, &mut hdr_opts);
    ffi::av_dict_free(&mut hdr_opts);
    if r < 0 {
        return Err(format!("avformat_write_header: {}", av_err(r)));
    }

    // Build the ordered write list. Video first: rebase to 0 and force a single
    // strictly-increasing timestamp for pts AND dts — capture is VFR (PTS from
    // SystemRelativeTime) so 100ns→1/fps rounding can collide, and NVENC doesn't
    // propagate per-frame PTS; bumping only dts would trip the `pts < dts` check.
    let vid_dst_tb = (*st_v).time_base; // write_header may have adjusted it
    let vid_src_tb = ffi::AVRational { num: 1, den: fps };
    let first_pts = packets[0].pts;
    let audio_cap: usize = audio_streams.iter().map(|(_, a)| a.packets.len()).sum();
    let mut ops: Vec<WriteOp> = Vec::with_capacity(packets.len() + audio_cap);
    let mut last_ts = i64::MIN;
    for p in packets {
        let mut ts = p.pts - first_pts;
        if ts <= last_ts {
            ts = last_ts + 1;
        }
        last_ts = ts;
        // Time from clip start in 100ns ticks (the ordering axis).
        let order = (ts as i128 * 10_000_000i128 / fps as i128) as i64;
        ops.push(WriteOp {
            order,
            stream_index: 0,
            pts: ts,
            src_tb: vid_src_tb,
            dst_tb: vid_dst_tb,
            keyframe: p.keyframe,
            data: &p.data,
        });
    }
    let video_count = ops.len();

    for (st_a, a) in &audio_streams {
        let sr = a.meta.sample_rate.max(1) as i64;
        let stream_index = (**st_a).index;
        let aud_dst_tb = (**st_a).time_base; // write_header may have adjusted it
        let aud_src_tb = ffi::AVRational {
            num: 1,
            den: sr as i32,
        };
        for p in a.packets {
            let rel_ticks = p.pts - a.clip_start_ticks;
            if rel_ticks < 0 {
                continue; // before the clip start — drop (small leading silence)
            }
            let pts_samples = (rel_ticks as i128 * sr as i128 / 10_000_000i128) as i64;
            ops.push(WriteOp {
                order: rel_ticks,
                stream_index,
                pts: pts_samples,
                src_tb: aud_src_tb,
                dst_tb: aud_dst_tb,
                keyframe: true,
                data: &p.data,
            });
        }
    }

    // Stable sort by time so interleaving is well-ordered before handing to
    // av_interleaved_write_frame (which still does final dts ordering).
    ops.sort_by_key(|o| o.order);

    let mut pkt = ffi::av_packet_alloc();
    if pkt.is_null() {
        return Err("av_packet_alloc failed".into());
    }

    // Write loop kept in its own scope so we free `pkt` on every path.
    let write_result = (|| -> std::result::Result<usize, String> {
        for op in &ops {
            if ffi::av_new_packet(pkt, op.data.len() as i32) < 0 {
                return Err("av_new_packet failed".into());
            }
            ptr::copy_nonoverlapping(op.data.as_ptr(), (*pkt).data, op.data.len());
            (*pkt).pts = op.pts;
            (*pkt).dts = op.pts; // no B-frames in either stream ⇒ dts == pts
            (*pkt).stream_index = op.stream_index;
            (*pkt).flags = if op.keyframe { AV_PKT_FLAG_KEY } else { 0 };
            ffi::av_packet_rescale_ts(pkt, op.src_tb, op.dst_tb);

            // Takes ownership of pkt's buffer and unrefs it (ready to reuse).
            let r = ffi::av_interleaved_write_frame(ofmt, pkt);
            if r < 0 {
                return Err(format!("av_interleaved_write_frame: {}", av_err(r)));
            }
        }
        // Finalize moov/trailer.
        let r = ffi::av_write_trailer(ofmt);
        if r < 0 {
            return Err(format!("av_write_trailer: {}", av_err(r)));
        }
        Ok(video_count)
    })();

    ffi::av_packet_free(&mut pkt);
    write_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::PacketRing;
    use crate::core::convert::Converter;
    use crate::core::device;
    use crate::core::encode::Encoder;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    /// Reopen an MP4 and return (stream count, codec_id, width, height, packet
    /// count) — proves the file we wrote is a valid, demuxable H.264 MP4.
    unsafe fn probe_mp4(path: &Path) -> (u32, u32, i32, i32, usize) {
        let c = CString::new(path.to_str().unwrap()).unwrap();
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        let r = ffi::avformat_open_input(&mut ic, c.as_ptr(), ptr::null_mut(), ptr::null_mut());
        assert!(r >= 0, "avformat_open_input: {}", av_err(r));
        let r = ffi::avformat_find_stream_info(ic, ptr::null_mut());
        assert!(r >= 0, "avformat_find_stream_info: {}", av_err(r));

        let nb = (*ic).nb_streams;
        let st = *(*ic).streams; // stream 0
        let par = (*st).codecpar;
        let (codec_id, w, h) = ((*par).codec_id, (*par).width, (*par).height);

        let mut pkt = ffi::av_packet_alloc();
        let mut count = 0usize;
        while ffi::av_read_frame(ic, pkt) >= 0 {
            count += 1;
            ffi::av_packet_unref(pkt);
        }
        ffi::av_packet_free(&mut pkt);
        ffi::avformat_close_input(&mut ic);
        (nb, codec_id, w, h, count)
    }

    /// End-to-end: synthetic BGRA → NV12 → `h264_qsv` → ring → `write_clip` →
    /// reopen and validate. Proves the stream-copy mux produces a real MP4.
    #[test]
    fn writes_a_playable_mp4_from_encoded_packets() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        // Synthetic BGRA source frame.
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
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
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();

        let conv = Converter::new(&d3d_device, &ctx, w, h, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");

        // Encode ~1.5 s so the ring holds more than one GOP (keyint = 1 s).
        let mut ring = PacketRing::new(fps, 30);
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12 tex");
            conv.convert(&bgra, &nv12).expect("convert");
            for p in enc.encode(&nv12, i).expect("encode") {
                ring.push(p);
            }
        }
        for p in enc.flush().expect("flush") {
            ring.push(p);
        }

        let extradata = enc.extradata();
        println!("encoder extradata = {} bytes", extradata.len());
        assert!(
            !extradata.is_empty(),
            "encoder produced no avcC extradata (GLOBAL_HEADER not honored?)"
        );

        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            codec_id: ffi::AV_CODEC_ID_H264,
            extradata,
        };
        let packets = ring.slice_last(1);
        assert!(packets.first().map(|p| p.keyframe).unwrap_or(false));

        let out = std::env::temp_dir().join("hako_mux_test.mp4");
        let _ = std::fs::remove_file(&out);
        let written = write_clip(&out, &meta, &packets, &[]).expect("write_clip");
        println!("wrote {written} packets to {}", out.display());

        let size = std::fs::metadata(&out).expect("output exists").len();
        assert!(size > 0, "output mp4 is empty");

        unsafe {
            let (nb, codec_id, pw, ph, count) = probe_mp4(&out);
            println!("probe: {nb} stream(s), codec_id={codec_id}, {pw}x{ph}, {count} packets",);
            assert_eq!(nb, 1, "expected exactly one stream");
            assert_eq!(codec_id, ffi::AV_CODEC_ID_H264, "stream is not H.264");
            assert_eq!(pw, w as i32);
            assert_eq!(ph, h as i32);
            assert_eq!(count, written, "demuxed packet count != written");
        }

        let _ = std::fs::remove_file(&out);
    }

    /// The custom label written for a stream: the `title` (udta) if present,
    /// else the `hdlr` `handler_name` when it isn't FFmpeg's default
    /// ("SoundHandler"/"VideoHandler"). `None` if unlabeled.
    unsafe fn stream_label(metadata: *mut ffi::AVDictionary) -> Option<String> {
        let read = |key: &str| -> Option<String> {
            let k = CString::new(key).unwrap();
            let e = ffi::av_dict_get(metadata, k.as_ptr(), ptr::null(), 0);
            if e.is_null() {
                None
            } else {
                Some(
                    std::ffi::CStr::from_ptr((*e).value)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        read("title").or_else(|| {
            read("handler_name")
                .filter(|h| h != "SoundHandler" && h != "VideoHandler" && !h.is_empty())
        })
    }

    /// Probe stream count + the set of (codec_type, codec_id, title) present, to
    /// verify the video + audio tracks (and their names) were written.
    unsafe fn probe_streams(path: &Path) -> Vec<(i32, u32, Option<String>)> {
        let c = CString::new(path.to_str().unwrap()).unwrap();
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        let r = ffi::avformat_open_input(&mut ic, c.as_ptr(), ptr::null_mut(), ptr::null_mut());
        assert!(r >= 0, "avformat_open_input: {}", av_err(r));
        let r = ffi::avformat_find_stream_info(ic, ptr::null_mut());
        assert!(r >= 0, "avformat_find_stream_info: {}", av_err(r));
        let mut out = Vec::new();
        for i in 0..(*ic).nb_streams as isize {
            let st = *(*ic).streams.offset(i);
            let par = (*st).codecpar;
            let title = stream_label((*st).metadata);
            out.push(((*par).codec_type, (*par).codec_id, title));
        }
        ffi::avformat_close_input(&mut ic);
        out
    }

    /// End-to-end A/V: synthetic H.264 video + real (silent) AAC audio →
    /// `write_clip` with an audio track → reopen and confirm the MP4 has both a
    /// video and an audio stream. Proves the two-stream interleave path.
    #[test]
    fn writes_mp4_with_video_and_audio() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
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
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();
        let conv = Converter::new(&d3d_device, &ctx, w, h, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");

        let mut ring = PacketRing::new(fps, 30);
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            for p in enc.encode(&nv12, i).expect("encode") {
                ring.push(p);
            }
        }
        for p in enc.flush().expect("flush") {
            ring.push(p);
        }
        let vpackets = ring.slice_last(1);
        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            codec_id: ffi::AV_CODEC_ID_H264,
            extradata: enc.extradata(),
        };

        // Real AAC (silence), ticks from 0 — clip starts at tick 0 to match the
        // video (whose first PTS rebases to 0).
        let (ameta, apackets) = crate::core::audio::encode_silence_aac(1.5);
        assert!(!apackets.is_empty(), "no AAC packets produced");
        let audio = AudioClip {
            meta: &ameta,
            name: "All Audio",
            packets: &apackets,
            clip_start_ticks: 0,
        };

        let out = std::env::temp_dir().join("hako_mux_av_test.mp4");
        let _ = std::fs::remove_file(&out);
        write_clip(&out, &meta, &vpackets, std::slice::from_ref(&audio)).expect("write_clip");

        unsafe {
            let streams = probe_streams(&out);
            println!("A/V probe: {streams:?}");
            assert_eq!(streams.len(), 2, "expected video + audio streams");
            assert!(
                streams.iter().any(|(t, id, _)| *t == ffi::AVMEDIA_TYPE_VIDEO
                    && *id == ffi::AV_CODEC_ID_H264),
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

    /// Multi-track: one video + **two** named AAC streams (master + a stem) →
    /// confirm the MP4 has 3 streams and both audio tracks carry their `title`.
    /// Proves the per-track stream declaration + interleave (Phase 4 muxing).
    #[test]
    fn writes_mp4_with_two_audio_tracks() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
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
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();
        let conv = Converter::new(&d3d_device, &ctx, w, h, w, h).expect("converter");
        let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");

        let mut ring = PacketRing::new(fps, 30);
        for i in 0..90i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            for p in enc.encode(&nv12, i).expect("encode") {
                ring.push(p);
            }
        }
        for p in enc.flush().expect("flush") {
            ring.push(p);
        }
        let vpackets = ring.slice_last(1);
        let meta = ClipMeta {
            width: w,
            height: h,
            fps,
            codec_id: ffi::AV_CODEC_ID_H264,
            extradata: enc.extradata(),
        };

        // Two independent AAC streams (silence) — master + one stem.
        let (m_meta, m_pkts) = crate::core::audio::encode_silence_aac(1.5);
        let (s_meta, s_pkts) = crate::core::audio::encode_silence_aac(1.5);
        assert!(!m_pkts.is_empty() && !s_pkts.is_empty(), "no AAC packets");
        let tracks = [
            AudioClip {
                meta: &m_meta,
                name: "All Audio",
                packets: &m_pkts,
                clip_start_ticks: 0,
            },
            AudioClip {
                meta: &s_meta,
                name: "Microphone",
                packets: &s_pkts,
                clip_start_ticks: 0,
            },
        ];

        let out = std::env::temp_dir().join("hako_mux_2track_test.mp4");
        let _ = std::fs::remove_file(&out);
        write_clip(&out, &meta, &vpackets, &tracks).expect("write_clip");

        unsafe {
            let streams = probe_streams(&out);
            println!("2-track probe: {streams:?}");
            assert_eq!(streams.len(), 3, "expected video + 2 audio streams");
            let audio: Vec<&(i32, u32, Option<String>)> = streams
                .iter()
                .filter(|(t, _, _)| *t == ffi::AVMEDIA_TYPE_AUDIO)
                .collect();
            assert_eq!(audio.len(), 2, "expected exactly two audio streams");
            assert!(
                audio.iter().all(|(_, id, _)| *id == ffi::AV_CODEC_ID_AAC),
                "audio streams must be AAC"
            );
            let names: Vec<String> = audio
                .iter()
                .filter_map(|(_, _, title)| title.clone())
                .collect();
            assert!(
                names.iter().any(|n| n == "All Audio"),
                "missing 'All Audio' track title, got {names:?}"
            );
            assert!(
                names.iter().any(|n| n == "Microphone"),
                "missing 'Microphone' track title, got {names:?}"
            );
        }
        let _ = std::fs::remove_file(&out);
    }
}
