//! Editor export re-mux: pick / re-mix a multi-track clip's audio stems.
//!
//! A multi-track clip (recorded with "Separate audio tracks") carries one named
//! AAC stream per track — track 0 is the master "All Audio" mix and tracks 1..N
//! are the raw stems (Game / Mic / Discord / …, gain 1.0). Browsers can only play
//! the *first* audio track, so per-track mute/solo/volume in the editor is a
//! **pre-export selection**, applied here by re-muxing:
//!
//! - [`probe_audio_tracks`] reports the file's audio streams (count + names) so
//!   the editor can label the per-track controls without a library schema change.
//! - [`remux_with_tracks`] writes a new clip whose single audio track is the
//!   chosen stems mixed at their volumes:
//!   - **0 stems** → video-only (delegates to [`crate::library::trim`]).
//!   - **1 stem at unity gain** → loss-less **stream copy** of that stem
//!     (delegates to [`trim::trim_keeping_audio`]) — the CPU never re-encodes.
//!   - **otherwise** → decode the selected stems to PCM, mix (gain-scaled),
//!     re-encode one AAC master, and interleave it with the stream-copied video.
//!
//! The video is always stream-copied and keyframe-aligned exactly like a trim;
//! `[start, end)` trims and the track mix are applied in the same pass.

#![allow(dead_code)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use rusty_ffmpeg::ffi;

use crate::core::audio::encode_pcm_to_aac;
use crate::core::encode::av_err;
use crate::library::trim::{self, TrimResult};

// ABI-stable raw flag values (mirrors trim.rs / mux.rs).
const AVFMT_NOFILE: i32 = 0x0001;
const AVIO_FLAG_WRITE: i32 = 2;
const AV_PKT_FLAG_KEY: i32 = 1;
const AVSEEK_FLAG_BACKWARD: i32 = 1;
const AV_TIME_BASE: i64 = 1_000_000;
const AV_NOPTS_VALUE: i64 = i64::MIN;
/// Required trailing zero-padding on a buffer handed to FFmpeg as extradata.
const AV_INPUT_BUFFER_PADDING_SIZE: usize = 64;
/// The mixed master track is 48 kHz stereo (matches the capture mix rate).
const MIX_RATE: i32 = 48_000;
const MIX_CHANNELS: i32 = 2;
/// 100 ns ticks per second (audio packet PTS unit from `encode_pcm_to_aac`).
const TICKS_PER_SECOND: i64 = 10_000_000;

const TB_Q: ffi::AVRational = ffi::AVRational {
    num: 1,
    den: AV_TIME_BASE as i32,
};

/// One of a clip's audio streams, for the editor's per-track controls.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioTrackInfo {
    /// 0-based index among the file's **audio** streams (0 = master "All Audio").
    pub index: u32,
    /// Track label read from the MP4 (`title`/`handler_name`), or a fallback.
    pub name: String,
}

/// A selected stem to mix into the exported master: its [`AudioTrackInfo::index`],
/// a linear `gain` (0..1, the editor's 0–100 volume ÷ 100), and whether to run
/// offline noise suppression on it (the editor's per-stem "noise cancel", used
/// for the mic stem). A denoised stem can never take the stream-copy fast path —
/// its samples are rewritten, so it's always decoded and re-encoded.
#[derive(Debug, Clone, Copy)]
pub struct TrackSel {
    pub index: u32,
    pub gain: f32,
    pub denoise: bool,
}

/// Probe `input` for its audio streams (count + names), in file order. Audio
/// stream 0 is the master mix; 1..N are the stems. Mirrors the `probe_streams`
/// label logic in `mux.rs`/`session.rs` (read `title`, else a non-default
/// `handler_name`).
pub fn probe_audio_tracks(input: &Path) -> Result<Vec<AudioTrackInfo>, String> {
    let c_in = CString::new(input.to_str().ok_or("input path not UTF-8")?)
        .map_err(|_| "input path has NUL")?;
    unsafe {
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        if ffi::avformat_open_input(&mut ic, c_in.as_ptr(), ptr::null_mut(), ptr::null_mut()) < 0 {
            return Err("avformat_open_input failed".into());
        }
        let result = (|| -> Result<Vec<AudioTrackInfo>, String> {
            if ffi::avformat_find_stream_info(ic, ptr::null_mut()) < 0 {
                return Err("find_stream_info failed".into());
            }
            let mut out = Vec::new();
            let mut ordinal = 0u32;
            for i in 0..(*ic).nb_streams as isize {
                let st = *(*ic).streams.offset(i);
                let par = (*st).codecpar;
                if (*par).codec_type != ffi::AVMEDIA_TYPE_AUDIO {
                    continue;
                }
                let name = read_label((*st).metadata).unwrap_or_else(|| {
                    if ordinal == 0 {
                        "All Audio".to_string()
                    } else {
                        format!("Track {ordinal}")
                    }
                });
                out.push(AudioTrackInfo {
                    index: ordinal,
                    name,
                });
                ordinal += 1;
            }
            Ok(out)
        })();
        ffi::avformat_close_input(&mut ic);
        result
    }
}

