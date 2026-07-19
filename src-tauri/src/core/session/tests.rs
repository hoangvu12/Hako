//! Integration tests for the streaming session writer.
//!
//! Each test drives a real D3D11 device and encoder, writes an MP4 through
//! [`SessionWriter`], then reopens it with `probe` to prove the file is a valid,
//! demuxable MP4 with the expected streams — so they require GPU hardware.

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
    let adapter =
        device::default_capture_index(&gpus).map(|i| device::adapter_at(i).expect("adapter_at"));
    let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
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
    let conv = Converter::new(
        &d3d_device,
        &ctx,
        w,
        h,
        w,
        h,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    )
    .expect("converter");
    let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");

    let meta = ClipMeta {
        width: w,
        height: h,
        fps,
        codec_id: ffi::AV_CODEC_ID_H264,
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
            writer.push(&p, wall, false);
            pushed += 1;
        }
    }
    for p in enc.flush().expect("flush") {
        let wall = base_ticks + p.pts * TICKS_PER_SECOND / fps as i64;
        writer.push(&p, wall, false);
        pushed += 1;
    }
    assert!(pushed > 0, "encoder produced no packets");

    let (path, output) = writer.finish().expect("finish");
    let timeline = output.timeline;
    assert_eq!(path, out);
    assert!(!timeline.is_empty(), "timeline has no samples");

    // The session starts at PTS 0: the first frame's wall-clock maps to ~0.
    let at_start = timeline.pts_at(base_ticks).expect("pts at start");
    assert_eq!(at_start, 0, "session must start at PTS 0");

    // One second in (1e7 ticks past base) maps near PTS = fps (1/fps units).
    let one_sec = timeline
        .pts_at(base_ticks + TICKS_PER_SECOND)
        .expect("pts at +1s");
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
    let adapter =
        device::default_capture_index(&gpus).map(|i| device::adapter_at(i).expect("adapter_at"));
    let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
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
    let conv = Converter::new(
        &d3d_device,
        &ctx,
        w,
        h,
        w,
        h,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    )
    .expect("converter");
    let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");
    let meta = ClipMeta {
        width: w,
        height: h,
        fps,
        codec_id: ffi::AV_CODEC_ID_H264,
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
    writer.push(
        first,
        base_ticks + first.pts * TICKS_PER_SECOND / fps as i64,
        false,
    );
    for ap in &apackets {
        writer.push_audio(0, ap);
    }
    for vp in &vpackets[1..] {
        writer.push(
            vp,
            base_ticks + vp.pts * TICKS_PER_SECOND / fps as i64,
            false,
        );
    }

    let (_path, _output) = writer.finish().expect("finish");
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
    let adapter =
        device::default_capture_index(&gpus).map(|i| device::adapter_at(i).expect("adapter_at"));
    let (d3d_device, ctx, _fl) = device::create_device(adapter.as_ref()).expect("create device");
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
    let conv = Converter::new(
        &d3d_device,
        &ctx,
        w,
        h,
        w,
        h,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
    )
    .expect("converter");
    let mut enc = Encoder::new_qsv(&d3d_device, &ctx, w, h, fps).expect("encoder");
    let meta = ClipMeta {
        width: w,
        height: h,
        fps,
        codec_id: ffi::AV_CODEC_ID_H264,
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
    writer.push(
        first,
        base_ticks + first.pts * TICKS_PER_SECOND / fps as i64,
        false,
    );
    for ap in &m_pkts {
        writer.push_audio(0, ap);
    }
    for ap in &s_pkts {
        writer.push_audio(1, ap);
    }
    for vp in &vpackets[1..] {
        writer.push(
            vp,
            base_ticks + vp.pts * TICKS_PER_SECOND / fps as i64,
            false,
        );
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
