//! GPU scheduling-priority boost for the capture process.
//!
//! Under a fullscreen game the Windows GPU scheduler tends to deprioritize a
//! background recorder's GPU work (our BGRA→NV12 convert + NVENC/QSV encode)
//! behind the game's render queue — which is exactly the FPS-cliff we're chasing.
//! Raising this process's *GPU* scheduling-priority class (the GPU analogue of a
//! CPU priority class) keeps that capture/convert/encode work from starving.
//!
//! This mirrors what Medal (`GPUSchedulingUtility.cs`) and OBS
//! (`libobs-d3d11: Set maximum GPU priority`, commit
//! `ec769ef008b748f7dfba211daec9eb203ea4bea0`) both do:
//!   1. Enable `SeIncreaseBasePriorityPrivilege` (best-effort; usually denied when
//!      non-elevated, in which case we simply achieve a lower class).
//!   2. Call the undocumented-but-stable `D3DKMTSetProcessSchedulingPriorityClass`
//!      (gdi32.dll, resolved dynamically — not in windows-rs bindings), stepping
//!      down the priority enum until one sticks.
//!   3. Call `IDXGIDevice::SetGPUThreadPriority(1)` on the capture device.
//!
//! HAGS (Hardware-Accelerated GPU Scheduling) changes the safe ceiling: with HAGS
//! on we cap at HIGH(4); with it off we use REALTIME(5). Intel adapters are left
//! untouched (conservative per-vendor clamp).
//!
//! Everything here is strictly best-effort — a failure never affects capture; it
//! only means we didn't get the boost. Success/failure is reported on a single
//! grep-able `gpu_priority:` info line.

#![allow(non_snake_case, non_camel_case_types)]

use std::ffi::c_void;

use windows::core::{s, w, Interface, PCSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, SE_INC_BASE_PRIORITY_NAME, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

// ── D3DKMT_SCHEDULINGPRIORITYCLASS ───────────────────────────────────────────
const CLASS_ABOVE_NORMAL: u32 = 3;
const CLASS_HIGH: u32 = 4;
const CLASS_REALTIME: u32 = 5;

const VENDOR_INTEL: u32 = 0x8086;
const VENDOR_NVIDIA: u32 = 0x10DE;
const VENDOR_AMD: u32 = 0x1002;

/// `KMTQAITYPE_WDDM_2_7_CAPS`
const KMTQAITYPE_WDDM_2_7_CAPS: u32 = 70;

// ── Dynamically-resolved gdi32 entry points (not in windows-rs bindings) ──────
type D3DKmtSetProcessSchedulingPriorityClass =
    unsafe extern "system" fn(handle: HANDLE, priority: u32) -> i32; // NTSTATUS
type D3DKmtOpenAdapterFromLuid =
    unsafe extern "system" fn(arg: *mut D3DKMT_OPENADAPTERFROMLUID) -> i32;
type D3DKmtQueryAdapterInfo = unsafe extern "system" fn(arg: *mut D3DKMT_QUERYADAPTERINFO) -> i32;
type D3DKmtCloseAdapter = unsafe extern "system" fn(arg: *const D3DKMT_CLOSEADAPTER) -> i32;

#[repr(C)]
struct D3DKMT_OPENADAPTERFROMLUID {
    AdapterLuid: LUID,
    hAdapter: u32,
}

#[repr(C)]
struct D3DKMT_QUERYADAPTERINFO {
    hAdapter: u32,
    Type: u32,
    pPrivateDriverData: *mut c_void,
    PrivateDriverDataSize: u32,
}

#[repr(C)]
struct D3DKMT_CLOSEADAPTER {
    hAdapter: u32,
}

/// Resolve a gdi32 export by name, transmuted to `T` (an `unsafe extern "system"`
/// fn type). `None` if gdi32 isn't loaded or the symbol is missing.
///
/// # Safety
/// `T` must be the exact ABI-correct signature of the named export.
unsafe fn resolve_gdi32<T: Copy>(name: PCSTR) -> Option<T> {
    debug_assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<usize>(),
        "T must be a plain fn pointer"
    );
    let gdi32 = GetModuleHandleW(w!("gdi32.dll")).ok()?;
    let addr = GetProcAddress(gdi32, name)?;
    Some(std::mem::transmute_copy::<_, T>(&addr))
}

