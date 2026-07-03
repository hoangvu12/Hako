//! In-frame mouse-cursor compositor ("record cursor").
//!
//! The graphics-hook (Game Capture) path shares the game's **backbuffer**, and on
//! Windows the mouse pointer is a *hardware cursor* — composited by the OS/GPU
//! **after** the game presents, so it never lands in the backbuffer we capture.
//! League of Legends, Valorant, and most games use a hardware cursor, so without
//! this the clip shows no pointer at all. This is exactly why OBS "Game Capture"
//! and Medal expose a "capture cursor" / "show mouse cursor" toggle: they draw the
//! cursor host-side. We do the same — query the live cursor (`GetCursorInfo`),
//! rasterize its shape (GDI), and composite it onto the captured BGRA staging
//! texture *before* NV12 convert/encode, at the cursor's position mapped into the
//! frame.
//!
//! Like [`crate::core::overlay_card`], Direct2D draws onto the staging texture
//! (created with `BIND_RENDER_TARGET` on a `D3D11_CREATE_DEVICE_BGRA_SUPPORT`
//! device) and everything lives on the encode thread (the D2D context is used
//! single-threaded). Cursor shapes are rasterized once and cached by `HCURSOR`.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::Interface;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT, D2D_RECT_F,
    D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateDevice, ID2D1Bitmap1, ID2D1DeviceContext, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_INTERPOLATION_MODE_LINEAR,
};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D, D3D11_TEXTURE2D_DESC};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetCursorInfo, CURSORINFO, CURSOR_SHOWING,
};

/// A rasterized cursor shape ready to blend: a premultiplied-BGRA D2D bitmap plus
/// its size (source pixels) and hotspot (the click point, relative to the bitmap's
/// top-left — subtracted from the cursor position so the shape lands correctly).
struct CachedCursor {
    bmp: ID2D1Bitmap1,
    width: u32,
    height: u32,
    hotspot_x: i32,
    hotspot_y: i32,
}

/// Cap on distinct cached shapes. Cursors are a small, stable set of shared
/// handles; the cap only guards against unbounded growth if a game churns custom
/// cursors (and against a freed-then-reallocated handle aliasing a stale entry).
const MAX_CACHED_CURSORS: usize = 32;

/// Direct2D compositor that stamps the live mouse cursor onto captured frames.
/// Lives on the encode thread.
pub struct CursorOverlay {
    device: ID3D11Device,
    ctx: ID2D1DeviceContext,
    /// D2D target bitmaps wrapping each staging texture, keyed by COM pointer —
    /// built once and reused, exactly like [`crate::core::overlay_card`].
    targets: RefCell<HashMap<*mut c_void, ID2D1Bitmap1>>,
    /// Rasterized cursor shapes keyed by `HCURSOR`. `None` marks a handle we could
    /// not rasterize, so we don't retry it every frame.
    shapes: RefCell<HashMap<isize, Option<CachedCursor>>>,
}

