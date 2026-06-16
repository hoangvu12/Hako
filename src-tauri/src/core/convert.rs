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

/// BGRA → NV12 color converter bound to one shared D3D11 device at a fixed
/// resolution. Reused for every frame; recreate on a resolution change.
pub struct Converter {
    device: ID3D11Device,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    width: u32,
    height: u32,
}

impl Converter {
    /// Build a video processor for `width`x`height` BGRA → NV12 on `device`.
    ///
    /// `device` must have been created with `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`
    /// (our `device::create_device` does). NV12 is 4:2:0, so both dimensions are
    /// rounded down to even values.
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let width = width & !1;
        let height = height & !1;

        let video_device: ID3D11VideoDevice = device.cast()?;
        let video_context: ID3D11VideoContext = context.cast()?;

        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: 0, Denominator: 0 },
            InputWidth: width,
            InputHeight: height,
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
            width,
            height,
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

    /// Convert one BGRA frame into `nv12_out` (must be from `create_nv12_texture`).
    ///
    /// Creates per-call input/output views — WGC recycles its frame-pool
    /// textures, so caching by pointer is a later optimization.
    pub fn convert(&self, bgra: &ID3D11Texture2D, nv12_out: &ID3D11Texture2D) -> Result<()> {
        let bgra_res: ID3D11Resource = bgra.cast()?;
        let nv12_res: ID3D11Resource = nv12_out.cast()?;

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

        let conv = Converter::new(&device, &context, w, h).expect("converter");
        let nv12 = conv.create_nv12_texture().expect("nv12 tex");
        conv.convert(&bgra, &nv12).expect("convert");
        println!("converted {}x{} BGRA -> NV12 OK", conv.width(), conv.height());
    }
}