/// Read a stream's custom label: the `title` (udta) if present, else the `hdlr`
/// `handler_name` when it isn't FFmpeg's default. `None` if unlabeled.
unsafe fn read_label(metadata: *mut ffi::AVDictionary) -> Option<String> {
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

/// Export `input` → `output` over `[start, end)` seconds, with the output's audio
/// being `selections` (the chosen stems) mixed at their gains. See the module
/// docs for the three cases (video-only / stream-copy / re-encode mix).
pub fn remux_with_tracks(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    selections: &[TrackSel],
) -> Result<TrimResult, String> {
    if !(end > start) {
        return Err("export end must be after start".into());
    }

    // Map the requested audio ordinals onto absolute input stream indices,
    // dropping any that don't exist (the file may have fewer tracks than the UI
    // last saw). Track 0 is the master; the editor normally selects stems (≥1).
    let audio_abs = audio_stream_indices(input)?;
    let mut sel: Vec<(i32, f32, bool)> = Vec::new();
    for s in selections {
        if let Some(&abs) = audio_abs.get(s.index as usize) {
            sel.push((abs, s.gain.max(0.0), s.denoise));
        }
    }

    // 0 stems → video-only. 1 stem at unity gain *and not denoised* → loss-less
    // stream copy. A denoised stem rewrites its samples, so it must go through
    // the decode→enhance→re-encode path even at unity gain.
    if sel.is_empty() {
        return trim::trim_clip(input, output, start, end, true);
    }
    if sel.len() == 1 && (sel[0].1 - 1.0).abs() < 1e-3 && !sel[0].2 {
        return trim::trim_keeping_audio(input, output, start, end, sel[0].0);
    }

    remux_mix(input, output, start, end, &sel)
}

/// The absolute input stream indices of every audio stream, in file order
/// (index 0 of the returned vec = audio ordinal 0 = master).
fn audio_stream_indices(input: &Path) -> Result<Vec<i32>, String> {
    let c_in = CString::new(input.to_str().ok_or("input path not UTF-8")?)
        .map_err(|_| "input path has NUL")?;
    unsafe {
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        if ffi::avformat_open_input(&mut ic, c_in.as_ptr(), ptr::null_mut(), ptr::null_mut()) < 0 {
            return Err("avformat_open_input failed".into());
        }
        let result = (|| -> Result<Vec<i32>, String> {
            if ffi::avformat_find_stream_info(ic, ptr::null_mut()) < 0 {
                return Err("find_stream_info failed".into());
            }
            let mut out = Vec::new();
            for i in 0..(*ic).nb_streams as i32 {
                let st = *(*ic).streams.offset(i as isize);
                if (*(*st).codecpar).codec_type == ffi::AVMEDIA_TYPE_AUDIO {
                    out.push(i);
                }
            }
            Ok(out)
        })();
        ffi::avformat_close_input(&mut ic);
        result
    }
}

/// One ready-to-write packet on a common ordering axis (microseconds from clip
/// start) so video + the freshly-mixed audio interleave correctly.
struct BufOp {
    order_us: i64,
    is_video: bool,
    /// PTS/DTS in `src_tb` units (video: µs/`TB_Q`; audio: samples/`1/48000`).
    pts: i64,
    dts: i64,
    src_tb: ffi::AVRational,
    keyframe: bool,
    data: Vec<u8>,
}

/// Decode the selected stems → mix → re-encode a single AAC master, and write it
/// interleaved with the stream-copied (keyframe-aligned) video over `[start,
/// end)`. `sel` is `(absolute_stream_index, gain)` and is guaranteed non-empty.
fn remux_mix(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    sel: &[(i32, f32, bool)],
) -> Result<TrimResult, String> {
    let c_in = CString::new(input.to_str().ok_or("input path not UTF-8")?)
        .map_err(|_| "input path has NUL")?;
    let c_out = CString::new(output.to_str().ok_or("output path not UTF-8")?)
        .map_err(|_| "output path has NUL")?;
    let c_mp4 = CString::new("mp4").unwrap();

    unsafe {
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        if ffi::avformat_open_input(&mut ic, c_in.as_ptr(), ptr::null_mut(), ptr::null_mut()) < 0 {
            return Err("avformat_open_input failed".into());
        }
        let mut ofmt: *mut ffi::AVFormatContext = ptr::null_mut();

        let result = (|| -> Result<TrimResult, String> {
            if ffi::avformat_find_stream_info(ic, ptr::null_mut()) < 0 {
                return Err("find_stream_info failed".into());
            }

            // First video stream (kept, copied) + its dimensions.
            let nb = (*ic).nb_streams as i32;
            let mut video_idx = -1i32;
            let (mut vw, mut vh) = (0i64, 0i64);
            for i in 0..nb {
                let par = (*(*(*ic).streams.offset(i as isize))).codecpar;
                if (*par).codec_type == ffi::AVMEDIA_TYPE_VIDEO && video_idx < 0 {
                    video_idx = i;
                    vw = (*par).width as i64;
                    vh = (*par).height as i64;
                }
            }
            if video_idx < 0 {
                return Err("no video stream in clip".into());
            }

            // Decode + mix the selected stems into one 48 kHz stereo buffer
            // (whole-file; sliced to the trim window after the video pass fixes
            // the keyframe-aligned start).
            let mixed = decode_and_mix(ic, sel)?;

            // --- video copy ops + keyframe offset -----------------------------
            let mut vops: Vec<BufOp> = Vec::new();
            let (offset_us, first_v, last_v) =
                collect_video_ops(ic, video_idx, start, end, &mut vops)?;
            if vops.is_empty() {
                return Err("no keyframe found in the selected range".into());
            }

            // --- slice the mix to the same window + encode an AAC master ------
            let mixed_frames = (mixed.len() / 2) as i64;
            let start_frame =
                (offset_us as i128 * MIX_RATE as i128 / AV_TIME_BASE as i128) as i64;
            let end_us = (end * AV_TIME_BASE as f64) as i64;
            let end_frame = (end_us as i128 * MIX_RATE as i128 / AV_TIME_BASE as i128) as i64;
            let from = start_frame.clamp(0, mixed_frames);
            let to = end_frame.clamp(from, mixed_frames);
            let window = &mixed[(from * 2) as usize..(to * 2) as usize];

            let mut aops: Vec<BufOp> = Vec::new();
            let audio_meta = if window.is_empty() {
                None // stems ended before the window — fall back to video-only
            } else {
                let (ameta, apackets) = encode_pcm_to_aac(window)?;
                for p in &apackets {
                    // p.pts is in 100 ns ticks from 0 (window start).
                    let pts_samples =
                        (p.pts as i128 * MIX_RATE as i128 / TICKS_PER_SECOND as i128) as i64;
                    aops.push(BufOp {
                        order_us: p.pts / (TICKS_PER_SECOND / AV_TIME_BASE),
                        is_video: false,
                        pts: pts_samples,
                        dts: pts_samples,
                        src_tb: ffi::AVRational {
                            num: 1,
                            den: MIX_RATE,
                        },
                        keyframe: true,
                        data: p.data.clone(),
                    });
                }
                (!aops.is_empty()).then_some(ameta)
            };

            // --- output context + streams -------------------------------------
            let r = ffi::avformat_alloc_output_context2(
                &mut ofmt,
                ptr::null_mut(),
                c_mp4.as_ptr(),
                c_out.as_ptr(),
            );
            if r < 0 || ofmt.is_null() {
                return Err(format!("alloc_output_context2: {}", av_err(r)));
            }

            // Video stream: copy the input video's codec parameters.
            let in_vst = *(*ic).streams.offset(video_idx as isize);
            let out_vst = ffi::avformat_new_stream(ofmt, ptr::null());
            if out_vst.is_null() {
                return Err("avformat_new_stream(video) failed".into());
            }
            if ffi::avcodec_parameters_copy((*out_vst).codecpar, (*in_vst).codecpar) < 0 {
                return Err("avcodec_parameters_copy(video) failed".into());
            }
            (*(*out_vst).codecpar).codec_tag = 0;
            (*out_vst).time_base = (*in_vst).time_base;

            // Audio stream: a fresh AAC master from the encoder (when we have one).
            let out_ast = if let Some(meta) = &audio_meta {
                let st = ffi::avformat_new_stream(ofmt, ptr::null());
                if st.is_null() {
                    return Err("avformat_new_stream(audio) failed".into());
                }
                (*st).time_base = ffi::AVRational {
                    num: 1,
                    den: MIX_RATE,
                };
                let apar = (*st).codecpar;
                (*apar).codec_type = ffi::AVMEDIA_TYPE_AUDIO;
                (*apar).codec_id = ffi::AV_CODEC_ID_AAC;
                (*apar).sample_rate = meta.sample_rate as i32;
                (*apar).format = ffi::AV_SAMPLE_FMT_FLTP;
                ffi::av_channel_layout_default(&mut (*apar).ch_layout, MIX_CHANNELS);
                set_extradata(apar, &meta.extradata)?;
                let title = CString::new("All Audio").unwrap();
                let title_k = CString::new("title").unwrap();
                let handler_k = CString::new("handler_name").unwrap();
                ffi::av_dict_set(&mut (*st).metadata, handler_k.as_ptr(), title.as_ptr(), 0);
                ffi::av_dict_set(&mut (*st).metadata, title_k.as_ptr(), title.as_ptr(), 0);
                Some(st)
            } else {
                None
            };

            // --- open file + faststart header ---------------------------------
            if ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
                let r = ffi::avio_open(&mut (*ofmt).pb, c_out.as_ptr(), AVIO_FLAG_WRITE);
                if r < 0 {
                    return Err(format!("avio_open: {}", av_err(r)));
                }
            }
            let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
            let mov_k = CString::new("movflags").unwrap();
            let mov_v = CString::new("faststart").unwrap();
            ffi::av_dict_set(&mut opts, mov_k.as_ptr(), mov_v.as_ptr(), 0);
            let r = ffi::avformat_write_header(ofmt, &mut opts);
            ffi::av_dict_free(&mut opts);
            if r < 0 {
                return Err(format!("write_header: {}", av_err(r)));
            }

            // Stream indices + (header-adjusted) destination time bases.
            let v_index = (*out_vst).index;
            let v_dst_tb = (*out_vst).time_base;
            let (a_index, a_dst_tb) = match out_ast {
                Some(st) => ((*st).index, (*st).time_base),
                None => (-1, ffi::AVRational { num: 1, den: 1 }),
            };

            // --- write all ops in time order ----------------------------------
            let mut ops = vops;
            if a_index >= 0 {
                ops.extend(aops);
            }
            ops.sort_by_key(|o| o.order_us);

            let pkt = ffi::av_packet_alloc();
            if pkt.is_null() {
                return Err("av_packet_alloc failed".into());
            }
            let write = (|| -> Result<(), String> {
                for op in &ops {
                    let (stream_index, dst_tb) = if op.is_video {
                        (v_index, v_dst_tb)
                    } else {
                        (a_index, a_dst_tb)
                    };
                    if stream_index < 0 {
                        continue;
                    }
                    if ffi::av_new_packet(pkt, op.data.len() as i32) < 0 {
                        return Err("av_new_packet failed".into());
                    }
                    ptr::copy_nonoverlapping(op.data.as_ptr(), (*pkt).data, op.data.len());
                    (*pkt).pts = ffi::av_rescale_q(op.pts, op.src_tb, dst_tb);
                    (*pkt).dts = ffi::av_rescale_q(op.dts, op.src_tb, dst_tb);
                    if (*pkt).pts < (*pkt).dts {
                        (*pkt).pts = (*pkt).dts;
                    }
                    (*pkt).stream_index = stream_index;
                    (*pkt).flags = if op.keyframe { AV_PKT_FLAG_KEY } else { 0 };
                    (*pkt).pos = -1;
                    let r = ffi::av_interleaved_write_frame(ofmt, pkt);
                    if r < 0 {
                        return Err(format!("interleaved_write_frame: {}", av_err(r)));
                    }
                }
                Ok(())
            })();
            let mut p = pkt;
            ffi::av_packet_free(&mut p);
            write?;

            let r = ffi::av_write_trailer(ofmt);
            if r < 0 {
                return Err(format!("write_trailer: {}", av_err(r)));
            }

            let duration_secs = if last_v > first_v {
                (last_v - first_v) as f64 / AV_TIME_BASE as f64
            } else {
                end - start
            };
            Ok(TrimResult {
                width: vw,
                height: vh,
                duration_secs,
            })
        })();

        // Teardown (mirror trim.rs).
        if !ofmt.is_null() {
            if !(*ofmt).pb.is_null() && ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
                ffi::avio_closep(&mut (*ofmt).pb);
            }
            ffi::avformat_free_context(ofmt);
        }
        ffi::avformat_close_input(&mut ic);
        result
    }
}