/// Best-effort: raise this process's GPU scheduling priority for the capture
/// adapter. Never fails the caller — logs the outcome at info level.
pub(crate) fn raise_gpu_priority(device: &ID3D11Device) {
    let (luid, vendor_id) = match adapter_luid_and_vendor(device) {
        Some(v) => v,
        None => {
            tracing::info!("gpu_priority: could not read adapter LUID/vendor — skipping boost");
            return;
        }
    };
    let vendor = vendor_name(vendor_id);

    // Vendor gate: leave Intel adapters untouched (conservative per-vendor clamp;
    // Medal keeps a per-vendor max and clamps Intel low).
    if vendor_id == VENDOR_INTEL {
        tracing::info!("gpu_priority: vendor=intel — not raising GPU priority (per-vendor clamp)");
        return;
    }

    // HAGS on (or unknown) → cap at HIGH; HAGS off → REALTIME.
    let (hags, hags_src) = detect_hags(luid);
    let target = match hags {
        Some(false) => CLASS_REALTIME,
        _ => CLASS_HIGH, // Some(true) or None (assume enabled — conservative)
    };
    let hags_state = match hags {
        Some(true) => "on",
        Some(false) => "off",
        None => "unknown",
    };

    // Best-effort privilege elevation — the priority call may still succeed at a
    // lower class without it.
    enable_base_priority_privilege();

    let set_class: Option<D3DKmtSetProcessSchedulingPriorityClass> =
        unsafe { resolve_gdi32(s!("D3DKMTSetProcessSchedulingPriorityClass")) };
    let Some(set_class) = set_class else {
        tracing::info!(
            "gpu_priority: vendor={vendor} hags={hags_state}({hags_src}) \
             result=unavailable (D3DKMTSetProcessSchedulingPriorityClass not found)"
        );
        return;
    };

    // Fallback ladder: try the target class, step down toward ABOVE_NORMAL until
    // one sticks. STATUS_SUCCESS == 0.
    let mut achieved: Option<u32> = None;
    let mut last_status: i32 = 0;
    let proc = unsafe { GetCurrentProcess() };
    let mut class = target;
    loop {
        let status = unsafe { set_class(proc, class) };
        if status == 0 {
            achieved = Some(class);
            break;
        }
        last_status = status;
        if class <= CLASS_ABOVE_NORMAL {
            break;
        }
        class -= 1;
    }

    match achieved {
        Some(class) => {
            // Second half of the fix: bump the DXGI device's GPU thread priority.
            let thread_pri = set_gpu_thread_priority(device);
            tracing::info!(
                "gpu_priority: vendor={vendor} hags={hags_state}({hags_src}) \
                 class={class} thread_priority={thread_pri} result=ok"
            );
        }
        None => {
            tracing::info!(
                "gpu_priority: vendor={vendor} hags={hags_state}({hags_src}) \
                 class={target} result=failed last_status={last_status:#x} (not admin?)"
            );
        }
    }
}

/// Read the capture device's adapter LUID + PCI vendor id via DXGI.
fn adapter_luid_and_vendor(device: &ID3D11Device) -> Option<(LUID, u32)> {
    let dxgi: IDXGIDevice = device.cast().ok()?;
    let adapter = unsafe { dxgi.GetAdapter() }.ok()?;
    let desc = unsafe { adapter.GetDesc() }.ok()?;
    Some((desc.AdapterLuid, desc.VendorId))
}

