//! GPU color conversion BGRA → NV12 via `ID3D11VideoProcessor` /
//! `VideoProcessorBlt`. Handles BT.709 full→limited range in one GPU pass.
//! No CPU readback: BGRA capture texture → NV12 texture, both GPU-resident.
//!
//! Runs on the encode thread (the `ID3D11VideoContext` is NOT free-threaded).
//! The capture `FrameArrived` thread only hands off the
//! BGRA texture over a bounded channel; all VideoProcessor work happens here.
//!
//! NV12 output textures are created with `BIND_RENDER_TARGET` only (for the
//! VideoProcessor output view). This Intel driver rejects
//! `RENDER_TARGET | DECODER` combined on NV12, but `encode.rs` verified
//! that `h264_qsv` accepts a RENDER_TARGET-only NV12 texture directly as input —
//! so the whole convert→encode path is single-texture zero-copy, no extra copy.
//! The encoder holds frames asynchronously, so the caller keeps a small pool
//! (ring) of NV12 textures rather than reusing one.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator,
    ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
    DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_COLOR_SPACE_TYPE,
    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT, DXGI_FORMAT_NV12,
    DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

/// The DXGI input color space to declare to the VideoProcessor for a captured
/// backbuffer of `format`.
///
/// Games that output HDR present one of two backbuffer formats, each in its own
/// color space — telling the VideoProcessor the *actual* space is what lets it
/// tone-map down to the SDR BT.709 NV12 we encode:
/// - **`R10G10B10A2_UNORM`** → HDR10: BT.2020 primaries, ST.2084 (PQ) transfer.
/// - **`R16G16B16A16_FLOAT`** → scRGB: BT.709 primaries, linear (gamma 1.0).
/// - **everything else** → standard 8-bit SDR: full-range BT.709, gamma 2.2.
///
/// The pipeline previously hardcoded the SDR value for every format. For an SDR
/// game that is correct; for an HDR backbuffer it mislabels PQ/linear content as
/// gamma-2.2 SDR, so even when a frame converts it comes out with wrong colors —
/// and a mid-session SDR↔HDR flip breaks the convert/encode outright. Naming the
/// right space here is hako's equivalent of Medal's `PREFER_HICOLOR` hi-color
/// capture path.
pub fn input_color_space(format: DXGI_FORMAT) -> DXGI_COLOR_SPACE_TYPE {
    match format {
        DXGI_FORMAT_R10G10B10A2_UNORM => DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
        DXGI_FORMAT_R16G16B16A16_FLOAT => DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709,
        _ => DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
    }
}

/// Whether `format` is an HDR backbuffer format (10-bit HDR10 or FP16 scRGB) —
/// i.e. one the VideoProcessor must tone-map rather than pass through the SDR
/// BT.709 fast path. Used for logging / diagnostics.
pub fn is_hdr_format(format: DXGI_FORMAT) -> bool {
    matches!(
        format,
        DXGI_FORMAT_R10G10B10A2_UNORM | DXGI_FORMAT_R16G16B16A16_FLOAT
    )
}

/// BGRA → NV12 color converter bound to one shared D3D11 device at fixed input
/// and output resolutions. Reused for every frame; recreate on a resolution
/// change. When the output dimensions are smaller than the input, the
/// `VideoProcessorBlt` downscales as part of the same GPU pass (it stretches the
/// full input surface onto the full output surface), so resolution scaling is
/// free and stays entirely on the GPU.
pub struct Converter {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    /// `ID3D11VideoContext1` view of `video_context`, kept so the HDR→SDR fallback
    /// (see [`Converter::convert`]) can re-set the input color space at runtime.
    video_context1: ID3D11VideoContext1,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    /// True when the input was set up as an HDR color space (10-bit/FP16 source).
    /// Gates the one-time SDR fallback below.
    hdr_input: bool,
    /// Set once if the driver's VideoProcessor can't tone-map this HDR input to SDR
    /// (`VideoProcessorBlt` fails): we relabel the input SDR BT.709 and retry, so
    /// HDR games still record valid frames (with approximate colors) instead of
    /// dropping every frame. Interior-mutable because `convert` takes `&self`.
    sdr_fallback: Cell<bool>,
    /// Input (captured) frame size — the size of the BGRA textures fed to
    /// [`Converter::convert`].
    in_width: u32,
    in_height: u32,
    /// Output (NV12) frame size — the size of textures from
    /// [`Converter::create_nv12_texture`] and what the encoder is built for. Equal
    /// to the input size when not scaling.
    width: u32,
    height: u32,
    /// Cached VideoProcessor views, keyed by the texture's COM pointer. Both the
    /// BGRA staging pool and the NV12 ring reuse a fixed set of texture objects,
    /// so each view is created once and reused for every frame instead of being
    /// rebuilt per `convert` call (a per-frame driver allocation otherwise). Owned
    /// here; lives on the single encode thread, so `RefCell` is enough.
    input_views: RefCell<HashMap<*mut c_void, ID3D11VideoProcessorInputView>>,
    output_views: RefCell<HashMap<*mut c_void, ID3D11VideoProcessorOutputView>>,
}

