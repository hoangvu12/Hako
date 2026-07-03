//! FFmpeg hardware encode. One AVHWDeviceContext (D3D11VA) wrapping our
//! shared device; AVHWFramesContext (D3D11/NV12) derived to QSV. NVENC/AMF/QSV
//! selection with graceful fallback (HEVC→H.264) — never silent x264.
//!
//! The encoder runs on the **same (display-owning) adapter the frame was
//! captured/converted on**, so the path is zero-copy with no cross-adapter copy.
//! The HW backend is chosen from that adapter's vendor ([`Encoder::new`]):
//! - **NVIDIA → `h264_nvenc`**: the NV12 `AV_PIX_FMT_D3D11` frame is sent
//!   directly to the encoder (NVENC consumes D3D11 surfaces natively).
//! - **Intel → `h264_qsv`**: the D3D11 frame is mapped (no copy)
//!   to an `AV_PIX_FMT_QSV` frame via `av_hwframe_map` first — QSV can only
//!   derive from an Intel device, which is why a hardcoded `h264_qsv` failed
//!   when capturing on an NVIDIA display adapter.
//!
//! Either way the CPU only ever sees the compressed `AVPacket` (golden rule).
//! AMD (AMF) is the remaining vendor; we never fall back to software x264.
//!
//! `probe()` (the original link/availability check) is kept for the dashboard.

#![allow(dead_code)]

use std::ffi::{c_void, CString};
use std::ptr;

use rusty_ffmpeg::ffi;
use serde::Serialize;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Multithread, ID3D11Resource, ID3D11Texture2D,
};

use crate::core::device::Vendor;

// ---------------------------------------------------------------------------
// FFmpeg D3D11VA hardware-context structs.
//
// These live in libavutil/hwcontext_d3d11va.h, which the prebuilt binding
// (src-tauri/ffmpeg/binding.rs) does NOT include — so we declare them here,
// matching the FFmpeg 8.1 ABI. `#[repr(C)]` + exact field order/types is what
// makes this safe; the layout has been stable for years.
// ---------------------------------------------------------------------------