/// Collect the video stream's packets over `[start, end)` as keyframe-aligned,
/// rebased [`BufOp`]s (matching `trim.rs`'s copy loop). Pushes onto `out` and
/// returns `(offset_us, first_order_us, last_order_us)` — the global µs of the
/// first kept keyframe and the span of kept video. Seeks `ic` to the keyframe at
/// or before `start` first.
unsafe fn collect_video_ops(
    ic: *mut ffi::AVFormatContext,
    video_idx: i32,
    start: f64,
    end: f64,
    out: &mut Vec<BufOp>,
) -> Result<(i64, i64, i64), String> {
    let vst = *(*ic).streams.offset(video_idx as isize);
    let start_ts_v = ffi::av_rescale_q(
        (start * AV_TIME_BASE as f64) as i64,
        TB_Q,
        (*vst).time_base,
    );
    ffi::av_seek_frame(ic, video_idx, start_ts_v, AVSEEK_FLAG_BACKWARD);

    let start_global = (start * AV_TIME_BASE as f64) as i64;
    let end_global = (end * AV_TIME_BASE as f64) as i64;

    let pkt = ffi::av_packet_alloc();
    if pkt.is_null() {
        return Err("av_packet_alloc failed".into());
    }
    let mut offset = AV_NOPTS_VALUE;
    let mut started = false;
    let mut first_v = AV_NOPTS_VALUE;
    let mut last_v = AV_NOPTS_VALUE;

    let res = (|| -> Result<(), String> {
        while ffi::av_read_frame(ic, pkt) >= 0 {
            if (*pkt).stream_index != video_idx {
                ffi::av_packet_unref(pkt);
                continue; // audio is re-encoded from the decode pass, not copied
            }
            let in_tb = (*vst).time_base;
            let raw = if (*pkt).dts != AV_NOPTS_VALUE {
                (*pkt).dts
            } else {
                (*pkt).pts
            };
            if raw == AV_NOPTS_VALUE {
                ffi::av_packet_unref(pkt);
                continue;
            }
            let g = ffi::av_rescale_q(raw, in_tb, TB_Q);
            let key = ((*pkt).flags & AV_PKT_FLAG_KEY) != 0;
            if !started {
                if !key || g < start_global {
                    ffi::av_packet_unref(pkt);
                    continue;
                }
                started = true;
                offset = g;
            }
            if g >= end_global {
                ffi::av_packet_unref(pkt);
                break;
            }
            let pts_g = if (*pkt).pts != AV_NOPTS_VALUE {
                ffi::av_rescale_q((*pkt).pts, in_tb, TB_Q) - offset
            } else {
                AV_NOPTS_VALUE
            };
            let dts_g = if (*pkt).dts != AV_NOPTS_VALUE {
                ffi::av_rescale_q((*pkt).dts, in_tb, TB_Q) - offset
            } else {
                AV_NOPTS_VALUE
            };
            let order = if dts_g != AV_NOPTS_VALUE { dts_g } else { pts_g };
            if order < 0 {
                ffi::av_packet_unref(pkt);
                continue;
            }
            let data = std::slice::from_raw_parts((*pkt).data, (*pkt).size as usize).to_vec();
            out.push(BufOp {
                order_us: order,
                is_video: true,
                pts: if pts_g != AV_NOPTS_VALUE { pts_g } else { order },
                dts: if dts_g != AV_NOPTS_VALUE { dts_g } else { order },
                src_tb: TB_Q,
                keyframe: key,
                data,
            });
            if first_v == AV_NOPTS_VALUE {
                first_v = order;
            }
            last_v = order;
            ffi::av_packet_unref(pkt);
        }
        Ok(())
    })();
    let mut p = pkt;
    ffi::av_packet_free(&mut p);
    res?;

    if !started {
        return Ok((0, AV_NOPTS_VALUE, AV_NOPTS_VALUE));
    }
    Ok((offset, first_v, last_v))
}

