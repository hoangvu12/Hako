//! In-frame "tabbed out" freeze overlay.
//!
//! When the capture is frozen (the game minimized / alt-tabbed out of exclusive
//! fullscreen, or a stale swapchain), the recorded frame would otherwise be a
//! silently-held last frame — confusing to anyone watching the clip back. This
//! composites a pre-designed card (a dimming layer + centered icon/text, baked
//! into `assets/freeze-card.png`) straight onto the captured BGRA frame *before*
//! it is converted to NV12 and encoded, so the freeze reads as an intentional
//! "player tabbed out" state.
//!
//! Direct2D draws the card onto the staging texture (which `capture.rs` already
//! creates with `BIND_RENDER_TARGET`, on a device already created with
//! `D3D11_CREATE_DEVICE_BGRA_SUPPORT`). The PNG is decoded once with the `png`
//! crate — no WIC/COM — premultiplied, and uploaded to a D2D bitmap. All of this
//! lives on the encode thread (the D2D device context is used single-threaded).

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateDevice, ID2D1Bitmap1, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_INTERPOLATION_MODE_LINEAR,
};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D, D3D11_TEXTURE2D_DESC};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};

/// The freeze card, embedded at build time. Transparent PNG with the dim baked
/// in (see `design/freeze-card.jsx`), so a single `DrawBitmap` both dims the
/// frozen frame and stamps the icon/text.
static CARD_PNG: &[u8] = include_bytes!("../../assets/freeze-card.png");

/// Whether Direct2D can use a staging texture of this format as a device-context
/// target. Only the two 32-bpp UNORM render-target formats qualify (a `B8G8R8X8`
/// or typeless backbuffer can't be a D2D target), so the caller skips the overlay
/// — and the source loop's keep-alive emit — when the game renders in one of them.
pub fn format_supported(format: DXGI_FORMAT) -> bool {
    matches!(format, DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM)
}

/// Direct2D compositor for the freeze card. Lives on the encode thread.
pub struct FreezeOverlay {
    ctx: ID2D1DeviceContext,
    card: ID2D1Bitmap1,
    /// D2D target bitmaps wrapping each staging texture, keyed by COM pointer. The
    /// staging pool is small + fixed, so each wrapper is built once and reused —
    /// the same pattern `convert.rs` uses for its VideoProcessor views.
    targets: RefCell<HashMap<*mut c_void, ID2D1Bitmap1>>,
}

impl FreezeOverlay {
    /// Build the compositor on `device` (the shared capture device). Decodes +
    /// uploads the card up front so `draw` is allocation-free. Returns the reason
    /// on failure so the caller can log and proceed without an overlay.
    pub fn new(device: &ID3D11Device) -> Result<Self, String> {
        let dxgi: IDXGIDevice = device.cast().map_err(|e| format!("cast IDXGIDevice: {e}"))?;
        let d2d_device =
            unsafe { D2D1CreateDevice(&dxgi, None) }.map_err(|e| format!("D2D1CreateDevice: {e}"))?;
        let ctx = unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
            .map_err(|e| format!("CreateDeviceContext: {e}"))?;
        let card = decode_card(&ctx)?;
        Ok(Self {
            ctx,
            card,
            targets: RefCell::new(HashMap::new()),
        })
    }

    /// Composite the card over `staging` (the freshly-captured frame). Stretches
    /// the card to the full frame so its baked dim covers everything; the icon +
    /// text land centered. Best-effort — any D2D error is returned for the caller
    /// to log once, never fatal to the capture loop.
    pub fn draw(&self, staging: &ID3D11Texture2D) -> Result<(), String> {
        let target = self.target_for(staging)?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { staging.GetDesc(&mut desc) };
        let dest = D2D_RECT_F {
            left: 0.0,
            top: 0.0,
            right: desc.Width as f32,
            bottom: desc.Height as f32,
        };

        unsafe {
            self.ctx.SetTarget(&target);
            self.ctx.BeginDraw();
            self.ctx.DrawBitmap(
                &self.card,
                Some(&dest as *const _),
                1.0,
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            self.ctx
                .EndDraw(None, None)
                .map_err(|e| format!("EndDraw: {e}"))?;
        }
        Ok(())
    }

    /// The cached D2D target bitmap wrapping `staging`, creating it on first use.
    fn target_for(&self, staging: &ID3D11Texture2D) -> Result<ID2D1Bitmap1, String> {
        let key = staging.as_raw();
        if let Some(t) = self.targets.borrow().get(&key) {
            return Ok(t.clone());
        }

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { staging.GetDesc(&mut desc) };
        let surface: IDXGISurface = staging
            .cast()
            .map_err(|e| format!("cast IDXGISurface: {e}"))?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: desc.Format,
                // The captured frame is opaque; ignore its alpha when blending the
                // card over it (source-over with the card's premultiplied alpha).
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            // TARGET so D2D renders into it; CANNOT_DRAW because the staging texture
            // is RENDER_TARGET-only (no shader-resource bind) — it can be a target
            // but never a source.
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let target = unsafe { self.ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props as *const _)) }
            .map_err(|e| format!("CreateBitmapFromDxgiSurface: {e}"))?;
        self.targets.borrow_mut().insert(key, target.clone());
        Ok(target)
    }
}

/// Decode the embedded card PNG (straight-alpha RGBA8) and upload it to a
/// premultiplied BGRA D2D bitmap — the format D2D blends most reliably.
fn decode_card(ctx: &ID2D1DeviceContext) -> Result<ID2D1Bitmap1, String> {
    let mut reader = png::Decoder::new(CARD_PNG)
        .read_info()
        .map_err(|e| format!("freeze-card png header: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("freeze-card png decode: {e}"))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "freeze-card png must be RGBA8 (got {:?}/{:?})",
            info.color_type, info.bit_depth
        ));
    }
    let (w, h) = (info.width, info.height);
    let pixels = &buf[..(w as usize * h as usize * 4)];

    // RGBA (straight) → BGRA (premultiplied): swap R/B and scale rgb by alpha.
    let mut bgra = vec![0u8; pixels.len()];
    for (src, dst) in pixels.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
        let a = src[3] as u32;
        dst[0] = ((src[2] as u32 * a + 127) / 255) as u8; // B
        dst[1] = ((src[1] as u32 * a + 127) / 255) as u8; // G
        dst[2] = ((src[0] as u32 * a + 127) / 255) as u8; // R
        dst[3] = src[3]; // A
    }

    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        ..Default::default()
    };
    unsafe {
        ctx.CreateBitmap(
            D2D_SIZE_U {
                width: w,
                height: h,
            },
            Some(bgra.as_ptr() as *const c_void),
            w * 4,
            &props,
        )
    }
    .map_err(|e| format!("create d2d card bitmap: {e}"))
}