/// `AVHWDeviceContext.hwctx` when the type is `AV_HWDEVICE_TYPE_D3D11VA`.
/// libavutil takes ownership of the COM references stored here and Releases
/// them when the device context is freed.
#[repr(C)]
struct AVD3D11VADeviceContext {
    device: *mut c_void,         // ID3D11Device*
    device_context: *mut c_void, // ID3D11DeviceContext*
    video_device: *mut c_void,   // ID3D11VideoDevice*   (NULL → FFmpeg derives)
    video_context: *mut c_void,  // ID3D11VideoContext*  (NULL → FFmpeg derives)
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

/// `AVHWFramesContext.hwctx` when the format is `AV_PIX_FMT_D3D11`.
/// `BindFlags`/`MiscFlags` are applied to textures FFmpeg allocates for its
/// pool; we set `BindFlags` so the layout matches our externally-provided NV12
/// textures (this Intel driver rejects `RENDER_TARGET | DECODER` combined).
#[repr(C)]
struct AVD3D11VAFramesContext {
    texture: *mut c_void, // ID3D11Texture2D* (user-supplied array texture, opt)
    bind_flags: u32,      // UINT BindFlags
    misc_flags: u32,      // UINT MiscFlags
}

// ---------------------------------------------------------------------------
// Probe (unchanged): versions + which HW encoders the linked FFmpeg resolves.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// AVERROR helpers (the binding has no AVERROR macro / EOF constant).
// On every platform FFmpeg supports, AVERROR(e) == -e for POSIX errnos.
// ---------------------------------------------------------------------------

/// `AVERROR(EAGAIN)` — encoder needs more input / has no packet yet.
const AVERROR_EAGAIN: i32 = -(ffi::EAGAIN as i32);
/// `AVERROR_EOF` = `-FFERRTAG('E','O','F',' ')` (FFmpeg's end-of-stream code).
const AVERROR_EOF: i32 =
    -((b'E' as i32) | ((b'O' as i32) << 8) | ((b'F' as i32) << 16) | ((b' ' as i32) << 24));

/// Format an FFmpeg negative error code as a human-readable string.
pub(crate) fn av_err(code: i32) -> String {
    let mut buf = [0i8; 256];
    // SAFETY: buf is a valid writable buffer of the given length.
    unsafe {
        ffi::av_strerror(code, buf.as_mut_ptr(), buf.len());
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

// D3D11 bind flag constants we need (avoid pulling the whole windows enum into
// the FFI struct; these are the raw UINT values).
const D3D11_BIND_RENDER_TARGET: u32 = 0x20;
const D3D11_BIND_DECODER: u32 = 0x200;

/// Size of FFmpeg's D3D11 texture-array pool for the NVENC input path. NVENC
/// holds a few input surfaces in flight; the array must cover that.
const NVENC_POOL_SIZE: u32 = 8;

/// `AV_CODEC_FLAG_GLOBAL_HEADER` (1 << 22). Tells the encoder to emit SPS/PPS in
/// `extradata` (as avcC) instead of repeating them in-band — the canonical setup
/// for muxing the packets into MP4 by stream-copy (`mux.rs`). ABI-stable value.
const AV_CODEC_FLAG_GLOBAL_HEADER: i32 = 1 << 22;

// ---------------------------------------------------------------------------
// A compressed packet handed back to the caller (buffer.rs will keep AVPackets
// directly later; for now we copy the bytes out so the boundary is simple).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub keyframe: bool,
}

// ---------------------------------------------------------------------------
// Encoder: owns the FFmpeg hw device/frames contexts + codec context.
// ---------------------------------------------------------------------------

/// Zero-copy hardware H.264 encoder fed by D3D11 NV12 textures.
///
/// Built around our shared D3D11 device so capture → convert → encode never
/// leaves the GPU. Not `Send`-safe casually: it (like the whole encode thread)
/// owns the non-free-threaded `ID3D11DeviceContext` and the FFmpeg encoder;
/// keep all calls on one thread.
pub struct Encoder {
    /// Which HW backend this encoder drives — decides D3D11-direct (NVENC) vs
    /// mapped-to-QSV input, and which derived contexts exist.
    backend: Backend,
    /// The codec this encoder opened with (after any fallback) — drives the
    /// encoder name, the muxer stream `codec_id`, and logging.
    codec: VideoCodec,
    /// Target bit rate in bits/sec (from `Settings::bitrate_mbps`).
    bitrate_bps: i64,
    /// Our shared D3D11 immediate context. Kept for the NVENC path, which
    /// GPU-copies the converted NV12 into FFmpeg's pool texture-array slice
    /// (`CopySubresourceRegion`) before submitting the frame.
    context: ID3D11DeviceContext,
    codec_ctx: *mut ffi::AVCodecContext,
    /// D3D11 frames context (`AV_PIX_FMT_D3D11`, sw NV12) — describes the
    /// textures we wrap. Owned ref; freed on drop. NVENC's input directly.
    d3d11_frames: *mut ffi::AVBufferRef,
    /// QSV frames context derived from `d3d11_frames`. The QSV encoder's input;
    /// **null for NVENC** (which consumes D3D11 frames directly).
    qsv_frames: *mut ffi::AVBufferRef,
    /// D3D11VA hw device context. Owns our (AddRef'd) ID3D11Device/Context.
    d3d11_device_ctx: *mut ffi::AVBufferRef,
    /// QSV hw device context derived from `d3d11_device_ctx`; **null for NVENC**.
    qsv_device_ctx: *mut ffi::AVBufferRef,
    packet: *mut ffi::AVPacket,
    width: u32,
    height: u32,
    fps: u32,
}

/// Hardware encode backend, chosen from the **encode** adapter's vendor (which
/// equals the capture adapter's on the single-device fast path, but differs on a
/// cross-adapter setup — e.g. capture on the Intel iGPU, encode on the NVIDIA
/// dGPU; see `device::resolve_adapters`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// `h264_nvenc` — NVIDIA. Consumes our `AV_PIX_FMT_D3D11` NV12 texture
    /// directly (no QSV derive, no per-frame map); zero-copy on the NVIDIA device.
    Nvenc,
    /// `h264_qsv` — Intel. Needs the D3D11 frames mapped to an `AV_PIX_FMT_QSV`
    /// frame (still zero-copy, just an extra mapping step).
    Qsv,
}

impl Backend {
    fn encoder_name(self, codec: VideoCodec) -> &'static str {
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
    fn open_option_sets(self) -> Vec<Vec<(&'static str, &'static str)>> {
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

    fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
        }
    }

    /// This codec plus the fallbacks to try if its encoder is unavailable, most
    /// preferred first, always ending at H.264 (universally supported in HW).
    fn fallback_chain(self) -> &'static [VideoCodec] {
        match self {
            VideoCodec::Av1 => &[VideoCodec::Av1, VideoCodec::Hevc, VideoCodec::H264],
            VideoCodec::Hevc => &[VideoCodec::Hevc, VideoCodec::H264],
            VideoCodec::H264 => &[VideoCodec::H264],
        }
    }
}