/// One stem decoder: AAC decoder context + a resampler to 48 kHz stereo f32 and
/// the accumulated interleaved PCM.
struct StemDecoder {
    stream_idx: i32,
    gain: f32,
    /// Run offline noise suppression on this stem's PCM before mixing (the mic).
    denoise: bool,
    ctx: *mut ffi::AVCodecContext,
    swr: *mut ffi::SwrContext,
    out: Vec<f32>,
}

/// Decode every `sel` stream from the start of `ic` to PCM, resample each to
/// 48 kHz stereo interleaved f32, and additively mix them (gain-scaled) into one
/// buffer (length = the longest stem). Seeks `ic` back to the start first.
unsafe fn decode_and_mix(
    ic: *mut ffi::AVFormatContext,
    sel: &[(i32, f32, bool)],
) -> Result<Vec<f32>, String> {
    let mut decoders: Vec<StemDecoder> = Vec::new();
    // Build a decoder + resampler per selected stream.
    let build = (|| -> Result<(), String> {
        for &(abs, gain, denoise) in sel {
            let st = *(*ic).streams.offset(abs as isize);
            let par = (*st).codecpar;
            let codec = ffi::avcodec_find_decoder((*par).codec_id);
            if codec.is_null() {
                return Err("audio decoder not found".into());
            }
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3(audio) failed".into());
            }
            if ffi::avcodec_parameters_to_context(ctx, par) < 0 {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err("avcodec_parameters_to_context failed".into());
            }
            let r = ffi::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err(format!("avcodec_open2(audio): {}", av_err(r)));
            }
            let swr = match build_decode_resampler(ctx) {
                Ok(s) => s,
                Err(e) => {
                    let mut c = ctx;
                    ffi::avcodec_free_context(&mut c);
                    return Err(e);
                }
            };
            decoders.push(StemDecoder {
                stream_idx: abs,
                gain,
                denoise,
                ctx,
                swr,
                out: Vec::new(),
            });
        }
        Ok(())
    })();
    if let Err(e) = build {
        free_decoders(&mut decoders);
        return Err(e);
    }

    // Seek to the start and decode the whole file once, routing each packet to
    // its stem decoder.
    ffi::av_seek_frame(ic, -1, 0, AVSEEK_FLAG_BACKWARD);
    for d in &decoders {
        ffi::avcodec_flush_buffers(d.ctx);
    }

    let pkt = ffi::av_packet_alloc();
    let frame = ffi::av_frame_alloc();
    let decode = (|| -> Result<(), String> {
        if pkt.is_null() || frame.is_null() {
            return Err("alloc packet/frame failed".into());
        }
        while ffi::av_read_frame(ic, pkt) >= 0 {
            let si = (*pkt).stream_index;
            if let Some(d) = decoders.iter_mut().find(|d| d.stream_idx == si) {
                decode_into(d, pkt, frame)?;
            }
            ffi::av_packet_unref(pkt);
        }
        // Flush each decoder (drain buffered frames).
        for d in decoders.iter_mut() {
            decode_into_flush(d, frame)?;
        }
        Ok(())
    })();
    if !pkt.is_null() {
        let mut p = pkt;
        ffi::av_packet_free(&mut p);
    }
    if !frame.is_null() {
        let mut f = frame;
        ffi::av_frame_free(&mut f);
    }
    if let Err(e) = decode {
        free_decoders(&mut decoders);
        return Err(e);
    }

    // Offline noise suppression on each flagged stem (the mic), on its decoded
    // 48 kHz stereo PCM — before mixing, so the cleaned signal is what gets
    // gain-scaled and re-encoded. Runs per stem; a failure is a no-op inside
    // `denoise` (keeps the original samples), so the export never loses audio.
    for d in decoders.iter_mut().filter(|d| d.denoise) {
        crate::core::denoise::denoise_interleaved_stereo_48k(&mut d.out);
    }

    // Mix (gain-scaled) into one buffer the length of the longest stem.
    let max_len = decoders.iter().map(|d| d.out.len()).max().unwrap_or(0);
    let mut mixed = vec![0f32; max_len];
    for d in &decoders {
        for (i, &s) in d.out.iter().enumerate() {
            mixed[i] += s * d.gain;
        }
    }
    free_decoders(&mut decoders);
    Ok(mixed)
}

