//! IPC contract for OBS-style graphics-hook capture (the "Game Capture" path).
//!
//! Medal's recorder ships OBS's `graphics-hook` DLL (renamed) — its exports
//! (`capture_init_shtex`, `d3d11_shtex_capture`, `OBS_QueuePresentKHR`, …) and
//! its pipe name (`MedalCaptureHook_Pipe`) are OBS's `win-capture` verbatim. We
//! take the same route: reuse OBS's GPL hook DLL + `inject-helper` and implement
//! this host-side contract in Rust, then feed the shared backbuffer texture into
//! the existing `Converter`/`Encoder` pipeline.
//!
//! This module is the binary contract between the **host** (Hako, this process)
//! and the **injected DLL** (running inside the target game). Everything here is
//! transcribed from `obsproject/obs-studio`
//! `shared/obs-hook-config/graphics-hook-info.h` and must stay byte-compatible
//! with the hook DLL we ship.
//!
//! Who creates what (OBS semantics):
//! - The **DLL creates**: all 5 events, both texture mutexes, the hook-info file
//!   mapping, and the texture mapping. The host *opens* them.
//! - The **host creates**: only the keepalive mutex and the named-pipe server.
//!
//! Naming: every event/mutex/hook-info-mapping/pipe name is suffixed with the
//! **target game's process id** (`"{base}{pid}"`). The texture data mapping is
//! the one exception — it is keyed on the target window handle and a counter:
//! `"CaptureHook_Texture_{hwnd}_{map_id}"`.
//!
//! NOTE: these base names match OBS's *stock* hook DLL. If we later fork/recompile
//! the hook with Hako-specific strings (to avoid colliding with a user's running
//! OBS), rename on both sides together.

#![allow(dead_code)]

// ---- Base IPC names (OBS stock; wide except the pipe, which is ANSI) ---------

pub const EVENT_CAPTURE_RESTART: &str = "CaptureHook_Restart";
pub const EVENT_CAPTURE_STOP: &str = "CaptureHook_Stop";
pub const EVENT_HOOK_READY: &str = "CaptureHook_HookReady";
pub const EVENT_HOOK_EXIT: &str = "CaptureHook_Exit";
pub const EVENT_HOOK_INIT: &str = "CaptureHook_Initialize";
pub const WINDOW_HOOK_KEEPALIVE: &str = "CaptureHook_KeepAlive";
pub const MUTEX_TEXTURE1: &str = "CaptureHook_TextureMutex1";
pub const MUTEX_TEXTURE2: &str = "CaptureHook_TextureMutex2";
pub const SHMEM_HOOK_INFO: &str = "CaptureHook_HookInfo";
pub const SHMEM_TEXTURE: &str = "CaptureHook_Texture";
pub const PIPE_NAME: &str = "CaptureHook_Pipe";

/// `WM_USER + 432` — the message `inject-helper`'s safe (SetWindowsHookEx) path
/// posts to the target thread to nudge the loader into mapping the hook DLL.
pub const HOOK_WM_NUDGE: u32 = 0x0400 + 432;

/// Hook ABI version of the plugin side. The **hook DLL** writes its own version
/// into `HookInfo.hook_ver_*`; the host only *reads* it and refuses capture if
/// the DLL's major exceeds ours (OBS `game-capture.c`: `hook_ver_major >
/// HOOK_VER_MAJOR`). So the host must NOT write these fields.
///
/// Verified against the vendored OBS **32.1.0** hook (`graphics-hook-ver.h`:
/// `HOOK_VER_MAJOR 1`, `HOOK_VER_MINOR 8`, `HOOK_VER_PATCH 7`). Keep in sync if
/// the vendored binaries are bumped.
pub const HOOK_VER_MAJOR: u32 = 1;
pub const HOOK_VER_MINOR: u32 = 8;

// ---- Capture type (C `enum capture_type`, 4 bytes) ---------------------------

pub const CAPTURE_TYPE_MEMORY: u32 = 0;
pub const CAPTURE_TYPE_TEXTURE: u32 = 1;

// ---- Per-mapping payload structs --------------------------------------------

/// Texture (shtex) path payload — the DXGI *legacy* shared handle, truncated to
/// 32 bits (game-capture uses `D3D11_RESOURCE_MISC_SHARED`, not keyed-mutex, so
/// the host opens it with `OpenSharedResource` and there is no Acquire/Release).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ShtexData {
    pub tex_handle: u32,
}

/// Memory (shmem) fallback path payload — double-buffered; `last_tex` is the
/// index (0/1) the hook wrote most recently, guarded by the two texture mutexes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ShmemData {
    pub last_tex: i32,
    pub tex1_offset: u32,
    pub tex2_offset: u32,
}

