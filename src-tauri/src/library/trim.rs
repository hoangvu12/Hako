//! Loss-less clip trimming via FFmpeg **stream copy**.
//!
//! Cuts the half-open range `[start, end)` (seconds) out of an existing MP4 and
//! writes a new MP4 by copying packets — never re-encoding (the golden rule: the
//! CPU only touches compressed bytes). Optionally drops the audio track.
//!
//! Because we stream-copy, the start is **keyframe-aligned**: we seek to the
//! keyframe at or before `start`, then drop packets until the first video
//! keyframe. Hako clips carry a ~1s GOP, so the snap is ≤ ~1s — an acceptable
//! trade for instant, quality-preserving cuts. A single global timestamp offset
//! (the first kept video keyframe) is applied to every stream so A/V stays in
//! sync; audio preroll that would land before 0 is dropped.

#![allow(dead_code)]

use std::ffi::CString;
use std::path::Path;
use std::ptr;

use rusty_ffmpeg::ffi;

use crate::core::encode::av_err;

// ABI-stable raw flag values (avoids relying on the binding to export them).
const AVFMT_NOFILE: i32 = 0x0001;
const AVIO_FLAG_WRITE: i32 = 2;
const AV_PKT_FLAG_KEY: i32 = 1;
const AVSEEK_FLAG_BACKWARD: i32 = 1;
const AV_TIME_BASE: i64 = 1_000_000;
const AV_NOPTS_VALUE: i64 = i64::MIN;
const AVMEDIA_TYPE_VIDEO: i32 = 0;
const AVMEDIA_TYPE_AUDIO: i32 = 1;

const TB_Q: ffi::AVRational = ffi::AVRational {
    num: 1,
    den: AV_TIME_BASE as i32,
};

/// What the trim produced — used to refresh the library row / insert a new one.
#[derive(Debug, Clone)]
pub struct TrimResult {
    pub width: i64,
    pub height: i64,
    pub duration_secs: f64,
    /// How far the stream-copy start snapped *forward* past the requested start,
    /// in seconds (the cut begins at the first keyframe ≥ `start`). Callers
    /// rebase seek-bar markers by this so a marker measured from the requested
    /// start lines up with the clip that was actually written. 0 for whole-file /
    /// 0-second-start copies.
    pub start_shift_secs: f64,
}

/// Which audio streams a trim keeps in the output.
#[derive(Debug, Clone, Copy)]
pub enum AudioKeep {
    /// Every audio stream (multi-track clips keep all their stems).
    All,
    /// None — produce a video-only clip.
    None,
    /// Only the one audio stream at this **absolute** input stream index — the
    /// editor export's stream-copy path when a single stem is chosen at unity
    /// gain (it becomes the sole master track).
    Only(i32),
}

/// Trim `input` → `output` over `[start, end)` seconds via stream copy.
/// When `drop_audio` is set, the output is video-only.
pub fn trim_clip(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    drop_audio: bool,
) -> Result<TrimResult, String> {
    trim_inner(
        input,
        output,
        start,
        end,
        if drop_audio { AudioKeep::None } else { AudioKeep::All },
    )
}

/// Trim `input` → `output` over `[start, end)` keeping **only** the audio stream
/// at absolute index `audio_idx` (stream-copy). The editor's export uses this
/// when a single stem is selected at unity gain.
pub fn trim_keeping_audio(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    audio_idx: i32,
) -> Result<TrimResult, String> {
    trim_inner(input, output, start, end, AudioKeep::Only(audio_idx))
}