/// Send `pkt` to `d`'s decoder and resample every produced frame into `d.out`.
unsafe fn decode_into(
    d: &mut StemDecoder,
    pkt: *mut ffi::AVPacket,
    frame: *mut ffi::AVFrame,
) -> Result<(), String> {
    let r = ffi::avcodec_send_packet(d.ctx, pkt);
    if r < 0 {
        return Ok(()); // tolerate a bad packet rather than abort the export
    }
    drain_frames(d, frame)
}

/// Flush `d`'s decoder (send a null packet) and drain its buffered frames.
unsafe fn decode_into_flush(d: &mut StemDecoder, frame: *mut ffi::AVFrame) -> Result<(), String> {
    let _ = ffi::avcodec_send_packet(d.ctx, ptr::null());
    drain_frames(d, frame)
}

unsafe fn drain_frames(d: &mut StemDecoder, frame: *mut ffi::AVFrame) -> Result<(), String> {
    loop {
        let r = ffi::avcodec_receive_frame(d.ctx, frame);
        if r < 0 {
            break; // EAGAIN / EOF / error → done draining
        }
        resample_into(d, frame);
        ffi::av_frame_unref(frame);
    }
    Ok(())
}

/// Resample one decoded frame to 48 kHz stereo interleaved f32, appended to
/// `d.out`.
unsafe fn resample_into(d: &mut StemDecoder, frame: *mut ffi::AVFrame) {
    let in_samples = (*frame).nb_samples;
    if in_samples <= 0 {
        return;
    }
    let in_rate = (*d.ctx).sample_rate.max(1);
    let max_out = (in_samples as i64 * MIX_RATE as i64 / in_rate as i64 + 1024) as i32;
    let base = d.out.len();
    d.out.resize(base + max_out as usize * 2, 0.0);
    let out_ptr = d.out.as_mut_ptr().add(base) as *mut u8;
    let out_planes: [*mut u8; 1] = [out_ptr];
    let in_planes = (*frame).extended_data as *const *const u8;
    let n = ffi::swr_convert(d.swr, out_planes.as_ptr(), max_out, in_planes, in_samples);
    if n < 0 {
        d.out.truncate(base);
        return;
    }
    d.out.truncate(base + n as usize * 2);
}