impl CursorOverlay {
    /// Build the compositor on `device` (the shared capture device). Returns the
    /// reason on failure so the caller can log and proceed without a cursor.
    pub fn new(device: &ID3D11Device) -> Result<Self, String> {
        let dxgi: IDXGIDevice = device
            .cast()
            .map_err(|e| format!("cast IDXGIDevice: {e}"))?;
        let d2d_device = unsafe { D2D1CreateDevice(&dxgi, None) }
            .map_err(|e| format!("D2D1CreateDevice: {e}"))?;
        let ctx = unsafe { d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE) }
            .map_err(|e| format!("CreateDeviceContext: {e}"))?;
        Ok(Self {
            device: device.clone(),
            ctx,
            targets: RefCell::new(HashMap::new()),
            shapes: RefCell::new(HashMap::new()),
        })
    }

    /// Composite the current mouse cursor onto `staging` (the freshly-captured
    /// frame) at its on-screen position, mapped into the frame. `hwnd_raw` is the
    /// captured window (screen→client mapping); `frame_w`/`frame_h` are the staging
    /// dimensions (the backbuffer size). No-op when the cursor is hidden, off the
    /// window, or its shape can't be rasterized. Best-effort — any D2D error is
    /// returned for the caller to log once, never fatal to the capture loop.
    pub fn draw(
        &self,
        staging: &ID3D11Texture2D,
        hwnd_raw: i64,
        frame_w: u32,
        frame_h: u32,
    ) -> Result<(), String> {
        // Live cursor: handle, visibility, and screen position (of the hotspot).
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetCursorInfo(&mut ci) }.is_err() {
            return Ok(());
        }
        // CURSOR_SHOWING clear ⇒ pointer hidden (many games hide the OS cursor and
        // draw their own into the scene — which the backbuffer already captured).
        if (ci.flags.0 & CURSOR_SHOWING.0) == 0 || ci.hCursor.is_invalid() {
            return Ok(());
        }

        let handle = ci.hCursor.0 as isize;
        // Rasterize (or fetch cached) this shape. A handle we failed to rasterize
        // is cached as `None` so we don't retry it every frame.
        {
            let mut shapes = self.shapes.borrow_mut();
            if !shapes.contains_key(&handle) {
                if shapes.len() >= MAX_CACHED_CURSORS {
                    shapes.clear();
                }
                let shape = self.rasterize(ci.hCursor);
                shapes.insert(handle, shape);
            }
            if shapes.get(&handle).and_then(|s| s.as_ref()).is_none() {
                return Ok(());
            }
        }

        // Map the hotspot's screen position into the captured window's client area,
        // then into frame pixels (the backbuffer may be scaled vs. the client, e.g.
        // a game rendering below native resolution).
        let mut pt = POINT {
            x: ci.ptScreenPos.x,
            y: ci.ptScreenPos.y,
        };
        let hwnd = HWND(hwnd_raw as *mut c_void);
        let _ = unsafe { ScreenToClient(hwnd, &mut pt) };
        let (scale_x, scale_y) = client_scale(hwnd, frame_w, frame_h);

        let shapes = self.shapes.borrow();
        let cur = match shapes.get(&handle).and_then(|s| s.as_ref()) {
            Some(c) => c,
            None => return Ok(()),
        };
        let left = (pt.x - cur.hotspot_x) as f32 * scale_x;
        let top = (pt.y - cur.hotspot_y) as f32 * scale_y;
        let dest = D2D_RECT_F {
            left,
            top,
            right: left + cur.width as f32 * scale_x,
            bottom: top + cur.height as f32 * scale_y,
        };

        let target = self.target_for(staging)?;
        unsafe {
            self.ctx.SetTarget(&target);
            self.ctx.BeginDraw();
            self.ctx.DrawBitmap(
                &cur.bmp,
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

    /// Rasterize an `HCURSOR` to a premultiplied-BGRA D2D bitmap + hotspot. Returns
    /// `None` if the shape can't be read or uploaded (the caller caches the miss).
    fn rasterize(&self, hcursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR) -> Option<CachedCursor> {
        let raster = unsafe { cursor_bgra(hcursor) }?;
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
        let bmp = unsafe {
            self.ctx.CreateBitmap(
                D2D_SIZE_U {
                    width: raster.width,
                    height: raster.height,
                },
                Some(raster.pixels.as_ptr() as *const c_void),
                raster.width * 4,
                &props,
            )
        }
        .ok()?;
        Some(CachedCursor {
            bmp,
            width: raster.width,
            height: raster.height,
            hotspot_x: raster.hotspot_x,
            hotspot_y: raster.hotspot_y,
        })
    }

    /// The cached D2D target bitmap wrapping `staging`, creating it on first use.
    /// Identical to [`crate::core::overlay_card`]'s target cache.
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
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            ..Default::default()
        };
        let target = unsafe {
            self.ctx
                .CreateBitmapFromDxgiSurface(&surface, Some(&props as *const _))
        }
        .map_err(|e| format!("CreateBitmapFromDxgiSurface: {e}"))?;
        self.targets.borrow_mut().insert(key, target.clone());
        Ok(target)
    }
}

/// A rasterized cursor: premultiplied BGRA pixels + geometry.
struct CursorRaster {
    width: u32,
    height: u32,
    hotspot_x: i32,
    hotspot_y: i32,
    /// `width * height * 4` bytes, premultiplied BGRA, top-down.
    pixels: Vec<u8>,
}