impl Encoder {
    /// Build the hardware encoder matching the **encode adapter's vendor** ×
    /// requested `codec`: NVIDIA → `*_nvenc`, Intel → `*_qsv`. `device`/`context`
    /// are the **encode** device — the same device the frame was captured/converted
    /// on for the single-device fast path (no cross-adapter copy), or the separate
    /// encode-GPU device on a cross-adapter setup (see `device::resolve_adapters`).
    /// The vendor MUST be the encode adapter's, not the capture adapter's — that's
    /// what selects NVENC vs QSV correctly when they differ.
    ///
    /// `codec` falls back toward H.264 if its encoder isn't present in this FFmpeg
    /// build or won't open on this GPU (e.g. AV1 NVENC needs RTX 40-series); the
    /// codec that actually opened is reported by [`Self::codec`]. `bitrate_mbps`
    /// sets the target bit rate.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        vendor: Vendor,
        codec: VideoCodec,
        bitrate_mbps: u32,
        width: u32,
        height: u32,
        fps: u32,
    ) -> std::result::Result<Self, String> {
        let backend = match vendor {
            Vendor::Nvidia => Backend::Nvenc,
            Vendor::Intel => Backend::Qsv,
            // AMD (AMF) is the remaining vendor; Other has no HW path.
            // Never silently fall back to software encoding.
            Vendor::Amd => return Err("AMD (AMF) hardware encode is not implemented yet".into()),
            Vendor::Other => return Err("no hardware encoder for this adapter's vendor".into()),
        };

        // Try the requested codec, then graceful fallbacks toward H.264. Skip any
        // whose encoder isn't compiled into this FFmpeg build before attempting.
        let mut last_err = String::new();
        for &cand in codec.fallback_chain() {
            let name = backend.encoder_name(cand);
            if !encoder_exists(name) {
                last_err = format!("{name} not available in this build");
                continue;
            }
            match Self::build(
                device,
                context,
                backend,
                cand,
                bitrate_mbps,
                width,
                height,
                fps,
            ) {
                Ok(enc) => {
                    if cand != codec {
                        tracing::warn!(
                            "{} encode unavailable; fell back to {}",
                            codec.label(),
                            cand.label()
                        );
                    }
                    return Ok(enc);
                }
                Err(e) => last_err = e,
            }
        }
        Err(format!(
            "no usable {} hardware encoder (last error: {last_err})",
            codec.label()
        ))
    }

    /// Build a QSV H.264 encoder explicitly (Intel input). Used by tests.
    pub fn new_qsv(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
        fps: u32,
    ) -> std::result::Result<Self, String> {
        Self::build(
            device,
            context,
            Backend::Qsv,
            VideoCodec::H264,
            20,
            width,
            height,
            fps,
        )
    }

    /// Build an NVENC H.264 encoder explicitly (NVIDIA input).
    pub fn new_nvenc(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
        fps: u32,
    ) -> std::result::Result<Self, String> {
        Self::build(
            device,
            context,
            Backend::Nvenc,
            VideoCodec::H264,
            20,
            width,
            height,
            fps,
        )
    }

    /// Build an encoder for `backend` × `codec` on `device` for `width`x`height`
    /// @ `fps`, targeting `bitrate_mbps`.
    ///
    /// `device`/`context` are our shared D3D11 device (must have
    /// `VIDEO_SUPPORT`). They are AddRef'd and handed to FFmpeg, which Releases
    /// them when the encoder is dropped.
    fn build(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        backend: Backend,
        codec: VideoCodec,
        bitrate_mbps: u32,
        width: u32,
        height: u32,
        fps: u32,
    ) -> std::result::Result<Self, String> {
        // NV12 is 4:2:0 — dimensions must be even (matches convert.rs).
        let width = width & !1;
        let height = height & !1;
        let fps = fps.clamp(1, 480);
        let bitrate_bps = (bitrate_mbps.clamp(1, 200) as i64) * 1_000_000;

        // Build incrementally; on any error tear down what we've made so far.
        let mut enc = Encoder {
            backend,
            codec,
            bitrate_bps,
            context: context.clone(),
            codec_ctx: ptr::null_mut(),
            d3d11_frames: ptr::null_mut(),
            qsv_frames: ptr::null_mut(),
            d3d11_device_ctx: ptr::null_mut(),
            qsv_device_ctx: ptr::null_mut(),
            packet: ptr::null_mut(),
            width,
            height,
            fps,
        };
        match enc.init(device, context) {
            Ok(()) => Ok(enc),
            Err(e) => Err(e), // enc's Drop frees any partial allocations
        }
    }

    /// The codec this encoder actually opened with (may differ from the requested
    /// one after fallback). Its [`VideoCodec::av_codec_id`] is what the muxers use.
    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    fn init(
        &mut self,
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
    ) -> std::result::Result<(), String> {
        // QSV (oneVPL/MediaSDK) requires the D3D11 device handed to it to have
        // multithread protection enabled. FFmpeg sets this when it creates the
        // device itself, but not when we supply our own — without it,
        // av_hwdevice_ctx_create_derived(QSV) fails with MFX "Error setting
        // child device handle: -16" (MFX_ERR_UNDEFINED_BEHAVIOR).
        if let Ok(mt) = context.cast::<ID3D11Multithread>() {
            unsafe {
                let _ = mt.SetMultithreadProtected(true);
            }
        }

        unsafe {
            // 1. D3D11VA hw device context wrapping OUR device (no FFmpeg device).
            self.d3d11_device_ctx = ffi::av_hwdevice_ctx_alloc(ffi::AV_HWDEVICE_TYPE_D3D11VA);
            if self.d3d11_device_ctx.is_null() {
                return Err("av_hwdevice_ctx_alloc(D3D11VA) failed".into());
            }
            let dev_ctx = (*self.d3d11_device_ctx).data as *mut ffi::AVHWDeviceContext;
            let d3d11_hwctx = (*dev_ctx).hwctx as *mut AVD3D11VADeviceContext;
            // Hand FFmpeg owned references (clone AddRefs, into_raw transfers).
            (*d3d11_hwctx).device = device.clone().into_raw();
            (*d3d11_hwctx).device_context = context.clone().into_raw();
            // video_device/video_context left NULL → FFmpeg derives them.
            let r = ffi::av_hwdevice_ctx_init(self.d3d11_device_ctx);
            if r < 0 {
                return Err(format!("av_hwdevice_ctx_init(D3D11VA): {}", av_err(r)));
            }

            // 2. (QSV only) Derive a QSV device from the D3D11 device (same
            //    adapter). NVENC consumes D3D11 frames directly, so it skips this
            //    — and must, since QSV can't derive from a non-Intel device.
            if self.backend == Backend::Qsv {
                let r = ffi::av_hwdevice_ctx_create_derived(
                    &mut self.qsv_device_ctx,
                    ffi::AV_HWDEVICE_TYPE_QSV,
                    self.d3d11_device_ctx,
                    0,
                );
                if r < 0 {
                    return Err(format!(
                        "av_hwdevice_ctx_create_derived(QSV): {}",
                        av_err(r)
                    ));
                }
            }

            // 3. D3D11 frames context (format D3D11, sw NV12). initial_pool_size
            //    = 0: we supply our own textures per frame (convert.rs output).
            self.d3d11_frames = ffi::av_hwframe_ctx_alloc(self.d3d11_device_ctx);
            if self.d3d11_frames.is_null() {
                return Err("av_hwframe_ctx_alloc(D3D11) failed".into());
            }
            let frames = (*self.d3d11_frames).data as *mut ffi::AVHWFramesContext;
            (*frames).format = ffi::AV_PIX_FMT_D3D11;
            (*frames).sw_format = ffi::AV_PIX_FMT_NV12;
            (*frames).width = self.width as i32;
            (*frames).height = self.height as i32;
            let d3d11_frames_hwctx = (*frames).hwctx as *mut AVD3D11VAFramesContext;
            match self.backend {
                Backend::Qsv => {
                    // We supply our own RENDER_TARGET NV12 textures per frame
                    // (from convert.rs) and map them to QSV; no FFmpeg pool.
                    (*frames).initial_pool_size = 0;
                    (*d3d11_frames_hwctx).bind_flags = D3D11_BIND_RENDER_TARGET;
                }
                Backend::Nvenc => {
                    // NVENC will not encode an externally-created standalone
                    // texture (EncodePicture → invalid param regardless of bind
                    // flags). It needs frames from FFmpeg's own pool — a DECODER
                    // texture ARRAY it pre-manages. We GPU-copy the converted NV12
                    // into a pool slice in encode(). DECODER bind = canonical
                    // NVENC input.
                    (*frames).initial_pool_size = NVENC_POOL_SIZE as i32;
                    (*d3d11_frames_hwctx).bind_flags = D3D11_BIND_DECODER;
                }
            }
            let r = ffi::av_hwframe_ctx_init(self.d3d11_frames);
            if r < 0 {
                return Err(format!("av_hwframe_ctx_init(D3D11): {}", av_err(r)));
            }

            // 4. (QSV only) QSV frames context derived from the D3D11 frames
            //    context. NVENC encodes the D3D11 frames directly.
            if self.backend == Backend::Qsv {
                let r = ffi::av_hwframe_ctx_create_derived(
                    &mut self.qsv_frames,
                    ffi::AV_PIX_FMT_QSV,
                    self.qsv_device_ctx,
                    self.d3d11_frames,
                    0,
                );
                if r < 0 {
                    return Err(format!(
                        "av_hwframe_ctx_create_derived(QSV frames): {}",
                        av_err(r)
                    ));
                }
            }

            // 5. Encoder context. Input is QSV frames (Intel) or D3D11 frames
            //    (NVENC, direct). Opened with a low-latency / low-GPU-load option
            //    set so capture doesn't steal frames from the game; falls back to
            //    progressively barer sets if a GPU/driver rejects an option (see
            //    `Backend::open_option_sets`). A failed `avcodec_open2` taints the
            //    context, so each attempt re-allocates a fresh one.
            let cname = CString::new(self.backend.encoder_name(self.codec)).unwrap();
            let codec = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            if codec.is_null() {
                return Err(format!(
                    "{} encoder not found",
                    self.backend.encoder_name(self.codec)
                ));
            }

            let attempts = self.backend.open_option_sets();
            let mut opened = false;
            let mut last_err = String::new();
            for (attempt_idx, set) in attempts.iter().enumerate() {
                if !self.codec_ctx.is_null() {
                    ffi::avcodec_free_context(&mut self.codec_ctx);
                }
                self.codec_ctx = ffi::avcodec_alloc_context3(codec);
                if self.codec_ctx.is_null() {
                    return Err("avcodec_alloc_context3 failed".into());
                }
                let c = &mut *self.codec_ctx;
                c.width = self.width as i32;
                c.height = self.height as i32;
                // QSV consumes mapped QSV frames; NVENC the D3D11 frames directly.
                c.pix_fmt = match self.backend {
                    Backend::Qsv => ffi::AV_PIX_FMT_QSV,
                    Backend::Nvenc => ffi::AV_PIX_FMT_D3D11,
                };
                c.time_base = ffi::AVRational {
                    num: 1,
                    den: self.fps as i32,
                };
                c.framerate = ffi::AVRational {
                    num: self.fps as i32,
                    den: 1,
                };
                c.gop_size = self.fps as i32; // keyint = 1s (clip cut points)
                c.max_b_frames = 0; // no B-frames: no reorder delay on the capture path
                c.bit_rate = self.bitrate_bps; // from Settings::bitrate_mbps
                                               // Emit SPS/PPS as avcC extradata (not in-band) so mux.rs can write
                                               // them once into the MP4 header on stream-copy.
                c.flags |= AV_CODEC_FLAG_GLOBAL_HEADER;
                c.hw_frames_ctx = match self.backend {
                    Backend::Qsv => ffi::av_buffer_ref(self.qsv_frames),
                    Backend::Nvenc => ffi::av_buffer_ref(self.d3d11_frames),
                };

                let mut opts: *mut ffi::AVDictionary = ptr::null_mut();
                for (k, v) in set.iter() {
                    let ck = CString::new(*k).unwrap();
                    let cv = CString::new(*v).unwrap();
                    ffi::av_dict_set(&mut opts, ck.as_ptr(), cv.as_ptr(), 0);
                }

                let r = ffi::avcodec_open2(self.codec_ctx, codec, &mut opts);
                // Options the encoder didn't consume stay in the dict; on a
                // successful open that means an unrecognized option name.
                let leftover = ffi::av_dict_count(opts);
                ffi::av_dict_free(&mut opts);

                if r >= 0 {
                    if leftover > 0 {
                        tracing::warn!(
                            encoder = self.backend.encoder_name(self.codec),
                            "encoder ignored {leftover} unrecognized option(s)"
                        );
                    }
                    tracing::info!(
                        encoder = self.backend.encoder_name(self.codec),
                        attempt = attempt_idx,
                        options = ?set,
                        width = self.width,
                        height = self.height,
                        fps = self.fps,
                        "opened hardware encoder"
                    );
                    opened = true;
                    break;
                }

                last_err = av_err(r);
                tracing::warn!(
                    encoder = self.backend.encoder_name(self.codec),
                    attempt = attempt_idx,
                    options = ?set,
                    "avcodec_open2 failed ({last_err}); trying a more conservative option set"
                );
            }
            if !opened {
                return Err(format!(
                    "avcodec_open2({}) failed for every option set: {}",
                    self.backend.encoder_name(self.codec),
                    last_err
                ));
            }

            self.packet = ffi::av_packet_alloc();
            if self.packet.is_null() {
                return Err("av_packet_alloc failed".into());
            }
        }
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The encoder's codec extradata (avcC SPS/PPS), populated by `avcodec_open2`
    /// because we set `AV_CODEC_FLAG_GLOBAL_HEADER`. `mux.rs` writes this into the
    /// MP4 sample description so stream-copied clips are decodable. Empty until
    /// the encoder has produced it (always available right after `new_qsv`).
    pub fn extradata(&self) -> Vec<u8> {
        // SAFETY: codec_ctx is a valid open context for the encoder's lifetime.
        unsafe {
            let c = &*self.codec_ctx;
            if c.extradata.is_null() || c.extradata_size <= 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
        }
    }

    /// Encode one NV12 D3D11 texture (from `convert.rs`) at `pts` (in time_base
    /// units, i.e. frame index for our 1/fps base). Returns any packets the
    /// encoder produced (may be empty while it fills its pipeline).
    ///
    /// The texture is AddRef'd for the duration FFmpeg holds the frame; no CPU
    /// readback happens — only the compressed packet crosses to the CPU.
    pub fn encode(
        &mut self,
        nv12: &ID3D11Texture2D,
        pts: i64,
    ) -> std::result::Result<Vec<EncodedPacket>, String> {
        unsafe {
            let r = match self.backend {
                Backend::Nvenc => self.encode_nvenc(nv12, pts),
                Backend::Qsv => self.encode_qsv(nv12, pts),
            };
            match r {
                Ok(()) => self.drain(),
                Err(e) => Err(e),
            }
        }
    }

    /// NVENC: pull a frame from FFmpeg's D3D11 pool (a DECODER texture array
    /// NVENC pre-manages) and GPU-copy the converted NV12 into its array slice,
    /// then submit. No CPU readback — only a GPU→GPU `CopySubresourceRegion`.
    /// (NVENC rejects externally-created standalone textures.)
    unsafe fn encode_nvenc(
        &mut self,
        nv12: &ID3D11Texture2D,
        pts: i64,
    ) -> std::result::Result<(), String> {
        let frame = ffi::av_frame_alloc();
        if frame.is_null() {
            return Err("av_frame_alloc(nvenc) failed".into());
        }
        let r = ffi::av_hwframe_get_buffer(self.d3d11_frames, frame, 0);
        if r < 0 {
            let mut f = frame;
            ffi::av_frame_free(&mut f);
            return Err(format!("av_hwframe_get_buffer(D3D11 pool): {}", av_err(r)));
        }

        // The pool frame: data[0] = ID3D11Texture2D* (the array), data[1] = the
        // array slice index. Copy our NV12 into that slice (GPU→GPU).
        let dst_raw = (*frame).data[0] as *mut c_void;
        let slice = (*frame).data[1] as usize as u32;
        let copy_result = (|| -> std::result::Result<(), String> {
            let dst_tex = ID3D11Texture2D::from_raw_borrowed(&dst_raw)
                .ok_or("pool frame has null texture")?;
            let dst: ID3D11Resource = dst_tex.cast().map_err(|e| format!("pool tex: {e}"))?;
            let src: ID3D11Resource = nv12.cast().map_err(|e| format!("nv12: {e}"))?;
            self.context
                .CopySubresourceRegion(&dst, slice, 0, 0, 0, &src, 0, None);
            Ok(())
        })();
        if let Err(e) = copy_result {
            let mut f = frame;
            ffi::av_frame_free(&mut f);
            return Err(e);
        }

        (*frame).pts = pts;
        let r = ffi::avcodec_send_frame(self.codec_ctx, frame);
        let mut f = frame;
        ffi::av_frame_free(&mut f); // encoder took its own ref
        if r < 0 {
            return Err(format!("avcodec_send_frame(nvenc): {}", av_err(r)));
        }
        Ok(())
    }

    /// QSV: wrap our NV12 texture as a D3D11 frame and map it (no copy) to an
    /// AV_PIX_FMT_QSV frame the `h264_qsv` encoder consumes.
    unsafe fn encode_qsv(
        &mut self,
        nv12: &ID3D11Texture2D,
        pts: i64,
    ) -> std::result::Result<(), String> {
        let d3d11_frame = ffi::av_frame_alloc();
        if d3d11_frame.is_null() {
            return Err("av_frame_alloc(d3d11) failed".into());
        }
        (*d3d11_frame).format = ffi::AV_PIX_FMT_D3D11;
        (*d3d11_frame).width = self.width as i32;
        (*d3d11_frame).height = self.height as i32;
        (*d3d11_frame).hw_frames_ctx = ffi::av_buffer_ref(self.d3d11_frames);
        // data[0] = ID3D11Texture2D*, data[1] = array slice index (0, single).
        let tex_owned = nv12.clone().into_raw(); // AddRef; released by buf free
        (*d3d11_frame).data[0] = tex_owned as *mut u8;
        (*d3d11_frame).data[1] = ptr::null_mut(); // subresource index 0
                                                  // buf[0] keeps the texture alive while the encoder (and the QSV mapping
                                                  // derived from it) hold the frame.
        (*d3d11_frame).buf[0] =
            ffi::av_buffer_create(tex_owned as *mut u8, 0, Some(release_texture), tex_owned, 0);
        if (*d3d11_frame).buf[0].is_null() {
            release_texture(tex_owned, tex_owned as *mut u8);
            let mut f = d3d11_frame;
            ffi::av_frame_free(&mut f);
            return Err("av_buffer_create(texture) failed".into());
        }
        (*d3d11_frame).pts = pts;

        let qsv_frame = ffi::av_frame_alloc();
        if qsv_frame.is_null() {
            let mut f = d3d11_frame;
            ffi::av_frame_free(&mut f);
            return Err("av_frame_alloc(qsv) failed".into());
        }
        (*qsv_frame).format = ffi::AV_PIX_FMT_QSV;
        (*qsv_frame).width = self.width as i32;
        (*qsv_frame).height = self.height as i32;
        (*qsv_frame).hw_frames_ctx = ffi::av_buffer_ref(self.qsv_frames);
        let mr = ffi::av_hwframe_map(
            qsv_frame,
            d3d11_frame,
            (ffi::AV_HWFRAME_MAP_DIRECT | ffi::AV_HWFRAME_MAP_READ) as i32,
        );
        if mr < 0 {
            let mut a = d3d11_frame;
            let mut b = qsv_frame;
            ffi::av_frame_free(&mut a);
            ffi::av_frame_free(&mut b);
            return Err(format!("av_hwframe_map(D3D11→QSV): {}", av_err(mr)));
        }
        (*qsv_frame).pts = pts;
        let r = ffi::avcodec_send_frame(self.codec_ctx, qsv_frame);
        let mut a = d3d11_frame;
        let mut b = qsv_frame;
        ffi::av_frame_free(&mut a);
        ffi::av_frame_free(&mut b);
        if r < 0 {
            return Err(format!("avcodec_send_frame(qsv): {}", av_err(r)));
        }
        Ok(())
    }

    /// Flush the encoder (send EOF) and return any remaining packets.
    pub fn flush(&mut self) -> std::result::Result<Vec<EncodedPacket>, String> {
        unsafe {
            let r = ffi::avcodec_send_frame(self.codec_ctx, ptr::null());
            if r < 0 && r != AVERROR_EOF {
                return Err(format!("avcodec_send_frame(flush): {}", av_err(r)));
            }
            self.drain()
        }
    }

    /// Drain all packets currently available from the encoder.
    unsafe fn drain(&mut self) -> std::result::Result<Vec<EncodedPacket>, String> {
        let mut out = Vec::new();
        loop {
            let r = ffi::avcodec_receive_packet(self.codec_ctx, self.packet);
            if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                break;
            }
            if r < 0 {
                return Err(format!("avcodec_receive_packet: {}", av_err(r)));
            }
            let pkt = &*self.packet;
            let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize).to_vec();
            // AV_PKT_FLAG_KEY == 1
            let keyframe = (pkt.flags & 1) != 0;
            out.push(EncodedPacket {
                data,
                pts: pkt.pts,
                dts: pkt.dts,
                keyframe,
            });
            ffi::av_packet_unref(self.packet);
        }
        Ok(out)
    }
}