fn trim_inner(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    keep_audio: AudioKeep,
) -> Result<TrimResult, String> {
    if !(end > start) {
        return Err("trim end must be after start".into());
    }
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

            // --- locate streams -------------------------------------------------
            // Keep the first video stream plus EVERY audio stream (multi-track
            // clips carry one AAC stream per recorded track) unless dropping audio.
            let nb = (*ic).nb_streams as i32;
            let mut video_idx = -1i32;
            let (mut vw, mut vh) = (0i64, 0i64);
            for i in 0..nb {
                let st = *(*ic).streams.offset(i as isize);
                let par = (*st).codecpar;
                if (*par).codec_type == AVMEDIA_TYPE_VIDEO && video_idx < 0 {
                    video_idx = i;
                    vw = (*par).width as i64;
                    vh = (*par).height as i64;
                }
            }
            if video_idx < 0 {
                return Err("no video stream in clip".into());
            }

            // --- output context + mapped streams --------------------------------
            let r = ffi::avformat_alloc_output_context2(
                &mut ofmt,
                ptr::null_mut(),
                c_mp4.as_ptr(),
                c_out.as_ptr(),
            );
            if r < 0 || ofmt.is_null() {
                return Err(format!("alloc_output_context2: {}", av_err(r)));
            }

            // input stream index -> output stream index (-1 = drop).
            let meta_keys = ["title", "handler_name"];
            let mut mapping = vec![-1i32; nb as usize];
            let mut out_count = 0i32;
            for i in 0..nb {
                let in_st = *(*ic).streams.offset(i as isize);
                let in_par = (*in_st).codecpar;
                let keep_this_audio = (*in_par).codec_type == AVMEDIA_TYPE_AUDIO
                    && match keep_audio {
                        AudioKeep::All => true,
                        AudioKeep::None => false,
                        AudioKeep::Only(idx) => i == idx,
                    };
                let keep = i == video_idx || keep_this_audio;
                if !keep {
                    continue;
                }
                let out_st = ffi::avformat_new_stream(ofmt, ptr::null());
                if out_st.is_null() {
                    return Err("avformat_new_stream failed".into());
                }
                if ffi::avcodec_parameters_copy((*out_st).codecpar, in_par) < 0 {
                    return Err("avcodec_parameters_copy failed".into());
                }
                (*(*out_st).codecpar).codec_tag = 0; // let mp4 pick the tag
                (*out_st).time_base = (*in_st).time_base;
                // Carry the track name (multi-track stems are titled via both the
                // udta `title` and the `hdlr` handler_name) so the cut clip keeps
                // its labels — parameters_copy doesn't copy stream metadata.
                for key in meta_keys {
                    let k = CString::new(key).unwrap();
                    let entry = ffi::av_dict_get((*in_st).metadata, k.as_ptr(), ptr::null(), 0);
                    if !entry.is_null() {
                        ffi::av_dict_set(&mut (*out_st).metadata, k.as_ptr(), (*entry).value, 0);
                    }
                }
                mapping[i as usize] = out_count;
                out_count += 1;
            }

            // --- open file + faststart header -----------------------------------
            if ((*(*ofmt).oformat).flags & AVFMT_NOFILE) == 0 {
                let r = ffi::avio_open(&mut (*ofmt).pb, c_out.as_ptr(), AVIO_FLAG_WRITE);
                if r < 0 {
                    return Err(format!("avio_open: {}", av_err(r)));
                }
            }
            let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
            let k = CString::new("movflags").unwrap();
            let v = CString::new("faststart").unwrap();
            ffi::av_dict_set(&mut opts, k.as_ptr(), v.as_ptr(), 0);
            let r = ffi::avformat_write_header(ofmt, &mut opts);
            ffi::av_dict_free(&mut opts);
            if r < 0 {
                return Err(format!("write_header: {}", av_err(r)));
            }

            // --- seek to the keyframe at/before start ----------------------------
            let vst = *(*ic).streams.offset(video_idx as isize);
            let start_ts_v = ffi::av_rescale_q(
                (start * AV_TIME_BASE as f64) as i64,
                TB_Q,
                (*vst).time_base,
            );
            ffi::av_seek_frame(ic, video_idx, start_ts_v, AVSEEK_FLAG_BACKWARD);

            let start_global = (start * AV_TIME_BASE as f64) as i64;
            let end_global = (end * AV_TIME_BASE as f64) as i64;

            // --- copy loop ------------------------------------------------------
            let pkt = ffi::av_packet_alloc();
            if pkt.is_null() {
                return Err("av_packet_alloc failed".into());
            }

            let mut offset = AV_NOPTS_VALUE; // global ts of first kept video keyframe
            let mut started = false; // seen the first video keyframe yet
            let mut first_v = AV_NOPTS_VALUE;
            let mut last_v = AV_NOPTS_VALUE;

            let copy = (|| -> Result<(), String> {
                while ffi::av_read_frame(ic, pkt) >= 0 {
                    let in_idx = (*pkt).stream_index;
                    let out_idx = mapping
                        .get(in_idx as usize)
                        .copied()
                        .unwrap_or(-1);
                    if out_idx < 0 {
                        ffi::av_packet_unref(pkt);
                        continue;
                    }
                    let in_st = *(*ic).streams.offset(in_idx as isize);
                    let in_tb = (*in_st).time_base;
                    let is_video = in_idx == video_idx;

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

                    // Wait for the first video keyframe at/after start before we
                    // begin writing anything (defines the global offset).
                    if !started {
                        let key = ((*pkt).flags & AV_PKT_FLAG_KEY) != 0;
                        if !is_video || !key || g < start_global {
                            ffi::av_packet_unref(pkt);
                            continue;
                        }
                        started = true;
                        offset = g;
                    }

                    // Stop once we've passed the requested end.
                    if g >= end_global {
                        ffi::av_packet_unref(pkt);
                        break;
                    }

                    // Rebase both stamps by the single global offset, drop preroll.
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
                        ffi::av_packet_unref(pkt); // audio before the video start
                        continue;
                    }

                    let out_st = *(*ofmt).streams.offset(out_idx as isize);
                    let out_tb = (*out_st).time_base;
                    (*pkt).pts = if pts_g != AV_NOPTS_VALUE {
                        ffi::av_rescale_q(pts_g, TB_Q, out_tb)
                    } else {
                        AV_NOPTS_VALUE
                    };
                    (*pkt).dts = if dts_g != AV_NOPTS_VALUE {
                        ffi::av_rescale_q(dts_g, TB_Q, out_tb)
                    } else {
                        AV_NOPTS_VALUE
                    };
                    if (*pkt).pts != AV_NOPTS_VALUE
                        && (*pkt).dts != AV_NOPTS_VALUE
                        && (*pkt).pts < (*pkt).dts
                    {
                        (*pkt).pts = (*pkt).dts;
                    }
                    if (*pkt).duration > 0 {
                        (*pkt).duration = ffi::av_rescale_q((*pkt).duration, in_tb, out_tb);
                    }
                    (*pkt).stream_index = out_idx;
                    (*pkt).pos = -1;

                    if is_video {
                        if first_v == AV_NOPTS_VALUE {
                            first_v = order;
                        }
                        last_v = order;
                    }

                    let r = ffi::av_interleaved_write_frame(ofmt, pkt);
                    // write_frame takes ownership of the buffer & unrefs pkt.
                    if r < 0 {
                        return Err(format!("interleaved_write_frame: {}", av_err(r)));
                    }
                }
                Ok(())
            })();

            let mut p = pkt;
            ffi::av_packet_free(&mut p);
            copy?;

            if !started {
                return Err("no keyframe found in the selected range".into());
            }
            let r = ffi::av_write_trailer(ofmt);
            if r < 0 {
                return Err(format!("write_trailer: {}", av_err(r)));
            }

            // Duration from the kept video span (global µs → secs). Falls back to
            // the requested window if we somehow only wrote one packet.
            let duration_secs = if last_v > first_v && first_v != AV_NOPTS_VALUE {
                (last_v - first_v) as f64 / AV_TIME_BASE as f64
            } else {
                end - start
            };

            // The kept start (`offset`, global µs of the first written keyframe)
            // snaps forward to ≥ the requested start; report that gap so markers
            // can be rebased onto the clip we actually wrote.
            let start_shift_secs = if offset != AV_NOPTS_VALUE {
                ((offset - start_global).max(0)) as f64 / AV_TIME_BASE as f64
            } else {
                0.0
            };

            Ok(TrimResult {
                width: vw,
                height: vh,
                duration_secs,
                start_shift_secs,
            })
        })();

        // Teardown (mirror order in mux.rs).
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
