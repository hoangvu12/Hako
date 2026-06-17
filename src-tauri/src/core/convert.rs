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

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

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
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
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
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        in_width: u32,
        in_height: u32,
        out_width: u32,
        out_height: u32,
    ) -> Result<Self> {
        let in_width = in_width & !1;
        let in_height = in_height & !1;
        let width = out_width & !1;
        let height = out_height & !1;

        let video_device: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;

        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: 0, Denominator: 0 },
            InputWidth: in_width,
            InputHeight: in_height,
            OutputFrameRate: DXGI_RATIONAL { Numerator: 0, Denominator: 0 },
            OutputWidth: width,
            OutputHeight: height,
            // Capture→encode wants throughput, not playback fidelity.
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };

        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content_desc)? };
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0)? };

        // Color space: WGC delivers full-range BT.709 RGB; H.264 wants
        // studio(limited)-range BT.709 YCbCr. The *ColorSpace1 APIs
        // (ID3D11VideoContext1) express both precisely.
        let video_context1: ID3D11VideoContext1 = video_context.cast()?;
        unsafe {
            video_context1.VideoProcessorSetStreamColorSpace1(
                &processor,
                0,
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
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
            enumerator,
            processor,
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
            self.device
                .CreateTexture2D(&desc, None, Some(&mut tex))?;
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
        self.input_views.borrow_mut().insert(key, input_view.clone());
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

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(input_view.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };

        unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, &output_view, 0, &[stream])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::device;

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
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
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

        let conv = Converter::new(&device, &context, w, h, w, h).expect("converter");
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        conv.convert(&bgra, &nv12).expect("convert");
        println!("converted {}x{} BGRA -> NV12 OK", conv.width(), conv.height());
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
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
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

        let conv = Converter::new(&device, &context, in_w, in_h, out_w, out_h).expect("converter");
        assert_eq!((conv.width(), conv.height()), (out_w, out_h));
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        conv.convert(&bgra, &nv12).expect("convert+scale");
        println!("scaled {in_w}x{in_h} BGRA -> {out_w}x{out_h} NV12 OK");
    }
}