/// Read an `HCURSOR`'s shape into premultiplied BGRA (the format Direct2D blends
/// most reliably). Handles both color cursors (32-bit, with or without a per-pixel
/// alpha channel) and legacy monochrome cursors (an AND/XOR mask pair). Mirrors
/// the icon-reading path in [`crate::core::audio`], with the mask→alpha fallback
/// OBS's `cursor-capture.c` uses.
///
/// SAFETY: calls GDI on the current thread; `hcursor` must be a live cursor handle.
unsafe fn cursor_bgra(
    hcursor: windows::Win32::UI::WindowsAndMessaging::HCURSOR,
) -> Option<CursorRaster> {
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, HICON, ICONINFO};

    let mut ii = ICONINFO::default();
    // A cursor is an icon for GetIconInfo purposes; the handles are interchangeable.
    GetIconInfo(HICON(hcursor.0), &mut ii).ok()?;
    let hotspot_x = ii.xHotspot as i32;
    let hotspot_y = ii.yHotspot as i32;
    let del = |h: HBITMAP| {
        if !h.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(h.0));
        }
    };

    let read_dib = |hbm: HBITMAP, w: i32, h: i32| -> Option<Vec<u8>> {
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            ..Default::default()
        };
        let mut bi = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let hdc = GetDC(None);
        let lines = GetDIBits(
            hdc,
            hbm,
            0,
            h as u32,
            Some(buf.as_mut_ptr() as *mut c_void),
            &mut bi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, hdc);
        (lines != 0).then_some(buf)
    };

    let result = if !ii.hbmColor.is_invalid() {
        // Color cursor: read the color bitmap; derive alpha from the AND mask when
        // it carries no per-pixel alpha (classic 24-bit-in-32 cursors).
        let mut bmp = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(ii.hbmColor.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut c_void),
        );
        let (w, h) = (bmp.bmWidth, bmp.bmHeight);
        if got == 0 || w <= 0 || h <= 0 || w > 512 || h > 512 {
            None
        } else if let Some(mut color) = read_dib(ii.hbmColor, w, h) {
            let any_alpha = color.chunks_exact(4).any(|px| px[3] != 0);
            if !any_alpha {
                if let Some(mask) = read_dib(ii.hbmMask, w, h) {
                    // AND mask: a set (white, non-zero) pixel is transparent.
                    for (px, m) in color.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                        px[3] = if m[0] != 0 { 0 } else { 255 };
                    }
                } else {
                    // No mask readable — assume fully opaque so the shape shows.
                    for px in color.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
            }
            premultiply(&mut color);
            Some((w as u32, h as u32, color))
        } else {
            None
        }
    } else {
        // Monochrome cursor: hbmMask is double-height — top half is the AND mask,
        // bottom half the XOR (color) mask. Combine into BGRA.
        let mut bmp = BITMAP::default();
        let got = GetObjectW(
            HGDIOBJ(ii.hbmMask.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bmp as *mut _ as *mut c_void),
        );
        let w = bmp.bmWidth;
        let full_h = bmp.bmHeight;
        let h = full_h / 2;
        if got == 0 || w <= 0 || h <= 0 || w > 512 || h > 512 {
            None
        } else if let Some(mask) = read_dib(ii.hbmMask, w, full_h) {
            let stride = (w * 4) as usize;
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let and = mask[y * stride + x * 4]; // top half
                    let xor = mask[(y + h as usize) * stride + x * 4]; // bottom half
                    let o = (y * w as usize + x) * 4;
                    if and != 0 {
                        // Transparent (AND=1). Inverted pixels (AND=1,XOR=1) are
                        // rare for a mouse cursor; leave them transparent.
                        out[o..o + 4].copy_from_slice(&[0, 0, 0, 0]);
                    } else {
                        // Opaque: XOR selects black (0) or white (255).
                        let c = if xor != 0 { 255 } else { 0 };
                        out[o..o + 4].copy_from_slice(&[c, c, c, 255]);
                    }
                }
            }
            // Already opaque/transparent with full-value colors → premultiply is a
            // no-op for alpha 0/255, but keep it uniform.
            premultiply(&mut out);
            Some((w as u32, h as u32, out))
        } else {
            None
        }
    };

    del(ii.hbmColor);
    del(ii.hbmMask);

    let (width, height, pixels) = result?;
    Some(CursorRaster {
        width,
        height,
        hotspot_x,
        hotspot_y,
        pixels,
    })
}

/// Scale straight-BGRA pixels (B,G,R,A) to premultiplied in place.
fn premultiply(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
        px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
        px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
    }
}

/// The frame-vs-client scale for mapping cursor client coords into frame pixels.
/// Normally 1:1 (backbuffer == client size); differs only when the game renders at
/// a non-native resolution. Falls back to 1:1 if the client rect is unreadable.
fn client_scale(hwnd: HWND, frame_w: u32, frame_h: u32) -> (f32, f32) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return (1.0, 1.0);
    }
    let cw = (rc.right - rc.left).max(1) as f32;
    let ch = (rc.bottom - rc.top).max(1) as f32;
    (frame_w as f32 / cw, frame_h as f32 / ch)
}
