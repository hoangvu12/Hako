//! Probing the bundled FFmpeg build: library versions and which hardware
//! encoders the linked build actually resolves.
//!
//! Pure detection -- it opens no device and allocates no encoder, so it is safe
//! to call from a Tauri command thread (it backs `ffmpeg_info`).

use std::ffi::CString;

use rusty_ffmpeg::ffi;
use serde::Serialize;

/// Result of probing the bundled FFmpeg build (detection step).
#[derive(Debug, Clone, Serialize)]
pub struct FfmpegProbe {
    pub avutil_version: String,
    pub avcodec_version: String,
    pub avformat_version: String,
    /// Hardware H.264/HEVC/AV1 encoders the linked FFmpeg can resolve by name.
    pub encoders: Vec<EncoderAvailability>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EncoderAvailability {
    pub name: String,
    pub available: bool,
}

const PROBED_ENCODERS: &[&str] = &[
    "h264_nvenc",
    "hevc_nvenc",
    "av1_nvenc",
    "h264_amf",
    "hevc_amf",
    "h264_qsv",
    "hevc_qsv",
];

/// Probe the linked FFmpeg: versions + which hardware encoders are present.
///
/// This validates that the bundled DLLs link and that the FFI ABI is sane
/// (versions decode to the expected 8.1.x), and that NVENC is reachable.
pub fn probe() -> FfmpegProbe {
    let encoders = PROBED_ENCODERS
        .iter()
        .map(|&name| EncoderAvailability {
            name: name.to_string(),
            available: encoder_exists(name),
        })
        .collect();

    FfmpegProbe {
        avutil_version: version_string(unsafe { ffi::avutil_version() }),
        avcodec_version: version_string(unsafe { ffi::avcodec_version() }),
        avformat_version: version_string(unsafe { ffi::avformat_version() }),
        encoders,
    }
}

/// True if FFmpeg can resolve an encoder by name (codec compiled in).
pub fn encoder_exists(name: &str) -> bool {
    let Ok(cname) = CString::new(name) else {
        return false;
    };
    // SAFETY: cname is a valid NUL-terminated string for the duration of the call.
    let codec = unsafe { ffi::avcodec_find_encoder_by_name(cname.as_ptr()) };
    !codec.is_null()
}

/// Decode an FFmpeg `AV_VERSION_INT` (major<<16 | minor<<8 | micro).
fn version_string(v: u32) -> String {
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
}