/// Build a resampler from a decoder context's format → 48 kHz stereo packed f32.
unsafe fn build_decode_resampler(ctx: *mut ffi::AVCodecContext) -> Result<*mut ffi::SwrContext, String> {
    let mut out_layout: ffi::AVChannelLayout = std::mem::zeroed();
    let mut in_layout: ffi::AVChannelLayout = std::mem::zeroed();
    ffi::av_channel_layout_default(&mut out_layout, MIX_CHANNELS);
    // Copy the decoder's channel layout; fall back to a default if it's unset.
    if ffi::av_channel_layout_copy(&mut in_layout, &(*ctx).ch_layout) < 0
        || (*ctx).ch_layout.nb_channels <= 0
    {
        ffi::av_channel_layout_default(&mut in_layout, (*ctx).ch_layout.nb_channels.max(1));
    }
    let mut swr: *mut ffi::SwrContext = ptr::null_mut();
    let r = ffi::swr_alloc_set_opts2(
        &mut swr,
        &out_layout,
        ffi::AV_SAMPLE_FMT_FLT,
        MIX_RATE,
        &in_layout,
        (*ctx).sample_fmt,
        (*ctx).sample_rate.max(1),
        0,
        ptr::null_mut(),
    );
    ffi::av_channel_layout_uninit(&mut out_layout);
    ffi::av_channel_layout_uninit(&mut in_layout);
    if r < 0 || swr.is_null() {
        return Err(format!("swr_alloc_set_opts2(decode): {}", av_err(r)));
    }
    let r = ffi::swr_init(swr);
    if r < 0 {
        ffi::swr_free(&mut swr);
        return Err(format!("swr_init(decode): {}", av_err(r)));
    }
    Ok(swr)
}

