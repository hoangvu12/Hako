//! Encoder selection policy: which encoder name to ask FFmpeg for, which open
//! options to try, and what to fall back to when a codec is unavailable.
//!
//! Deliberately free of raw pointers and device handles -- this is the layer
//! that changes when a new codec or vendor is added, so it stays testable
//! without hardware. [`super::Encoder`] consumes these decisions.

use rusty_ffmpeg::ffi;

/// Hardware encode backend, chosen from the **encode** adapter's vendor (which
/// equals the capture adapter's on the single-device fast path, but differs on a
/// cross-adapter setup — e.g. capture on the Intel iGPU, encode on the NVIDIA
/// dGPU; see `device::resolve_adapters`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Backend {
    /// `h264_nvenc` — NVIDIA. Consumes our `AV_PIX_FMT_D3D11` NV12 texture
    /// directly (no QSV derive, no per-frame map); zero-copy on the NVIDIA device.
    Nvenc,
    /// `h264_qsv` — Intel. Needs the D3D11 frames mapped to an `AV_PIX_FMT_QSV`
    /// frame (still zero-copy, just an extra mapping step).
    Qsv,
}

impl Backend {
    pub(super) fn encoder_name(self, codec: VideoCodec) -> &'static str {
        match (self, codec) {
            (Backend::Nvenc, VideoCodec::H264) => "h264_nvenc",
            (Backend::Nvenc, VideoCodec::Hevc) => "hevc_nvenc",
            (Backend::Nvenc, VideoCodec::Av1) => "av1_nvenc",
            (Backend::Qsv, VideoCodec::H264) => "h264_qsv",
            (Backend::Qsv, VideoCodec::Hevc) => "hevc_qsv",
            (Backend::Qsv, VideoCodec::Av1) => "av1_qsv",
        }
    }

    /// Encoder option sets to try with `avcodec_open2`, **lowest GPU load / latency
    /// first**, falling back to progressively more conservative sets. Option
    /// availability is build- and GPU-specific (QSV `low_power`/VDEnc needs Gen9+
    /// Intel; some NVENC options vary by driver), and a rejected option makes
    /// `avcodec_open2` fail — so we re-try with fewer options rather than refusing
    /// to start. The last set reproduces the historical default, so this can only
    /// improve on or match what opened before.
    ///
    /// Note: `tune=ull` does NOT itself disable lookahead at the FFmpeg layer, so
    /// `rc-lookahead=0` is set explicitly (B-frames are already off via
    /// `max_b_frames = 0`). QSV has no `rc` option — CBR/VBR is implied by the
    /// bitrate fields on the codec context, so only `low_power`/`preset`/
    /// `async_depth` are passed here.
    pub(super) fn open_option_sets(self) -> Vec<Vec<(&'static str, &'static str)>> {
        match self {
            Backend::Nvenc => vec![
                vec![
                    ("preset", "p1"),          // fastest NVENC preset → least load
                    ("tune", "ull"),           // ultra-low-latency tuning
                    ("rc-lookahead", "0"),     // no lookahead (latency + GPU)
                    ("multipass", "disabled"), // single pass
                ],
                vec![("preset", "p4")], // older driver: just a balanced preset
                vec![],                 // bare (historical default)
            ],
            Backend::Qsv => vec![
                vec![
                    ("low_power", "1"),     // VDEnc fixed-function path (Gen9+)
                    ("preset", "veryfast"), // fastest target-usage
                    ("async_depth", "1"),   // shallow async → immediate backpressure
                ],
                vec![("preset", "veryfast"), ("async_depth", "1")], // no low_power
                vec![("async_depth", "1")],                         // bare (historical default)
            ],
        }
    }
}

/// Output video codec, selected from `Settings::codec`. The hardware encoder
/// actually used is this codec × the adapter vendor (e.g. Hevc × NVIDIA =
/// `hevc_nvenc`), with graceful fallback toward H.264 when a codec's encoder
/// isn't available on the GPU/driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

/// Codec + bitrate + output-resolution selection, threaded from settings down to
/// the encode thread.
#[derive(Debug, Clone, Copy)]
pub struct EncodeSettings {
    pub codec: VideoCodec,
    pub bitrate_mbps: u32,
    /// Output-resolution target box (width, height), or `None` for native (no
    /// scaling). The encode thread fits the captured frame into this box by
    /// height and never upscales (see [`crate::settings::Settings::resolution_dims`]).
    pub target_res: Option<(u32, u32)>,
    /// Composite the "tabbed out" freeze card onto frozen frames before encoding
    /// (minimized / alt-tabbed / stale swapchain), so a clip viewer sees an
    /// intentional notice instead of a silently-held frame. See
    /// [`crate::core::overlay_card`].
    pub freeze_overlay: bool,
    /// Composite the mouse cursor onto captured frames before encoding (the
    /// hardware cursor isn't in the shared backbuffer). See
    /// [`crate::core::cursor_overlay`]. A per-frame flag, toggled live.
    pub record_cursor: bool,
}

