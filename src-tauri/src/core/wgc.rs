//! Windows.Graphics.Capture (WGC) frame source — the robustness fallback for the
//! OBS-injection capture path (Part D of the frozen-capture plan).
//!
//! The injection/shtex path freezes when the game stops presenting (minimized,
//! exclusive-fullscreen alt-tab, or a mid-match fullscreen↔borderless switch) —
//! see `docs/plans/frozen-capture-detection.md`. WGC, the same API Medal/Overwolf
//! lean on, captures the *window* via the compositor and keeps delivering frames
//! across those transitions, so it doesn't suffer the silent stale-frame freeze.
//!
//! This module is the reusable **capture source**: it mirrors
//! [`crate::core::hook::RunningHook`]'s hand-off shape — [`WgcCapture::acquire`]
//! returns `(ID3D11Texture2D, ts)` where `ts` is 100-ns `SystemRelativeTime` (the
//! same clock domain as the hook path and the round anchors), so the existing
//! staging-copy → encode pipeline consumes it unchanged. It uses the **free-
//! threaded** frame pool + polling `TryGetNextFrame` (no `DispatcherQueue`/event
//! handler needed), matching the host-paces-the-fps model the source loop already
//! uses.
//!
//! STATUS: compile-verified building block. Wiring it in as a selectable backend
//! and an automatic fallback (switch on a detected freeze) — plus the live-game
//! runtime verification — is the remaining Part D work tracked in
//! `docs/plans/wgc-fallback-capture.md`. It is intentionally not yet on the live
//! capture path so it can't regress the working hook backend.

use windows::core::{Interface, Result as WinResult};
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;

/// Number of buffers in the WGC frame pool. WGC recycles surfaces, so a small
/// pool covers our single-frame-at-a-time polling (the host copies each frame out
/// promptly, exactly like the shtex path).
const FRAME_POOL_BUFFERS: i32 = 2;

/// A live WGC capture of one window. Dropping it stops the session and releases
/// the frame pool. Held on a single thread, like [`crate::core::hook::RunningHook`].
pub struct WgcCapture {
    item: GraphicsCaptureItem,
    device: IDirect3DDevice,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// The previous frame, kept alive until the next `acquire` so the caller's
    /// copy out of its surface stays valid; closed when the next frame arrives.
    last_frame: Option<windows::Graphics::Capture::Direct3D11CaptureFrame>,
    /// Last content size we sized the pool for; a change triggers `Recreate`.
    last_size: SizeInt32,
    /// Token for the `Closed` handler so we can detach it on drop.
    closed_token: i64,
}

// SAFETY: like `RunningHook`, a `WgcCapture` is moved to one dedicated frame-
// source thread and used only there. The WinRT objects are agile (free-threaded
// frame pool), so single-threaded ownership transfer is sound.
unsafe impl Send for WgcCapture {}

impl WgcCapture {
    /// Start capturing `hwnd`, producing frames onto a free-threaded pool bound to
    /// `d3d_device` (which must be the same device feeding the encoder, so the
    /// surfaces are usable without a cross-device copy). Returns an error if the
    /// window can't be captured (e.g. WGC unsupported, or a protected window).
    pub fn start(hwnd: HWND, d3d_device: &windows::Win32::Graphics::Direct3D11::ID3D11Device) -> WinResult<WgcCapture> {
        // Wrap the D3D11 device as a WinRT IDirect3DDevice (WGC's pool binds to it).
        let dxgi: IDXGIDevice = d3d_device.cast()?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? };
        let device: IDirect3DDevice = inspectable.cast()?;

        // Build a capture item from the window handle via the interop factory.
        let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
        let size = item.Size()?;

        // Free-threaded pool → we can `TryGetNextFrame` from our own loop thread
        // without a DispatcherQueue. BGRA8 matches the rest of the BGRA→NV12 path.
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            FRAME_POOL_BUFFERS,
            size,
        )?;
        let session = frame_pool.CreateCaptureSession(&item)?;
        // Hide the yellow capture border where the OS allows it (build 20348+).
        // Best-effort: older builds don't expose the property.
        let _ = session.SetIsBorderRequired(false);

        // Stop cleanly if the target window closes (the item raises `Closed`).
        let closed_token = item.Closed(&TypedEventHandler::new(
            move |_item: windows::core::Ref<GraphicsCaptureItem>, _| {
                tracing::info!("wgc: capture item closed (window gone)");
                Ok(())
            },
        ))?;

        session.StartCapture()?;
        tracing::info!(
            width = size.Width,
            height = size.Height,
            "wgc: capture started"
        );

        Ok(WgcCapture {
            item,
            device,
            frame_pool,
            session,
            last_frame: None,
            last_size: size,
            closed_token,
        })
    }

    /// Sample the latest frame: copy-target texture + its 100-ns `SystemRelativeTime`.
    /// Returns `Ok(None)` when no new frame is queued yet (the caller paces to fps,
    /// same as the hook source loop). The returned texture is owned by the WGC
    /// frame and recycled on the *next* `acquire`, so the caller must copy it out
    /// immediately (`CopySubresourceRegion` into a staging texture) before then.
    pub fn acquire(&mut self) -> WinResult<Option<(ID3D11Texture2D, i64)>> {
        // Close the previously-held frame now that the caller has copied it.
        if let Some(prev) = self.last_frame.take() {
            let _ = prev.Close();
        }

        let frame = match self.frame_pool.TryGetNextFrame() {
            Ok(f) => f,
            // An empty pool surfaces as an error in some builds — treat as "no
            // frame yet" rather than a hard failure.
            Err(_) => return Ok(None),
        };

        // Recreate the pool if the window resized (WGC requires it), then skip this
        // frame — the next `acquire` will get a correctly-sized one.
        let content = frame.ContentSize()?;
        if content.Width != self.last_size.Width || content.Height != self.last_size.Height {
            self.last_size = content;
            self.frame_pool.Recreate(
                &self.device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                FRAME_POOL_BUFFERS,
                content,
            )?;
            let _ = frame.Close();
            return Ok(None);
        }

        // Pull the underlying ID3D11Texture2D out of the frame's surface.
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let tex: ID3D11Texture2D = unsafe { access.GetInterface()? };
        let ts = frame.SystemRelativeTime()?.Duration;

        // Keep the frame alive until the next acquire so `tex` stays valid for the
        // caller's copy; then hand back the texture + timestamp.
        self.last_frame = Some(frame);
        Ok(Some((tex, ts)))
    }

    /// The capture item's current size (window client size, in pixels).
    pub fn size(&self) -> WinResult<SizeInt32> {
        self.item.Size()
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        if let Some(prev) = self.last_frame.take() {
            let _ = prev.Close();
        }
        let _ = self.item.RemoveClosed(self.closed_token);
        // Closing the pool + session releases the WGC resources. Best-effort.
        let _ = self.session.Close();
        let _ = self.frame_pool.Close();
    }
}

/// Whether WGC is available on this OS (Windows 10 1903+ / build 18362). Callers
/// use this to decide if the WGC backend is even an option before trying `start`.
pub fn is_supported() -> bool {
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}