/// Free every decoder's codec context + resampler (idempotent on the pointers).
unsafe fn free_decoders(decoders: &mut Vec<StemDecoder>) {
    for d in decoders.iter_mut() {
        if !d.swr.is_null() {
            ffi::swr_free(&mut d.swr);
        }
        if !d.ctx.is_null() {
            ffi::avcodec_free_context(&mut d.ctx);
        }
    }
    decoders.clear();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::buffer::PacketRing;
    use crate::core::convert::Converter;
    use crate::core::device;
    use crate::core::encode::Encoder;
    use crate::core::mux::{write_clip, AudioClip, ClipMeta};
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    /// Build a real MP4 at `path` with one H.264 video stream + two named AAC
    /// audio tracks ("All Audio" master + "Microphone" stem, both ~1.5 s of
    /// silence). Requires a GPU encoder, like the `mux.rs`/`session.rs` tests.
    fn make_two_track_clip(path: &Path) {
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
        let (m_meta, m_pkts) = crate::core::audio::encode_silence_aac(1.5);
        let (s_meta, s_pkts) = crate::core::audio::encode_silence_aac(1.5);
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
        let _ = std::fs::remove_file(path);
        write_clip(path, &meta, &vpackets, &tracks).expect("write_clip");
    }

    /// Probe reports both audio tracks with their names; the master is index 0.
    #[test]
    fn probes_multi_track_names() {
        let src = std::env::temp_dir().join("hako_remux_probe_src.mp4");
        make_two_track_clip(&src);

        let tracks = probe_audio_tracks(&src).expect("probe");
        assert_eq!(tracks.len(), 2, "expected 2 audio tracks, got {tracks:?}");
        assert_eq!(tracks[0].index, 0);
        assert!(
            tracks.iter().any(|t| t.name == "All Audio"),
            "missing master name, got {tracks:?}"
        );
        assert!(
            tracks.iter().any(|t| t.name == "Microphone"),
            "missing stem name, got {tracks:?}"
        );
        let _ = std::fs::remove_file(&src);
    }

    /// Selecting a single stem at unity gain takes the loss-less stream-copy path
    /// and yields a video + exactly one audio track.
    #[test]
    fn remux_single_stem_stream_copies() {
        let src = std::env::temp_dir().join("hako_remux_copy_src.mp4");
        let out = std::env::temp_dir().join("hako_remux_copy_out.mp4");
        make_two_track_clip(&src);

        // Track index 1 = the "Microphone" stem.
        let sel = [TrackSel { index: 1, gain: 1.0, denoise: false }];
        let res = remux_with_tracks(&src, &out, 0.0, 1.4, &sel).expect("remux copy");
        assert!(res.width > 0 && res.height > 0, "video lost in export");
        assert!(res.duration_secs > 0.0);

        let tracks = probe_audio_tracks(&out).expect("probe out");
        assert_eq!(tracks.len(), 1, "expected one master track, got {tracks:?}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    /// Mixing two stems at non-unity gains takes the decode→mix→re-encode path
    /// and yields a video + a single freshly-encoded master track.
    #[test]
    fn remux_mix_reencodes_master() {
        let src = std::env::temp_dir().join("hako_remux_mix_src.mp4");
        let out = std::env::temp_dir().join("hako_remux_mix_out.mp4");
        make_two_track_clip(&src);

        let sel = [
            TrackSel { index: 0, gain: 0.5, denoise: false },
            TrackSel { index: 1, gain: 0.8, denoise: false },
        ];
        let res = remux_with_tracks(&src, &out, 0.0, 1.4, &sel).expect("remux mix");
        assert!(res.width > 0 && res.height > 0, "video lost in export");
        assert!(res.duration_secs > 0.0, "no duration");

        let tracks = probe_audio_tracks(&out).expect("probe out");
        assert_eq!(tracks.len(), 1, "expected one mixed master, got {tracks:?}");
        assert_eq!(tracks[0].name, "All Audio");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    /// A denoised single stem at unity gain skips the stream-copy fast path
    /// (its samples are rewritten) and re-encodes one master without panicking.
    #[test]
    fn remux_denoise_reencodes_master() {
        let src = std::env::temp_dir().join("hako_remux_dn_src.mp4");
        let out = std::env::temp_dir().join("hako_remux_dn_out.mp4");
        make_two_track_clip(&src);

        // Mic stem (index 1) at unity gain but with noise suppression on — must
        // decode → denoise → re-encode, not stream-copy.
        let sel = [TrackSel { index: 1, gain: 1.0, denoise: true }];
        let res = remux_with_tracks(&src, &out, 0.0, 1.4, &sel).expect("remux denoise");
        assert!(res.width > 0 && res.height > 0, "video lost in export");
        assert!(res.duration_secs > 0.0, "no duration");

        let tracks = probe_audio_tracks(&out).expect("probe out");
        assert_eq!(tracks.len(), 1, "expected one denoised master, got {tracks:?}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    /// No stems selected ⇒ a video-only export (zero audio tracks).
    #[test]
    fn remux_no_stems_is_video_only() {
        let src = std::env::temp_dir().join("hako_remux_none_src.mp4");
        let out = std::env::temp_dir().join("hako_remux_none_out.mp4");
        make_two_track_clip(&src);

        let res = remux_with_tracks(&src, &out, 0.0, 1.4, &[]).expect("remux none");
        assert!(res.width > 0 && res.height > 0);

        let tracks = probe_audio_tracks(&out).expect("probe out");
        assert!(tracks.is_empty(), "expected video-only, got {tracks:?}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }
}
