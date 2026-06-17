//! Shared D3D11 device + DXGI adapter enumeration.
//!
//! One device is shared across capture, video-processor, and encoder so the
//! whole pipeline stays zero-copy on the GPU. Vendor is identified via DXGI
//! `VendorId` and the matching FFmpeg encoder is selected accordingly.

#![allow(dead_code)]

use serde::Serialize;
use windows::core::{Interface, Result, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1, IDXGIResource1,
    DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_NOT_FOUND,
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

    /// Whether [`crate::core::encode::Encoder`] actually implements a hardware
    /// encode backend for this vendor. NVENC (NVIDIA) and QSV (Intel) are wired
    /// up; AMD (AMF) is not yet, and `Other` has no path — so although
    /// [`Self::default_h264_encoder`] *names* `h264_amf` for the UI, an AMD
    /// adapter must not be resolved as a cross-adapter encode target (it would
    /// dead-end at `Encoder::new`). See [`resolve_adapters`].
    pub fn has_hw_encode(self) -> bool {
        matches!(self, Vendor::Nvidia | Vendor::Intel)
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

/// Vendor of the adapter at DXGI `index` in `gpus` ([`Vendor::Other`] if absent).
pub fn vendor_at(gpus: &[GpuInfo], index: u32) -> Vendor {
    gpus.iter()
        .find(|g| g.index == index)
        .map(|g| g.vendor)
        .unwrap_or(Vendor::Other)
}

// ---------------------------------------------------------------------------
// Cross-adapter encode (Medal-style discrete-GPU NVENC) — adapter resolution +
// capability probe. See docs/cross-adapter-encode-plan.md. Phase 0: the
// resolution is pure/testable and the probe is the runtime safety net; nothing
// here changes the pipeline yet (the encode device still equals the capture
// device until the shared hand-off lands in a later phase).
// ---------------------------------------------------------------------------

/// A resolved (capture, encode) adapter pair for one capture session.
///
/// WGC capture must run on the **display-owning** adapter (the frame pool binds
/// to the GPU that composites the desktop), but the *encoder* may be asked to run
/// on a different adapter — e.g. a discrete NVENC GPU on a hybrid laptop whose
/// panel is driven by the Intel iGPU. When [`Self::cross`] is true the pipeline
/// needs a cross-adapter NV12 copy (capture GPU → encode GPU); when false the
/// whole pipeline is single-device and zero-copy (today's fast path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterPlan {
    /// DXGI index of the WGC capture adapter (the display owner).
    pub capture_idx: u32,
    /// DXGI index of the adapter the encoder runs on.
    pub encode_idx: u32,
    /// True when `encode_idx != capture_idx` (a cross-adapter transfer is needed).
    pub cross: bool,
}

/// Resolve the (capture, encode) adapter pair from the enumerated adapters and
/// the user's "Selected GPU" choice (`requested_encode`: `None` = Auto, else a
/// DXGI adapter index from `Settings::gpu_adapter`).
///
/// - **Capture** is always the display-owning adapter (WGC requirement), with the
///   same fallbacks as [`default_capture_index`].
/// - **Encode** is the display owner for Auto (keeping the zero-copy fast path —
///   we never silently cross adapters), or the explicitly chosen adapter. A
///   chosen adapter that doesn't exist, is software, or has no *implemented* HW
///   encoder ([`Vendor::has_hw_encode`] — so AMD/AMF and `Other` are excluded) is
///   rejected back to the capture adapter (logged), rather than cross-adapter
///   into a dead end.
///
/// Returns `None` only when no usable capture adapter exists at all (no
/// non-software adapter present).
pub fn resolve_adapters(gpus: &[GpuInfo], requested_encode: Option<u32>) -> Option<AdapterPlan> {
    let capture_idx = default_capture_index(gpus)?;

    let encode_idx = match requested_encode {
        // Auto: keep the encoder on the display owner — zero-copy, no surprise
        // cross-adapter transfer. (A future `gpu_auto = "discrete"` mode would
        // pick `pick_preferred` here for Medal parity.)
        None => capture_idx,
        // Selecting the capture adapter is honored as-is (it's whatever WGC
        // captures on, encoder support aside).
        Some(req) if req == capture_idx => capture_idx,
        Some(req) => match gpus.iter().find(|g| g.index == req) {
            Some(g) if !g.is_software && g.vendor.has_hw_encode() => req,
            Some(g) => {
                tracing::warn!(
                    requested = req,
                    vendor = g.vendor.label(),
                    "selected encode GPU has no implemented hardware encoder; \
                     falling back to the capture adapter"
                );
                capture_idx
            }
            None => {
                tracing::warn!(
                    requested = req,
                    "selected encode GPU index is not present; falling back to the \
                     capture adapter"
                );
                capture_idx
            }
        },
    };

    Some(AdapterPlan {
        capture_idx,
        encode_idx,
        cross: encode_idx != capture_idx,
    })
}

/// Outcome of the cross-adapter capability probe (Phase 0). `ok` is whether a
/// shared keyed-mutex NV12 texture round-trips from the capture device to the
/// encode device; `reason` explains a failure so the UI can say why the discrete
/// encoder wasn't used (the caller falls back to the single-device path).
#[derive(Debug, Clone, Serialize)]
pub struct CrossAdapterProbe {
    pub ok: bool,
    pub reason: Option<String>,
}

// CreateSharedHandle access rights for a shared resource (the windows crate does
// not surface these as named constants here). Read + write so either device can
// use the keyed mutex. Values from dxgi.h.
const DXGI_SHARED_RESOURCE_READ: u32 = 0x8000_0000;
const DXGI_SHARED_RESOURCE_WRITE: u32 = 0x0000_0001;

/// Probe whether the cross-adapter NV12 hand-off is viable for `plan`:
/// 1. both adapters create a D3D11 device, and
/// 2. a 64×64 NV12 texture with `SHARED_NTHANDLE | SHARED_KEYEDMUTEX` created on
///    the capture device can be opened on the encode device (the NT-handle path,
///    `IDXGIResource1::CreateSharedHandle` → `ID3D11Device1::OpenSharedResource1`).
///
/// A non-cross plan (encode == capture) is trivially OK — the single-device fast
/// path needs no sharing. On any failure returns `ok: false` with a reason so the
/// caller can fall back to the display-owner single-device path. This is a pure
/// capability check: it creates and immediately drops throwaway devices/textures
/// and touches no running pipeline.
pub fn probe_cross_adapter(plan: &AdapterPlan) -> CrossAdapterProbe {
    if !plan.cross {
        return CrossAdapterProbe {
            ok: true,
            reason: None,
        };
    }
    match probe_shared_nv12(plan.capture_idx, plan.encode_idx) {
        Ok(()) => CrossAdapterProbe {
            ok: true,
            reason: None,
        },
        Err(e) => CrossAdapterProbe {
            ok: false,
            reason: Some(e),
        },
    }
}

/// The actual create-on-capture / open-on-encode round-trip behind
/// [`probe_cross_adapter`]. Returns an explanatory string on the first failure.
fn probe_shared_nv12(capture_idx: u32, encode_idx: u32) -> std::result::Result<(), String> {
    unsafe {
        let cap_adapter =
            adapter_at(capture_idx).map_err(|e| format!("capture adapter {capture_idx}: {e}"))?;
        let (cap_dev, _cap_ctx, _) =
            create_device(Some(&cap_adapter)).map_err(|e| format!("create capture device: {e}"))?;
        let enc_adapter =
            adapter_at(encode_idx).map_err(|e| format!("encode adapter {encode_idx}: {e}"))?;
        let (enc_dev, _enc_ctx, _) =
            create_device(Some(&enc_adapter)).map_err(|e| format!("create encode device: {e}"))?;

        // 64×64 NV12 shared keyed-mutex NT-handle texture on the capture device,
        // matching the real hand-off texture's flags (RENDER_TARGET = the
        // VideoProcessorBlt output target in convert.rs).
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 64,
            Height: 64,
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
            MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        cap_dev
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .map_err(|e| format!("create shared NV12 on capture device: {e}"))?;
        let tex = tex.ok_or("CreateTexture2D returned null shared NV12 texture")?;

        let resource: IDXGIResource1 = tex
            .cast()
            .map_err(|e| format!("query IDXGIResource1: {e}"))?;
        let handle = resource
            .CreateSharedHandle(
                None,
                DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE,
                PCWSTR::null(),
            )
            .map_err(|e| format!("CreateSharedHandle: {e}"))?;

        // Open it on the encode device — the load-bearing capability.
        let enc_dev1: ID3D11Device1 = enc_dev
            .cast()
            .map_err(|e| format!("query ID3D11Device1 on encode device: {e}"))?;
        let opened: Result<ID3D11Texture2D> = enc_dev1.OpenSharedResource1(handle);
        // The NT handle is owned by us; close it whether or not the open succeeded.
        let _ = CloseHandle(handle);
        opened.map_err(|e| format!("OpenSharedResource1 on encode device: {e}"))?;
        Ok(())
    }
}

fn utf16_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic adapter for the resolution tests (no DXGI involved).
    fn gpu(index: u32, vendor: Vendor, vram_mb: u64, drives_display: bool) -> GpuInfo {
        GpuInfo {
            index,
            name: format!("{} #{index}", vendor.label()),
            vendor,
            vendor_label: vendor.label().to_string(),
            vendor_id: 0,
            device_id: 0,
            dedicated_vram_mb: vram_mb,
            is_software: false,
            encoder: vendor.default_h264_encoder().map(str::to_string),
            preferred: false,
            drives_display,
        }
    }

    /// `enumerate_gpus` marks the highest-VRAM HW-encoder adapter `preferred`;
    /// mirror that for tests that rely on the capture fallback chain.
    fn mark_preferred(mut gpus: Vec<GpuInfo>) -> Vec<GpuInfo> {
        if let Some(i) = pick_preferred(&gpus) {
            gpus[i].preferred = true;
        }
        gpus
    }

    #[test]
    fn single_gpu_auto_is_not_cross() {
        // One NVIDIA GPU that drives the display: Auto encodes on it, no copy.
        let gpus = mark_preferred(vec![gpu(0, Vendor::Nvidia, 8192, true)]);
        let plan = resolve_adapters(&gpus, None).expect("plan");
        assert_eq!(plan.capture_idx, 0);
        assert_eq!(plan.encode_idx, 0);
        assert!(!plan.cross);
    }

    #[test]
    fn hybrid_auto_keeps_display_owner() {
        // Optimus laptop: Intel iGPU drives the panel (idx 0), NVIDIA dGPU is the
        // higher-VRAM preferred encoder (idx 1). Auto must NOT silently cross to
        // NVENC — it stays on the display owner (Intel QSV, zero-copy).
        let gpus = mark_preferred(vec![
            gpu(0, Vendor::Intel, 512, true),
            gpu(1, Vendor::Nvidia, 8192, false),
        ]);
        let plan = resolve_adapters(&gpus, None).expect("plan");
        assert_eq!(plan.capture_idx, 0);
        assert_eq!(plan.encode_idx, 0);
        assert!(!plan.cross);
    }

    #[test]
    fn hybrid_explicit_discrete_is_cross() {
        // Same laptop, but the user explicitly selected the NVIDIA dGPU (idx 1):
        // capture stays on the iGPU, encode moves to the dGPU → cross-adapter.
        let gpus = mark_preferred(vec![
            gpu(0, Vendor::Intel, 512, true),
            gpu(1, Vendor::Nvidia, 8192, false),
        ]);
        let plan = resolve_adapters(&gpus, Some(1)).expect("plan");
        assert_eq!(plan.capture_idx, 0);
        assert_eq!(plan.encode_idx, 1);
        assert!(plan.cross);
    }

    #[test]
    fn hybrid_explicit_display_owner_is_not_cross() {
        // Explicitly selecting the display-owning adapter is the zero-copy path.
        let gpus = mark_preferred(vec![
            gpu(0, Vendor::Intel, 512, true),
            gpu(1, Vendor::Nvidia, 8192, false),
        ]);
        let plan = resolve_adapters(&gpus, Some(0)).expect("plan");
        assert_eq!(plan.encode_idx, 0);
        assert!(!plan.cross);
    }

    #[test]
    fn explicit_out_of_range_falls_back_to_capture() {
        let gpus = mark_preferred(vec![
            gpu(0, Vendor::Intel, 512, true),
            gpu(1, Vendor::Nvidia, 8192, false),
        ]);
        let plan = resolve_adapters(&gpus, Some(7)).expect("plan");
        assert_eq!(plan.encode_idx, plan.capture_idx);
        assert!(!plan.cross);
    }

    #[test]
    fn explicit_amd_encode_is_rejected_to_capture() {
        // AMD names h264_amf for the UI but AMF encode isn't implemented, so it
        // must not be resolved as a cross-adapter target (would dead-end at
        // Encoder::new). Falls back to the display owner.
        let gpus = mark_preferred(vec![
            gpu(0, Vendor::Intel, 512, true),
            gpu(1, Vendor::Amd, 8192, false),
        ]);
        let plan = resolve_adapters(&gpus, Some(1)).expect("plan");
        assert_eq!(plan.encode_idx, 0);
        assert!(!plan.cross);
    }

    #[test]
    fn explicit_software_encode_is_rejected_to_capture() {
        let mut gpus = mark_preferred(vec![
            gpu(0, Vendor::Nvidia, 8192, true),
            gpu(1, Vendor::Intel, 512, false),
        ]);
        gpus[1].is_software = true;
        let plan = resolve_adapters(&gpus, Some(1)).expect("plan");
        assert_eq!(plan.encode_idx, 0);
        assert!(!plan.cross);
    }

    #[test]
    fn no_usable_adapter_yields_none() {
        assert!(resolve_adapters(&[], None).is_none());
    }

    #[test]
    fn non_cross_probe_is_trivially_ok() {
        // A non-cross plan never touches DXGI — safe to assert in CI.
        let plan = AdapterPlan {
            capture_idx: 0,
            encode_idx: 0,
            cross: false,
        };
        let probe = probe_cross_adapter(&plan);
        assert!(probe.ok);
        assert!(probe.reason.is_none());
    }
}