fn vendor_name(vendor_id: u32) -> &'static str {
    match vendor_id {
        VENDOR_NVIDIA => "nvidia",
        VENDOR_AMD => "amd",
        VENDOR_INTEL => "intel",
        _ => "unknown",
    }
}

/// Bump `IDXGIDevice::SetGPUThreadPriority(1)` on the capture device (Medal/OBS do
/// the same). Returns a short label for the log line.
fn set_gpu_thread_priority(device: &ID3D11Device) -> &'static str {
    let Ok(dxgi) = device.cast::<IDXGIDevice>() else {
        return "n/a";
    };
    // Valid range is -7..=7; 1 matches Medal.
    match unsafe { dxgi.SetGPUThreadPriority(1) } {
        Ok(()) => "1",
        Err(_) => "err",
    }
}

/// Detect whether HAGS (Hardware-Accelerated GPU Scheduling) is enabled for
/// `luid`. Returns `(state, source)` where state is `Some(true|false)` or `None`
/// (unknown → callers treat as enabled). Primary path is the D3DKMT WDDM 2.7 caps
/// query; on failure returns `None` (the registry fallback is intentionally
/// omitted — the D3DKMT path works on every Win10 1909+ machine).
fn detect_hags(luid: LUID) -> (Option<bool>, &'static str) {
    match query_wddm_2_7_caps(luid) {
        Some(caps) => {
            // bit 1 = HwSchEnabled.
            (Some((caps & 0b10) != 0), "d3dkmt")
        }
        None => (None, "unknown"),
    }
}

/// Open the adapter by LUID, query its WDDM 2.7 caps bitfield, and close it. All
/// three D3DKMT entry points are resolved from gdi32 dynamically.
fn query_wddm_2_7_caps(luid: LUID) -> Option<u32> {
    let open: D3DKmtOpenAdapterFromLuid =
        unsafe { resolve_gdi32(s!("D3DKMTOpenAdapterFromLuid"))? };
    let query: D3DKmtQueryAdapterInfo = unsafe { resolve_gdi32(s!("D3DKMTQueryAdapterInfo"))? };
    let close: D3DKmtCloseAdapter = unsafe { resolve_gdi32(s!("D3DKMTCloseAdapter"))? };

    let mut open_arg = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: luid,
        hAdapter: 0,
    };
    if unsafe { open(&mut open_arg) } != 0 {
        return None;
    }
    let h_adapter = open_arg.hAdapter;

    let mut caps: u32 = 0;
    let mut query_arg = D3DKMT_QUERYADAPTERINFO {
        hAdapter: h_adapter,
        Type: KMTQAITYPE_WDDM_2_7_CAPS,
        pPrivateDriverData: &mut caps as *mut u32 as *mut c_void,
        PrivateDriverDataSize: std::mem::size_of::<u32>() as u32,
    };
    let status = unsafe { query(&mut query_arg) };

    let close_arg = D3DKMT_CLOSEADAPTER {
        hAdapter: h_adapter,
    };
    unsafe {
        let _ = close(&close_arg);
    }

    if status != 0 {
        return None;
    }
    Some(caps)
}

/// Enable `SeIncreaseBasePriorityPrivilege` on the current process token.
/// Best-effort: usually denied when non-elevated — logged at debug, never fatal.
fn enable_base_priority_privilege() {
    let result = (|| -> windows::core::Result<()> {
        let mut token = HANDLE::default();
        unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )?;
        }
        // Ensure the token handle is closed regardless of outcome.
        let _guard = HandleGuard(token);

        let mut luid = LUID::default();
        unsafe {
            LookupPrivilegeValueW(None, SE_INC_BASE_PRIORITY_NAME, &mut luid)?;
        }

        let privs = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [windows::Win32::Security::LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        unsafe {
            AdjustTokenPrivileges(token, false, Some(&privs), 0, None, None)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        tracing::debug!("gpu_priority: could not enable SeIncreaseBasePriorityPrivilege: {e}");
    }
}

/// RAII closer for a token `HANDLE`.
struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