/// av_buffer_create free callback: Release the ID3D11Texture2D we AddRef'd.
unsafe extern "C" fn release_texture(opaque: *mut c_void, _data: *mut u8) {
    if !opaque.is_null() {
        // Reconstitute the owned interface and drop it → ID3D11Texture2D::Release.
        drop(ID3D11Texture2D::from_raw(opaque));
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.codec_ctx.is_null() {
                ffi::avcodec_free_context(&mut self.codec_ctx);
            }
            // Frames contexts before device contexts; derived before parent.
            if !self.qsv_frames.is_null() {
                ffi::av_buffer_unref(&mut self.qsv_frames);
            }
            if !self.d3d11_frames.is_null() {
                ffi::av_buffer_unref(&mut self.d3d11_frames);
            }
            if !self.qsv_device_ctx.is_null() {
                ffi::av_buffer_unref(&mut self.qsv_device_ctx);
            }
            if !self.d3d11_device_ctx.is_null() {
                ffi::av_buffer_unref(&mut self.d3d11_device_ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_links_and_nvenc_is_available() {
        let p = probe();
        println!(
            "avutil={} avcodec={} avformat={}",
            p.avutil_version, p.avcodec_version, p.avformat_version
        );
        for e in &p.encoders {
            println!("  {} = {}", e.name, e.available);
        }

        // Versions must decode to FFmpeg 8.x (avcodec major 62) — proves the ABI.
        assert!(
            p.avcodec_version.starts_with("62."),
            "unexpected avcodec version {} (ABI/link mismatch?)",
            p.avcodec_version
        );
        // The whole point of bundling this build: NVENC reachable from Rust.
        assert!(
            encoder_exists("h264_nvenc"),
            "h264_nvenc not found in linked FFmpeg"
        );
    }

    /// End-to-end answer: convert a synthetic BGRA frame to NV12 on the
    /// display adapter, feed that RENDER_TARGET NV12 texture to `h264_qsv`, and
    /// require at least one compressed packet back. Proves the zero-copy
    /// D3D11→QSV map works (or tells us we need the DECODER-copy fallback).
    #[test]
    fn qsv_encodes_nv12_from_convert() {
        use crate::core::{convert::Converter, device};

        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (d3d_device, ctx, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        // Synthetic BGRA "frame" → NV12 via the real converter.
        let src_desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
            BindFlags: windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&src_desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();

        let conv = Converter::new(&d3d_device, &ctx, w, h, w, h).expect("converter");

        let mut enc = match Encoder::new_qsv(&d3d_device, &ctx, w, h, fps) {
            Ok(e) => e,
            Err(e) => panic!("Encoder::new_qsv failed: {e}"),
        };

        // Feed a handful of frames so the QSV pipeline fills and emits.
        let mut total = 0usize;
        for i in 0..30i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12 tex");
            conv.convert(&bgra, &nv12).expect("convert");
            let pkts = enc.encode(&nv12, i).expect("encode");
            total += pkts.len();
        }
        let tail = enc.flush().expect("flush");
        total += tail.len();

        println!("h264_qsv produced {total} packet(s) from {}x{} NV12", w, h);
        assert!(total > 0, "encoder produced no packets");
    }

    /// Diagnostic: does `h264_nvenc` encode a plain SOFTWARE NV12 frame on this
    /// machine at all? Isolates "NVENC works" from "our D3D11 input is wrong".
    #[test]
    fn nvenc_encodes_software_nv12() {
        unsafe {
            let cname = CString::new("h264_nvenc").unwrap();
            let codec = ffi::avcodec_find_encoder_by_name(cname.as_ptr());
            assert!(!codec.is_null(), "h264_nvenc missing");
            let cctx = ffi::avcodec_alloc_context3(codec);
            {
                let c = &mut *cctx;
                c.width = 1280;
                c.height = 720;
                c.pix_fmt = ffi::AV_PIX_FMT_NV12;
                c.time_base = ffi::AVRational { num: 1, den: 60 };
                c.framerate = ffi::AVRational { num: 60, den: 1 };
                c.gop_size = 60;
                c.max_b_frames = 0;
                c.bit_rate = 20_000_000;
            }
            let r = ffi::avcodec_open2(cctx, codec, ptr::null_mut());
            assert!(r >= 0, "avcodec_open2(h264_nvenc sw): {}", av_err(r));
            let pkt = ffi::av_packet_alloc();
            let mut total = 0usize;
            for i in 0..30i64 {
                let mut f = ffi::av_frame_alloc();
                (*f).format = ffi::AV_PIX_FMT_NV12;
                (*f).width = 1280;
                (*f).height = 720;
                assert!(ffi::av_frame_get_buffer(f, 0) >= 0, "get_buffer");
                (*f).pts = i;
                let r = ffi::avcodec_send_frame(cctx, f);
                ffi::av_frame_free(&mut f);
                assert!(r >= 0, "send_frame(sw nv12): {}", av_err(r));
                loop {
                    let rr = ffi::avcodec_receive_packet(cctx, pkt);
                    if rr == AVERROR_EAGAIN || rr == AVERROR_EOF {
                        break;
                    }
                    assert!(rr >= 0, "receive: {}", av_err(rr));
                    total += 1;
                    ffi::av_packet_unref(pkt);
                }
            }
            println!("h264_nvenc (software NV12) produced {total} packet(s)");
            assert!(total > 0, "software nvenc produced no packets");
        }
    }

    /// NVENC counterpart: create the device on an NVIDIA adapter, convert a
    /// synthetic BGRA frame to NV12, and feed that `AV_PIX_FMT_D3D11` texture to
    /// `h264_nvenc` **directly** (no QSV derive/map). Proves the vendor-aware
    /// NVENC path used when capturing on an NVIDIA display adapter. Skips if no
    /// NVIDIA adapter is present.
    #[test]
    fn nvenc_encodes_nv12_from_convert() {
        use crate::core::{convert::Converter, device};

        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let Some(nv) = gpus
            .iter()
            .find(|g| g.vendor == Vendor::Nvidia && !g.is_software)
        else {
            eprintln!("no NVIDIA adapter present — skipping NVENC test");
            return;
        };
        let adapter = device::adapter_at(nv.index).expect("adapter_at");
        let (d3d_device, ctx, _fl) = device::create_device(Some(&adapter)).expect("create device");
        let (w, h, fps) = (1280u32, 720u32, 60u32);

        // Synthetic BGRA "frame" → NV12 via the real converter.
        let src_desc = windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT,
            BindFlags: windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut bgra: Option<ID3D11Texture2D> = None;
        unsafe {
            d3d_device
                .CreateTexture2D(&src_desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();

        let conv = Converter::new(&d3d_device, &ctx, w, h, w, h).expect("converter");
        let mut enc = match Encoder::new_nvenc(&d3d_device, &ctx, w, h, fps) {
            Ok(e) => e,
            Err(e) => panic!("Encoder::new_nvenc failed: {e}"),
        };

        let mut out_pts = Vec::new();
        for i in 0..30i64 {
            let nv12 = conv.create_nv12_texture().expect("nv12 tex");
            conv.convert(&bgra, &nv12).expect("convert");
            // Feed real-time-style PTS (frame i at 1/fps): 0, 1, 2, ...
            for p in enc.encode(&nv12, i).expect("encode") {
                out_pts.push(p.pts);
            }
        }
        for p in enc.flush().expect("flush") {
            out_pts.push(p.pts);
        }

        println!(
            "h264_nvenc produced {} packet(s) from {w}x{h} NV12",
            out_pts.len()
        );
        // PTS must be preserved (input was 0..29) — else clip duration/timing
        // breaks (clips would come out time-compressed).
        assert_eq!(out_pts.iter().copied().min(), Some(0));
        assert_eq!(out_pts.iter().copied().max(), Some(29));
        assert!(!out_pts.is_empty(), "nvenc produced no packets");
    }
}