/// vtable-slot offsets from each graphics module's base, computed once by
/// `get-graphics-offsets`. The host writes these into `HookInfo.offsets`; the
/// hook resolves real function addresses as `module_base + offset` and Detours
/// them (no import table / `GetProcAddress` on the hot path).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct D3d8Offsets {
    pub present: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct D3d9Offsets {
    pub present: u32,
    pub present_ex: u32,
    pub present_swap: u32,
    pub d3d9_clsoff: u32,
    pub is_d3d9ex_clsoff: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DxgiOffsets {
    /// `IDXGISwapChain::Present`  — vtable slot 8.
    pub present: u32,
    /// `IDXGISwapChain::ResizeBuffers` — vtable slot 13.
    pub resize: u32,
    /// `IDXGISwapChain1::Present1` — vtable slot 22.
    pub present1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DxgiOffsets2 {
    /// `IUnknown::Release` — vtable slot 2.
    pub release: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct DdrawOffsets {
    pub surface_create: u32,
    pub surface_restore: u32,
    pub surface_release: u32,
    pub surface_unlock: u32,
    pub surface_blt: u32,
    pub surface_flip: u32,
    pub surface_set_palette: u32,
    pub palette_set_entries: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct D3d12Offsets {
    /// `ID3D12CommandQueue::ExecuteCommandLists` — vtable slot 10.
    pub execute_command_lists: u32,
}

/// Mirror of OBS `struct graphics_offsets` (field order is load-bearing).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphicsOffsets {
    pub d3d8: D3d8Offsets,
    pub d3d9: D3d9Offsets,
    pub dxgi: DxgiOffsets,
    pub ddraw: DdrawOffsets,
    pub dxgi2: DxgiOffsets2,
    pub d3d12: D3d12Offsets,
}

/// Mirror of OBS `struct hook_info` (`#pragma pack(push, 8)`), the config block
/// in the `CaptureHook_HookInfo<pid>` mapping. The host *writes* `offsets`,
/// `frame_interval`, `force_shmem`, `capture_overlay`, `allow_srgb_alias`; the
/// hook *writes back* `type`, `window`, `format`, `cx`, `cy`, `pitch`, `map_id`,
/// `map_size`, `flip`. Plain `#[repr(C)]` reproduces the C layout exactly: the
/// `u64 frame_interval` forces 8-byte alignment (7 pad bytes after `flip`), and
/// the struct totals 648 bytes — asserted below.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct HookInfo {
    pub hook_ver_major: u32,
    pub hook_ver_minor: u32,

    pub type_: u32, // enum capture_type
    pub window: u32,
    pub format: u32,
    pub cx: u32,
    pub cy: u32,
    pub unused_base_cx: u32,
    pub unused_base_cy: u32,
    pub pitch: u32,
    pub map_id: u32,
    pub map_size: u32,
    pub flip: bool,

    pub frame_interval: u64, // ns; 0 = every frame
    pub unused_use_scale: bool,
    pub force_shmem: bool,
    pub capture_overlay: bool,
    pub allow_srgb_alias: bool,

    pub offsets: GraphicsOffsets,

    pub reserved: [u32; 126],
}

impl Default for HookInfo {
    fn default() -> Self {
        // SAFETY: HookInfo is a plain-old-data #[repr(C)] block whose all-zero
        // bit pattern is a valid value for every field (matches the C side,
        // which zero-initializes the mapping). No padding invariants to uphold.
        unsafe { core::mem::zeroed() }
    }
}

// Guard the ABI: if the layout ever drifts from OBS's 648-byte struct, fail to
// compile rather than silently corrupting the shared mapping.
const _: () = assert!(core::mem::size_of::<HookInfo>() == 648);
const _: () = assert!(core::mem::size_of::<GraphicsOffsets>() == 76);

// ---- Name builders -----------------------------------------------------------

/// Per-pid IPC object name, e.g. `event_name(EVENT_HOOK_READY, 4321)`
/// → `"CaptureHook_HookReady4321"`. Used for events, mutexes, and the hook-info
/// mapping (all suffixed with the *target game's* pid).
#[inline]
pub fn name_with_pid(base: &str, pid: u32) -> String {
    format!("{base}{pid}")
}

/// The texture data mapping name: keyed on the target window handle + map id,
/// e.g. `"CaptureHook_Texture_140298_1"`.
#[inline]
pub fn texture_mapping_name(hwnd: u64, map_id: u32) -> String {
    format!("{SHMEM_TEXTURE}_{hwnd}_{map_id}")
}

/// The full named-pipe path for the hook's log channel, e.g.
/// `r"\\.\pipe\CaptureHook_Pipe4321"`.
#[inline]
pub fn pipe_path(pid: u32) -> String {
    format!(r"\\.\pipe\{PIPE_NAME}{pid}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_sizes_match_obs() {
        // The const asserts above already gate compilation; mirror them as a
        // runtime check so the intent is visible in test output.
        assert_eq!(core::mem::size_of::<HookInfo>(), 648);
        assert_eq!(core::mem::size_of::<GraphicsOffsets>(), 76);
        assert_eq!(core::mem::size_of::<ShtexData>(), 4);
        assert_eq!(core::mem::size_of::<ShmemData>(), 12);
    }

    #[test]
    fn names_are_pid_suffixed() {
        assert_eq!(
            name_with_pid(EVENT_HOOK_READY, 4321),
            "CaptureHook_HookReady4321"
        );
        assert_eq!(
            texture_mapping_name(140298, 1),
            "CaptureHook_Texture_140298_1"
        );
        assert_eq!(pipe_path(4321), r"\\.\pipe\CaptureHook_Pipe4321");
    }
}