impl VideoCodec {
    /// Parse the `Settings::codec` string ("h264" | "hevc" | "av1"); unknown ⇒ H264.
    pub fn from_setting(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "hevc" | "h265" => VideoCodec::Hevc,
            "av1" => VideoCodec::Av1,
            _ => VideoCodec::H264,
        }
    }

    /// FFmpeg `AV_CODEC_ID_*` for muxing a stream of this codec — used by the
    /// stream-copy writers (`mux.rs`, `session.rs`) to declare the output stream.
    pub fn av_codec_id(self) -> u32 {
        match self {
            VideoCodec::H264 => ffi::AV_CODEC_ID_H264,
            VideoCodec::Hevc => ffi::AV_CODEC_ID_HEVC,
            VideoCodec::Av1 => ffi::AV_CODEC_ID_AV1,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// This codec plus the fallbacks to try if its encoder is unavailable, most
    /// preferred first, always ending at H.264 (universally supported in HW).
    pub(super) fn fallback_chain(self) -> &'static [VideoCodec] {
        match self {
            VideoCodec::Av1 => &[VideoCodec::Av1, VideoCodec::Hevc, VideoCodec::H264],
            VideoCodec::Hevc => &[VideoCodec::Hevc, VideoCodec::H264],
            VideoCodec::H264 => &[VideoCodec::H264],
        }
    }

    /// The full ordered list of codecs [`Encoder::new`] will attempt: the
    /// preference chain ([`Self::fallback_chain`], toward H.264 for
    /// *availability*) first, then — as an **open-failure** recovery — any
    /// remaining hardware codecs on this vendor.
    ///
    /// `fallback_chain` alone only degrades *toward* H.264, so a requested-H.264
    /// encoder that is present but **won't open** (e.g. `h264_nvenc` refusing a
    /// 10-bit / HDR backbuffer, or a wedged NVENC session after a mid-match
    /// swapchain recreation) previously failed the entire capture — the game was
    /// "detected" but never clipped. Appending the other codecs lets HEVC/AV1
    /// NVENC (which *do* accept those states) take over instead. HEVC is tried
    /// before AV1 because HEVC hardware encode is far more widely available
    /// (NVENC HEVC since Turing; AV1 NVENC needs Ada / RTX 40-series). The
    /// codec that actually opened is reported by [`Self::codec`] and drives the
    /// muxer's `codec_id`, so a fallback clip is still written correctly.
    pub(super) fn candidate_chain(self) -> Vec<VideoCodec> {
        let mut out: Vec<VideoCodec> = self.fallback_chain().to_vec();
        for c in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The open-failure recovery chain: whatever the user requested, `new` must
    /// eventually attempt every hardware codec, so a requested codec that is
    /// present but won't *open* on this GPU/driver/state (e.g. `h264_nvenc`
    /// refusing an HDR/10-bit backbuffer) still finds a working encoder instead
    /// of failing the whole capture. Requested codec first; HEVC before AV1 in
    /// recovery (HEVC HW encode is far more widely available).
    #[test]
    fn candidate_chain_covers_all_codecs_requested_first() {
        for req in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
            let chain = req.candidate_chain();
            assert_eq!(chain[0], req, "requested codec must be tried first");
            for c in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1] {
                assert!(chain.contains(&c), "{req:?} chain missing {c:?}: {chain:?}");
            }
            // No duplicate attempts.
            let mut seen = chain.clone();
            seen.dedup();
            assert_eq!(
                seen.len(),
                chain.len(),
                "duplicate in {req:?} chain: {chain:?}"
            );
            assert_eq!(seen.len(), 3, "chain should be exactly the 3 HW codecs");
            // In the recovery tail, HEVC precedes AV1 (HEVC HW encode is far more
            // widely available) — except when AV1 was explicitly requested, which
            // is honored first by preference.
            if req != VideoCodec::Av1 {
                let hevc = chain.iter().position(|c| *c == VideoCodec::Hevc).unwrap();
                let av1 = chain.iter().position(|c| *c == VideoCodec::Av1).unwrap();
                assert!(hevc < av1, "HEVC should be tried before AV1: {chain:?}");
            }
        }
    }
}
