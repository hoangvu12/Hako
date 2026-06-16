//! Thumbnail extraction from clips via FFmpeg.
//!
//! Decodes the first frame of a clip, scales it down (preserving aspect) with
//! swscale, and JPEG-encodes it with the `mjpeg` encoder. Pure CPU on a single
//! short-lived frame — cheap, runs off the hot path after a clip is saved.

#![allow(dead_code)]

use std::path::Path;
use std::ptr;

use rusty_ffmpeg::ffi;

use crate::core::encode::av_err;

const SWS_BILINEAR: i32 = 2;
const AVMEDIA_TYPE_VIDEO: i32 = 0;

/// Extract a thumbnail from `video` into `out_jpg`, scaled so its width is at
/// most `max_width` (height follows the source aspect ratio, both even).
pub fn extract_thumbnail(video: &Path, out_jpg: &Path, max_width: u32) -> Result<(), String> {
    let c_in = path_cstr(video)?;
    unsafe {
        // --- demux + decode one frame ---------------------------------------
        let mut ic: *mut ffi::AVFormatContext = ptr::null_mut();
        if ffi::avformat_open_input(&mut ic, c_in.as_ptr(), ptr::null_mut(), ptr::null_mut()) < 0 {
            return Err("avformat_open_input failed".into());
        }
        // From here on, jump to cleanup via the inner closure's Result.
        let result = (|| -> Result<(), String> {
            if ffi::avformat_find_stream_info(ic, ptr::null_mut()) < 0 {
                return Err("find_stream_info failed".into());
            }
            let nb = (*ic).nb_streams as isize;
            let mut vidx = -1i32;
            for i in 0..nb {
                let st = *(*ic).streams.offset(i);
                if (*(*st).codecpar).codec_type == AVMEDIA_TYPE_VIDEO {
                    vidx = i as i32;
                    break;
                }
            }
            if vidx < 0 {
                return Err("no video stream".into());
            }
            let st = *(*ic).streams.offset(vidx as isize);
            let codecpar = (*st).codecpar;
            let dec = ffi::avcodec_find_decoder((*codecpar).codec_id);
            if dec.is_null() {
                return Err("no decoder for clip".into());
            }
            let dctx = ffi::avcodec_alloc_context3(dec);
            if dctx.is_null() {
                return Err("alloc decoder ctx failed".into());
            }
            let dec_result = decode_first_frame(ic, dctx, dec, codecpar, vidx, out_jpg, max_width);
            let mut dctx_mut = dctx;
            ffi::avcodec_free_context(&mut dctx_mut);
            dec_result
        })();

        ffi::avformat_close_input(&mut ic);
        result
    }
}

/// Decode the first frame off `vidx`, scale to JPEG, write `out_jpg`.
unsafe fn decode_first_frame(
    ic: *mut ffi::AVFormatContext,
    dctx: *mut ffi::AVCodecContext,
    dec: *const ffi::AVCodec,
    codecpar: *mut ffi::AVCodecParameters,
    vidx: i32,
    out_jpg: &Path,
    max_width: u32,
) -> Result<(), String> {
    if ffi::avcodec_parameters_to_context(dctx, codecpar) < 0 {
        return Err("parameters_to_context failed".into());
    }
    if ffi::avcodec_open2(dctx, dec, ptr::null_mut()) < 0 {
        return Err("open decoder failed".into());
    }

    let pkt = ffi::av_packet_alloc();
    let frame = ffi::av_frame_alloc();
    if pkt.is_null() || frame.is_null() {
        return Err("alloc pkt/frame failed".into());
    }

    let mut got = false;
    while ffi::av_read_frame(ic, pkt) >= 0 {
        if (*pkt).stream_index == vidx {
            if ffi::avcodec_send_packet(dctx, pkt) >= 0
                && ffi::avcodec_receive_frame(dctx, frame) >= 0
            {
                got = true;
                ffi::av_packet_unref(pkt);
                break;
            }
        }
        ffi::av_packet_unref(pkt);
    }

    let res = if got {
        write_jpeg(frame, out_jpg, max_width)
    } else {
        Err("no decodable frame".into())
    };

    let mut p = pkt;
    let mut f = frame;
    ffi::av_packet_free(&mut p);
    ffi::av_frame_free(&mut f);
    res
}

/// Scale `frame` to ≤`max_width` and JPEG-encode it into `out_jpg`.
unsafe fn write_jpeg(
    frame: *mut ffi::AVFrame,
    out_jpg: &Path,
    max_width: u32,
) -> Result<(), String> {
    let src_w = (*frame).width;
    let src_h = (*frame).height;
    if src_w <= 0 || src_h <= 0 {
        return Err("decoded frame has no size".into());
    }
    let scale = (max_width as f64 / src_w as f64).min(1.0);
    let dst_w = (((src_w as f64 * scale) as i32) & !1).max(2);
    let dst_h = (((src_h as f64 * scale) as i32) & !1).max(2);

    // swscale: source pix_fmt → YUVJ420P (the mjpeg encoder's full-range 4:2:0).
    let yuvj420p = ffi::AV_PIX_FMT_YUVJ420P;
    let sws = ffi::sws_getContext(
        src_w,
        src_h,
        (*frame).format,
        dst_w,
        dst_h,
        yuvj420p,
        SWS_BILINEAR,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null(),
    );
    if sws.is_null() {
        return Err("sws_getContext failed".into());
    }

    let dst = ffi::av_frame_alloc();
    if dst.is_null() {
        ffi::sws_freeContext(sws);
        return Err("alloc dst frame failed".into());
    }
    (*dst).format = yuvj420p;
    (*dst).width = dst_w;
    (*dst).height = dst_h;

    let run = (|| -> Result<(), String> {
        if ffi::av_frame_get_buffer(dst, 32) < 0 {
            return Err("dst frame get_buffer failed".into());
        }
        ffi::sws_scale(
            sws,
            (*frame).data.as_ptr() as *const *const u8,
            (*frame).linesize.as_ptr(),
            0,
            src_h,
            (*dst).data.as_ptr() as *const *mut u8,
            (*dst).linesize.as_ptr(),
        );
        (*dst).pts = 0;
        encode_mjpeg(dst, dst_w, dst_h, out_jpg)
    })();

    ffi::sws_freeContext(sws);
    let mut d = dst;
    ffi::av_frame_free(&mut d);
    run
}