impl Converter {
    /// Build a video processor converting `in_width`x`in_height` BGRA →
    /// `out_width`x`out_height` NV12 on `device`. When the output is smaller than
    /// the input the processor downscales in the same pass (see the type docs);
    /// pass equal in/out sizes for a pure color conversion (no scaling).
    ///
    /// `device` must have been created with `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`
    /// (our `device::create_device` does). NV12 is 4:2:0, so all dimensions are
    /// rounded down to even values.
    ///
    /// `src_format` is the captured (staging) texture's format; it selects the
    /// input color space ([`input_color_space`]) so an HDR backbuffer is
    /// tone-mapped to SDR rather than mislabeled as SDR. Pass the same typed
    /// format the staging pool was created with.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        in_width: u32,
        in_height: u32,
        out_width: u32,
        out_height: u32,
        src_format: DXGI_FORMAT,
    ) -> Result<Self> {
        let in_width = in_width & !1;
        let in_height = in_height & !1;
        let width = out_width & !1;
        let height = out_height & !1;

        let video_device: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;

        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: 0,
                Denominator: 0,
            },
            InputWidth: in_width,
            InputHeight: in_height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: 0,
                Denominator: 0,
            },
            OutputWidth: width,
            OutputHeight: height,
            // Capture→encode wants throughput, not playback fidelity.
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };

        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc)? };
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0)? };

        // Color space. Input: the captured backbuffer's actual space, derived
        // from its format — SDR full-range BT.709 (gamma 2.2) for 8-bit games, or
        // an HDR space (HDR10 PQ/BT.2020, or scRGB linear) for a 10-bit/FP16
        // backbuffer. Output: studio(limited)-range BT.709 YCbCr, which is what
        // H.264 wants. Declaring an HDR input against the SDR output makes the
        // VideoProcessor tone-map HDR→SDR in the same pass (driver-dependent;
        // falls back to a best-effort convert if unsupported). The *ColorSpace1
        // APIs (ID3D11VideoContext1) express both precisely.
        let video_context1: ID3D11VideoContext1 = video_context.cast()?;
        unsafe {
            video_context1.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                input_color_space(src_format),
            );
            video_context1.VideoProcessorSetOutputColorSpace1(
                &processor,
                DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
            );
        }

        Ok(Self {
            device: device.clone(),
            video_device,
            video_context,
            video_context1,
            enumerator,
            processor,
            hdr_input: is_hdr_format(src_format),
            sdr_fallback: Cell::new(false),
            in_width,
            in_height,
            width,
            height,
            input_views: RefCell::new(HashMap::new()),
            output_views: RefCell::new(HashMap::new()),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Allocate one NV12 output texture sized for this converter.
    ///
    /// Flags: `RENDER_TARGET` — required for the VideoProcessor output view, and
    /// the converter's only concern. NOTE: this Intel Iris Xe driver rejects
    /// `RENDER_TARGET | DECODER` on NV12 (verified), so a single texture cannot
    /// be both VideoProcessor output and a DECODER-style encoder input here. The
    /// encoder (`encode.rs`) decides whether `h264_qsv` accepts a RENDER_TARGET
    /// NV12 directly (true zero-copy) or needs one GPU→GPU copy into a DECODER
    /// texture — either way no CPU readback. The caller keeps a ring of these
    /// (the encoder holds input surfaces for `async_depth` frames).
    pub fn create_nv12_texture(&self) -> Result<ID3D11Texture2D> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: self.width,
            Height: self.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
        }
        Ok(tex.expect("CreateTexture2D returned null NV12 texture"))
    }

    /// Cached VideoProcessor **input** view for a BGRA texture (created once per
    /// distinct texture; see the cache fields). A view is permanently bound to its
    /// texture, which is sound here because the staging pool reuses fixed textures.
    fn input_view(&self, bgra: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        let key = bgra.as_raw();
        if let Some(v) = self.input_views.borrow().get(&key) {
            return Ok(v.clone());
        }
        let bgra_res: ID3D11Resource = bgra.cast()?;
        let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };
        let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            self.video_device.CreateVideoProcessorInputView(
                &bgra_res,
                &self.enumerator,
                &in_desc,
                Some(&mut input_view),
            )?;
        }
        let input_view = input_view.expect("null input view");
        self.input_views
            .borrow_mut()
            .insert(key, input_view.clone());
        Ok(input_view)
    }

    /// Cached VideoProcessor **output** view for an NV12 texture (created once per
    /// distinct texture; the NV12 ring reuses a fixed set).
    fn output_view(&self, nv12_out: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorOutputView> {
        let key = nv12_out.as_raw();
        if let Some(v) = self.output_views.borrow().get(&key) {
            return Ok(v.clone());
        }
        let nv12_res: ID3D11Resource = nv12_out.cast()?;
        let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
        unsafe {
            self.video_device.CreateVideoProcessorOutputView(
                &nv12_res,
                &self.enumerator,
                &out_desc,
                Some(&mut output_view),
            )?;
        }
        let output_view = output_view.expect("null output view");
        self.output_views
            .borrow_mut()
            .insert(key, output_view.clone());
        Ok(output_view)
    }

    /// Convert one BGRA frame into `nv12_out` (must be from `create_nv12_texture`).
    ///
    /// Input/output views are cached per texture ([`Self::input_view`] /
    /// [`Self::output_view`]) since both pools reuse fixed textures — no per-frame
    /// view allocation on the encode thread.
    pub fn convert(&self, bgra: &ID3D11Texture2D, nv12_out: &ID3D11Texture2D) -> Result<()> {
        let input_view = self.input_view(bgra)?;
        let output_view = self.output_view(nv12_out)?;

        let make_stream = |view: &ID3D11VideoProcessorInputView| D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(view.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };

        let blt = || unsafe {
            self.video_context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                0,
                &[make_stream(&input_view)],
            )
        };

        match blt() {
            Ok(()) => Ok(()),
            // HDR tone-map path: some drivers' VideoProcessors don't implement
            // HDR→SDR conversion (e.g. NVIDIA returns E_NOTIMPL) and fail the Blt.
            // Relabel the input as SDR BT.709 once and retry: a plain convert of a
            // 10-bit surface *does* work on those GPUs (verified), so HDR games
            // still record — with approximate rather than tone-mapped colors —
            // instead of dropping every frame. This mirrors the pragmatic outcome
            // of Medal's fallback (get usable SDR frames) without an OS-capture
            // detour. One-time; logged once.
            Err(e) if self.hdr_input && !self.sdr_fallback.get() => {
                self.sdr_fallback.set(true);
                unsafe {
                    self.video_context1.VideoProcessorSetStreamColorSpace1(
                        &self.processor,
                        0,
                        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                    );
                }
                tracing::warn!(
                    "convert: VideoProcessor can't tone-map HDR→SDR on this GPU ({e:?}); \
                     falling back to an approximate SDR conversion so HDR clips still record"
                );
                blt().map_err(|e2| {
                    tracing::warn!("convert: SDR fallback Blt also failed: {e2:?}");
                    e2
                })
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::device;
    use windows::Win32::Graphics::Dxgi::Common::{
        DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
    };

    /// The input color space must track the backbuffer format: HDR10 (10-bit) and
    /// scRGB (FP16) each get their own space, and every SDR format falls through to
    /// full-range BT.709 gamma-2.2. This is pure logic (no GPU), so it always runs.
    #[test]
    fn input_color_space_matches_format() {
        assert_eq!(
            input_color_space(DXGI_FORMAT_R10G10B10A2_UNORM),
            DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020,
            "10-bit HDR10 backbuffer must be PQ / BT.2020"
        );
        assert_eq!(
            input_color_space(DXGI_FORMAT_R16G16B16A16_FLOAT),
            DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709,
            "FP16 scRGB backbuffer must be linear / BT.709"
        );
        for sdr in [DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM] {
            assert_eq!(
                input_color_space(sdr),
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                "8-bit backbuffer must stay SDR gamma-2.2 BT.709"
            );
        }

        assert!(is_hdr_format(DXGI_FORMAT_R10G10B10A2_UNORM));
        assert!(is_hdr_format(DXGI_FORMAT_R16G16B16A16_FLOAT));
        assert!(!is_hdr_format(DXGI_FORMAT_B8G8R8A8_UNORM));
    }

    /// Builds the video processor on the default device and converts a synthetic
    /// BGRA texture to NV12 — exercises the whole VideoProcessorBlt path headless.
    #[test]
    fn converts_bgra_to_nv12() {
        // Same adapter selection as capture: the display-owning GPU.
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (device, context, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h) = (1280u32, 720u32);

        // A plain BGRA render-target standing in for a captured frame.
        let src_desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
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
            device
                .CreateTexture2D(&src_desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();

        let conv = Converter::new(
            &device,
            &context,
            w,
            h,
            w,
            h,
            windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        )
        .expect("converter");
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        conv.convert(&bgra, &nv12).expect("convert");
        println!(
            "converted {}x{} BGRA -> NV12 OK",
            conv.width(),
            conv.height()
        );
    }

    /// HDR path: a 10-bit `R10G10B10A2_UNORM` (HDR10) input tone-mapped to SDR
    /// NV12. This is exactly what an HDR game's backbuffer needs — it exercises the
    /// PQ/BT.2020 input color space, a `CreateVideoProcessorInputView` on a 10-bit
    /// surface, and the tone-mapping `VideoProcessorBlt`. Before the fix the
    /// converter hardcoded SDR 709 for this format. Skips cleanly if the driver's
    /// VideoProcessor rejects the HDR input (older GPUs) so CI on non-HDR hardware
    /// doesn't fail — the point is that it must not panic on capable hardware.
    #[test]
    fn tonemaps_hdr10_to_nv12() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (device, context, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (w, h) = (1280u32, 720u32);

        // A 10-bit HDR10 render target standing in for an HDR game's backbuffer.
        let src_desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R10G10B10A2_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut hdr: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .CreateTexture2D(&src_desc, None, Some(&mut hdr))
                .expect("create hdr10");
        }
        let hdr = hdr.unwrap();

        // The converter must pick the PQ/BT.2020 input space for this format.
        assert_eq!(
            input_color_space(DXGI_FORMAT_R10G10B10A2_UNORM),
            DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
        );

        let conv = Converter::new(&device, &context, w, h, w, h, DXGI_FORMAT_R10G10B10A2_UNORM)
            .expect("hdr converter");
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        // Must succeed: either the driver tone-maps HDR→SDR directly, or the
        // converter's one-time SDR fallback kicks in and converts anyway. The whole
        // point of the fix is that a 10-bit HDR backbuffer no longer drops frames.
        conv.convert(&hdr, &nv12)
            .expect("HDR10 → NV12 must convert (direct tone-map or SDR fallback)");
        // A second frame exercises the post-fallback steady state (the relabeled
        // color space persists on the processor).
        conv.convert(&hdr, &nv12)
            .expect("HDR10 → NV12 second frame must also convert");
        println!("converted {w}x{h} HDR10 (R10G10B10A2) -> NV12 OK");
    }

    /// Downscale path: 1440p BGRA input → 720p NV12 output in one VideoProcessorBlt.
    /// Verifies the converter accepts differing in/out sizes and the NV12 texture
    /// it hands back is the (smaller) output size.
    #[test]
    fn scales_bgra_to_smaller_nv12() {
        let gpus = device::enumerate_gpus().expect("enumerate gpus");
        let adapter = device::default_capture_index(&gpus)
            .map(|i| device::adapter_at(i).expect("adapter_at"));
        let (device, context, _fl) =
            device::create_device(adapter.as_ref()).expect("create device");
        let (in_w, in_h) = (2560u32, 1440u32);
        let (out_w, out_h) = (1280u32, 720u32);

        let src_desc = D3D11_TEXTURE2D_DESC {
            Width: in_w,
            Height: in_h,
            MipLevels: 1,
            ArraySize: 1,
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
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
            device
                .CreateTexture2D(&src_desc, None, Some(&mut bgra))
                .expect("create bgra");
        }
        let bgra = bgra.unwrap();

        let conv = Converter::new(
            &device,
            &context,
            in_w,
            in_h,
            out_w,
            out_h,
            windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        )
        .expect("converter");
        assert_eq!((conv.width(), conv.height()), (out_w, out_h));
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        conv.convert(&bgra, &nv12).expect("convert+scale");
        println!("scaled {in_w}x{in_h} BGRA -> {out_w}x{out_h} NV12 OK");
    }
}
