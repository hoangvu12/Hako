//! Shared D3D11 device + DXGI adapter enumeration.
//!
//! One device is shared across capture, video-processor, and encoder so the
//! whole pipeline stays zero-copy on the GPU. Vendor is identified via DXGI
//! `VendorId` and the matching FFmpeg encoder is selected accordingly.

#![allow(dead_code)]

use serde::Serialize;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, DXGI_ADAPTER_FLAG,
    DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
};

/// GPU vendor identified by DXGI `VendorId` (PCI vendor IDs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_id(vendor_id: u32) -> Self {
        match vendor_id {
            0x10DE => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "Other",
        }
    }

    /// Default FFmpeg H.264 hardware encoder for this vendor.
    pub fn default_h264_encoder(self) -> Option<&'static str> {
        match self {
            Vendor::Nvidia => Some("h264_nvenc"),
            Vendor::Amd => Some("h264_amf"),
            Vendor::Intel => Some("h264_qsv"),
            Vendor::Other => None,
        }
    }
}

/// A DXGI adapter we could capture/encode on. Serialized to the UI.
#[derive(Debug, Clone, Serialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub vendor: Vendor,
    pub vendor_label: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_vram_mb: u64,
    pub is_software: bool,
    /// FFmpeg encoder we'd pick for this adapter (None for software/unknown).
    pub encoder: Option<String>,
    /// True for the adapter we'd default to for ENCODING (largest VRAM).
    pub preferred: bool,
    /// True if this adapter drives a display output (composites windows).
    /// WGC capture is cheapest on the display-owning adapter (avoids
    /// cross-adapter copies, iGPU/dGPU mismatch).
    pub drives_display: bool,
}

/// Enumerate all hardware DXGI adapters and mark the preferred one.
///
/// Preference: highest dedicated VRAM (the discrete GPU that most likely owns
/// the game's swapchain), with NVIDIA/AMD ranked above an Intel iGPU on ties.
pub fn enumerate_gpus() -> Result<Vec<GpuInfo>> {
    let mut gpus = Vec::new();

    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;

        let mut i = 0u32;
        loop {
            let adapter: IDXGIAdapter1 = match factory.EnumAdapters1(i) {
                Ok(a) => a,
                Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                Err(e) => return Err(e),
            };

            let desc = adapter.GetDesc1()?;

            let is_software =
                (DXGI_ADAPTER_FLAG(desc.Flags as i32).0 & DXGI_ADAPTER_FLAG_SOFTWARE.0) != 0;
            let vendor = Vendor::from_id(desc.VendorId);
            // An adapter with at least one DXGI output drives a display.
            let drives_display = adapter.EnumOutputs(0).is_ok();

            gpus.push(GpuInfo {
                index: i,
                name: utf16_to_string(&desc.Description),
                vendor,
                vendor_label: vendor.label().to_string(),
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                dedicated_vram_mb: (desc.DedicatedVideoMemory as u64) / (1024 * 1024),
                is_software,
                encoder: vendor.default_h264_encoder().map(str::to_string),
                preferred: false,
                drives_display,
            });

            i += 1;
        }
    }

    if let Some(idx) = pick_preferred(&gpus) {
        gpus[idx].preferred = true;
    }

    Ok(gpus)
}

/// Index of the adapter we'd default to: hardware, ranked by VRAM then vendor.
fn pick_preferred(gpus: &[GpuInfo]) -> Option<usize> {
    fn vendor_rank(v: Vendor) -> u8 {
        match v {
            Vendor::Nvidia | Vendor::Amd => 2, // discrete-class encoders
            Vendor::Intel => 1,
            Vendor::Other => 0,
        }
    }

    gpus.iter()
        .enumerate()
        .filter(|(_, g)| !g.is_software && g.encoder.is_some())
        .max_by(|(_, a), (_, b)| {
            a.dedicated_vram_mb
                .cmp(&b.dedicated_vram_mb)
                .then(vendor_rank(a.vendor).cmp(&vendor_rank(b.vendor)))
        })
        .map(|(i, _)| i)
}

/// Adapter to default WGC capture to: the display-owning one (avoids
/// per-frame cross-adapter copies). Falls back to the preferred encode
/// adapter, then the default adapter.
pub fn default_capture_index(gpus: &[GpuInfo]) -> Option<u32> {
    gpus.iter()
        .find(|g| g.drives_display && !g.is_software)
        .or_else(|| gpus.iter().find(|g| g.preferred))
        .map(|g| g.index)
}

/// Create a D3D11 device on a specific adapter (`None` = default adapter).
///
/// Flags: `BGRA_SUPPORT` (WGC delivers BGRA) + `VIDEO_SUPPORT`
/// (ID3D11VideoProcessor color convert). This is the device the whole pipeline
/// shares so there are no cross-device copies.
pub fn create_device(
    adapter: Option<&IDXGIAdapter1>,
) -> Result<(ID3D11Device, ID3D11DeviceContext, D3D_FEATURE_LEVEL)> {
    let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let mut feature_level = D3D_FEATURE_LEVEL_11_0;

    // IDXGIAdapter1 must be widened to its base IDXGIAdapter for D3D11CreateDevice.
    let base: Option<IDXGIAdapter> = match adapter {
        Some(a) => Some(a.cast()?),
        None => None,
    };

    unsafe {
        D3D11CreateDevice(
            base.as_ref(),
            // When an explicit adapter is passed, driver type must be UNKNOWN.
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            flags,
            Some(&feature_levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
    }

    Ok((
        device.expect("D3D11CreateDevice returned null device"),
        context.expect("D3D11CreateDevice returned null context"),
        feature_level,
    ))
}

/// Look up an adapter by its enumeration index (for capturing on a chosen GPU).
pub fn adapter_at(index: u32) -> Result<IDXGIAdapter1> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        factory.EnumAdapters1(index)
    }
}

fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