/// MJPEG-encode `frame` and write the single JPEG packet to `out_jpg`.
unsafe fn encode_mjpeg(
    frame: *mut ffi::AVFrame,
    w: i32,
    h: i32,
    out_jpg: &Path,
) -> Result<(), String> {
    let enc = ffi::avcodec_find_encoder(ffi::AV_CODEC_ID_MJPEG);
    if enc.is_null() {
        return Err("mjpeg encoder not found".into());
    }
    let ectx = ffi::avcodec_alloc_context3(enc);
    if ectx.is_null() {
        return Err("alloc mjpeg ctx failed".into());
    }
    (*ectx).width = w;
    (*ectx).height = h;
    (*ectx).pix_fmt = ffi::AV_PIX_FMT_YUVJ420P;
    (*ectx).time_base = ffi::AVRational { num: 1, den: 25 };

    let run = (|| -> Result<(), String> {
        if ffi::avcodec_open2(ectx, enc, ptr::null_mut()) < 0 {
            return Err("open mjpeg encoder failed".into());
        }
        if ffi::avcodec_send_frame(ectx, frame) < 0 {
            return Err("mjpeg send_frame failed".into());
        }
        ffi::avcodec_send_frame(ectx, ptr::null()); // flush
        let pkt = ffi::av_packet_alloc();
        if pkt.is_null() {
            return Err("alloc jpeg pkt failed".into());
        }
        let r = ffi::avcodec_receive_packet(ectx, pkt);
        let res = if r >= 0 {
            let bytes = std::slice::from_raw_parts((*pkt).data, (*pkt).size as usize);
            std::fs::write(out_jpg, bytes).map_err(|e| format!("write jpg: {e}"))
        } else {
            Err(format!("mjpeg receive_packet: {}", av_err(r)))
        };
        let mut p = pkt;
        ffi::av_packet_free(&mut p);
        res
    })();

    let mut e = ectx;
    ffi::avcodec_free_context(&mut e);
    run
}

fn path_cstr(p: &Path) -> Result<std::ffi::CString, String> {
    std::ffi::CString::new(p.to_str().ok_or("path not UTF-8")?).map_err(|_| "path has NUL".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{buffer::PacketRing, convert::Converter, device, encode::Encoder, mux};
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};

    /// Build a real short MP4 (encode synthetic frames), then extract a thumbnail
    /// and assert it's a valid JPEG. End-to-end over the FFmpeg decode→scale→jpeg.
    #[test]
    fn extracts_a_jpeg_thumbnail() {
        let gpus = device::enumerate_gpus().expect("gpus");
        let adapter =
            device::default_capture_index(&gpus).map(|i| device::adapter_at(i).expect("adapter"));
        let (dev, ctx, _fl) = device::create_device(adapter.as_ref()).expect("device");
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
        unsafe { dev.CreateTexture2D(&desc, None, Some(&mut bgra)).expect("bgra") };
        let bgra = bgra.unwrap();
        let conv = Converter::new(&dev, &ctx, w, h).expect("conv");
        let mut enc = Encoder::new_qsv(&dev, &ctx, w, h, fps).expect("enc");
        let mut ring = PacketRing::new(fps, 30);
        for i in 0..30i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12");
            conv.convert(&bgra, &nv12).expect("convert");
            for p in enc.encode(&nv12, i).expect("encode") {
                ring.push(p);
            }
        }
        for p in enc.flush().expect("flush") {
            ring.push(p);
        }
        let meta = mux::ClipMeta {
            width: w,
            height: h,
            fps,
            extradata: enc.extradata(),
        };
        let mp4 = std::env::temp_dir().join("hako_thumb_src.mp4");
        let _ = std::fs::remove_file(&mp4);
        mux::write_clip(&mp4, &meta, &ring.slice_last(1), None).expect("mux");

        let jpg = std::env::temp_dir().join("hako_thumb.jpg");
        let _ = std::fs::remove_file(&jpg);
        extract_thumbnail(&mp4, &jpg, 320).expect("thumbnail");

        let bytes = std::fs::read(&jpg).expect("jpg exists");
        println!("thumbnail {} bytes", bytes.len());
        assert!(bytes.len() > 100, "thumbnail too small");
        assert_eq!(&bytes[0..2], &[0xFF, 0xD8], "not a JPEG (no SOI marker)");

        let _ = std::fs::remove_file(&mp4);
        let _ = std::fs::remove_file(&jpg);
    }
}
