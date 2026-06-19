//! Host-side orchestration of the OBS `graphics-hook` (the "Game Capture" path).
//!
//! This is the Rust counterpart to OBS `plugins/win-capture/game-capture.c`. It
//! drives the per-session lifecycle described in [`super`] "Host lifecycle"
//! against the byte-level [`contract`]: resolve the target, hold a keepalive
//! mutex, stand up the log pipe, inject the vendored hook DLL, open the objects
//! the DLL creates, write the capture config into the shared `HookInfo`, and then
//! hand the shared backbuffer texture back each frame.
//!
//! ## What is implemented here
//! - Steps 1–9 (`HookCapture::start`): target resolution → keepalive → pipe →
//!   inject (safe SetWindowsHookEx path) → open DLL objects → write config →
//!   `SetEvent(Initialize)`.
//! - Step 10–11 intake (`RunningHook::acquire`): poll `HookReady`, read the
//!   capture type, `OpenSharedResource` the shtex handle, return the live
//!   `ID3D11Texture2D` + a wall-clock (QPC, 100 ns) timestamp so the caller can
//!   copy it into its own pooled texture and feed `Converter`/`Encoder` exactly
//!   like the WGC path's `(ID3D11Texture2D, ts)` hand-off.
//! - Step 12 teardown (`Drop`/`stop`): drop keepalive + `SetEvent(Stop)`, unmap,
//!   close handles.
//!
//! The shmem (`CAPTURE_TYPE_MEMORY`) fallback is stubbed — Valorant (D3D11) uses
//! the shared-texture path, which is what we validate first.

#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D};
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Threading::{
    CreateMutexW, OpenEventW, OpenMutexW, ReleaseMutex, SetEvent, WaitForSingleObject,
    SYNCHRONIZATION_ACCESS_RIGHTS,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

use super::contract::{
    self, GraphicsOffsets, HookInfo, ShtexData, CAPTURE_TYPE_MEMORY, CAPTURE_TYPE_TEXTURE,
};

// ---- Win32 access-right constants (kept local so we don't depend on which
// windows-rs feature module re-exports each one) ------------------------------

const SYNCHRONIZE: u32 = 0x0010_0000;
const EVENT_MODIFY_STATE: u32 = 0x0002;
/// `WAIT_OBJECT_0` — a 0-timeout wait returns this when the object is signaled.
const WAIT_OBJECT_0: u32 = 0x0000_0000;

/// How long to wait for the injected DLL to create its IPC objects before giving
/// up (the loader maps the DLL asynchronously via the user32 hook).
const OPEN_OBJECTS_TIMEOUT: Duration = Duration::from_secs(6);
const OPEN_OBJECTS_POLL: Duration = Duration::from_millis(50);

// =============================================================================
// RAII handle wrappers
// =============================================================================

/// Owns a Win32 `HANDLE`, closing it on drop. Construction rejects null /
/// invalid handles so callers can treat a successful build as "valid".
struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(h: HANDLE, what: &str) -> Result<Self, String> {
        if h.is_invalid() {
            Err(format!("{what}: received an invalid handle"))
        } else {
            Ok(OwnedHandle(h))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    /// Signal an event handle (`SetEvent`). Best-effort.
    fn set_event(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    /// 0-timeout wait: `true` when the (auto-reset) object is currently signaled.
    fn is_signaled(&self) -> bool {
        unsafe { WaitForSingleObject(self.0, 0).0 == WAIT_OBJECT_0 }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// A mapped view of a file mapping; unmaps on drop. `ptr` points at the shared
/// `HookInfo` (or a texture-data block).
struct MappedView {
    addr: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl MappedView {
    fn ptr(&self) -> *mut c_void {
        self.addr.Value
    }
}

impl Drop for MappedView {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.addr);
        }
    }
}

// =============================================================================
// Small Win32 helpers
// =============================================================================

/// UTF-16, null-terminated, for the `PCWSTR`-taking Win32 APIs. Keep the returned
/// `Vec` alive for as long as the `PCWSTR` is in use.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `QueryPerformanceCounter` normalized to 100-ns ticks — the same unit and
/// epoch family as WGC `SystemRelativeTime`, so hook-path PTS lines up with the
/// rest of the pipeline (`MasterClock` only needs a monotonic 100-ns tick).
fn qpc_100ns() -> i64 {
    unsafe {
        let mut freq = 0i64;
        let mut count = 0i64;
        if QueryPerformanceFrequency(&mut freq).is_err() || freq == 0 {
            return 0;
        }
        let _ = QueryPerformanceCounter(&mut count);
        // count / freq seconds → ×1e7 for 100-ns ticks; do the ×1e7 first (i128)
        // to avoid truncating sub-second precision.
        ((count as i128 * contract_ticks_per_second() as i128) / freq as i128) as i64
    }
}

#[inline]
fn contract_ticks_per_second() -> i64 {
    crate::core::clock::TICKS_PER_SECOND
}

/// Open an auto-reset event the hook created, by per-pid name.
fn open_event(base: &str, pid: u32) -> Result<OwnedHandle, String> {
    let name = contract::name_with_pid(base, pid);
    let w = wide(&name);
    let access = SYNCHRONIZATION_ACCESS_RIGHTS(SYNCHRONIZE | EVENT_MODIFY_STATE);
    let h = unsafe { OpenEventW(access, false, PCWSTR(w.as_ptr())) }
        .map_err(|e| format!("OpenEventW({name}): {e}"))?;
    OwnedHandle::new(h, &name)
}

/// Open a mutex the hook created, by per-pid name.
fn open_mutex(base: &str, pid: u32) -> Result<OwnedHandle, String> {
    let name = contract::name_with_pid(base, pid);
    let w = wide(&name);
    let access = SYNCHRONIZATION_ACCESS_RIGHTS(SYNCHRONIZE);
    let h = unsafe { OpenMutexW(access, false, PCWSTR(w.as_ptr())) }
        .map_err(|e| format!("OpenMutexW({name}): {e}"))?;
    OwnedHandle::new(h, &name)
}

/// Open the `CaptureHook_HookInfo<pid>` mapping and map a writable view over it.
fn open_hook_info(pid: u32) -> Result<(OwnedHandle, MappedView), String> {
    let name = contract::name_with_pid(contract::SHMEM_HOOK_INFO, pid);
    let w = wide(&name);
    let mapping = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(w.as_ptr())) }
        .map_err(|e| format!("OpenFileMappingW({name}): {e}"))?;
    let mapping = OwnedHandle::new(mapping, &name)?;
    let addr = unsafe {
        MapViewOfFile(
            mapping.raw(),
            FILE_MAP_ALL_ACCESS,
            0,
            0,
            std::mem::size_of::<HookInfo>(),
        )
    };
    if addr.Value.is_null() {
        return Err(format!("MapViewOfFile({name}) returned null"));
    }
    Ok((mapping, MappedView { addr }))
}

/// Open a named texture-data mapping (`CaptureHook_Texture_<hwnd>_<map_id>`) and
/// map `size` bytes of it read-only-enough for us to read `ShtexData`.
fn open_texture_mapping(hwnd: u64, map_id: u32, size: usize) -> Result<(OwnedHandle, MappedView), String> {
    let name = contract::texture_mapping_name(hwnd, map_id);
    let w = wide(&name);
    let mapping = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(w.as_ptr())) }
        .map_err(|e| format!("OpenFileMappingW({name}): {e}"))?;
    let mapping = OwnedHandle::new(mapping, &name)?;
    let addr = unsafe { MapViewOfFile(mapping.raw(), FILE_MAP_ALL_ACCESS, 0, 0, size.max(4)) };
    if addr.Value.is_null() {
        return Err(format!("MapViewOfFile({name}) returned null"));
    }
    Ok((mapping, MappedView { addr }))
}

// =============================================================================
// Vendored binaries
// =============================================================================

/// Set by the Tauri layer at startup to the bundled resource directory
/// (`<resource_dir>/vendor/obs-hook`). Packaged builds resolve their resource dir
/// via Tauri's path API (it isn't always the exe dir), so this override is the
/// authoritative location; the env/exe/dev fallbacks below cover dev + tests.
static HOOK_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Point the hook loader at the directory holding the vendored OBS binaries.
/// Called once from `main`'s `setup` with the resolved Tauri resource dir.
pub fn set_vendor_hook_dir(dir: PathBuf) {
    let _ = HOOK_DIR_OVERRIDE.set(dir);
}

/// Locate the vendored OBS hook binaries (`graphics-hook64.dll`,
/// `inject-helper64.exe`, `get-graphics-offsets64.exe`).
///
/// Resolution order: explicit override set by the app (`set_vendor_hook_dir`) →
/// `HAKO_OBS_HOOK_DIR` env → `<exe dir>/vendor/obs-hook` → in-repo
/// `src-tauri/vendor/obs-hook` (dev/tests).
fn vendor_hook_dir() -> Result<PathBuf, String> {
    if let Some(p) = HOOK_DIR_OVERRIDE.get() {
        if p.is_dir() {
            return Ok(p.clone());
        }
    }
    if let Ok(dir) = std::env::var("HAKO_OBS_HOOK_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("vendor").join("obs-hook");
            if p.is_dir() {
                return Ok(p);
            }
        }
    }
    // Dev fallback: repo layout relative to this crate.
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join("obs-hook");
    if dev.is_dir() {
        return Ok(dev);
    }
    Err("could not locate vendored OBS hook binaries (set HAKO_OBS_HOOK_DIR)".into())
}

/// Root for per-game copies of the hook DLL: `%LOCALAPPDATA%\Hako\HookDLL`
/// (falling back to the system temp dir). See [`prepare_hook_dll_copy`].
fn hook_dll_root() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Hako")
        .join("HookDLL")
}

/// Copy `graphics-hook64.dll` out of the (read-only, installed) vendor `dir` into
/// a per-PID subdir under [`hook_dll_root`], and return the copy's path.
///
/// Mirrors Medal's `HookLibraryManager.GetDLLForTarget` (per-`MedalHook_<pid>`
/// copy): the game `LoadLibrary`s — and therefore holds a write lock on — only
/// this throwaway copy, never the installed resource. That's the whole point:
/// without it, a game injected in a prior session keeps the installed
/// `graphics-hook64.dll` locked, and the NSIS auto-updater fails to overwrite it
/// ("Error opening file for writing"). OBS injects straight from its install dir
/// and hits exactly that bug; Medal sidesteps it with this copy. Stale copies are
/// reaped by [`cleanup_stale_hook_dll_copies`] at startup.
///
/// An existing same-size copy is reused (so a re-hook of the same PID doesn't
/// re-copy — and doesn't try to overwrite a copy the game already has locked).
fn prepare_hook_dll_copy(dir: &Path, pid: u32) -> Result<PathBuf, String> {
    let src = dir.join("graphics-hook64.dll");
    let src_len = std::fs::metadata(&src)
        .map_err(|e| format!("missing {}: {e}", src.display()))?
        .len();

    let dst_dir = hook_dll_root().join(format!("HakoHook_{pid}"));
    let dst = dst_dir.join("graphics-hook64.dll");

    let up_to_date = src_len != 0
        && std::fs::metadata(&dst).map(|m| m.len()).ok() == Some(src_len);
    if !up_to_date {
        std::fs::create_dir_all(&dst_dir)
            .map_err(|e| format!("create hook copy dir {}: {e}", dst_dir.display()))?;
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("copy hook DLL to {}: {e}", dst.display()))?;
    }
    Ok(dst)
}

/// Delete per-PID hook DLL copies left behind by games that have since exited
/// (e.g. after a crash or normal shutdown). Mirrors Medal's
/// `HookTracker.CleanUpOldHookDLLs`, called once at startup: a copy whose PID is
/// still running is left alone (the game has it locked); the rest are removed so
/// `HookDLL` doesn't grow without bound. Best-effort — failures are logged only.
pub fn cleanup_stale_hook_dll_copies() {
    let root = hook_dll_root();
    let Ok(entries) = std::fs::read_dir(&root) else {
        return; // nothing copied yet
    };
    // Only need process existence by PID, so refresh the process list with no
    // per-process detail (names/PIDs come from the base enumeration) rather than
    // `new_all()`, which also snapshots CPU/memory/disks/networks.
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(pid) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("HakoHook_"))
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if sys.process(sysinfo::Pid::from_u32(pid)).is_some() {
            continue; // game still running — its copy is locked, leave it
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::debug!("could not remove stale hook copy {}: {e}", path.display()),
        }
    }
    if removed > 0 {
        tracing::info!("cleaned up {removed} stale hook DLL copy dir(s)");
    }
}

/// Run `get-graphics-offsets64.exe` and parse its INI-style stdout into a
/// [`GraphicsOffsets`] (module-relative function offsets the hook will Detour).
///
/// Output looks like `[dxgi]\npresent=...\nresize=...\npresent1=...` with one
/// section per graphics API. Values may be decimal or `0x`-prefixed hex.
fn get_graphics_offsets(dir: &std::path::Path) -> Result<GraphicsOffsets, String> {
    let exe = dir.join("get-graphics-offsets64.exe");
    let out = Command::new(&exe)
        .output()
        .map_err(|e| format!("run {}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!(
            "get-graphics-offsets64 exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut offsets = GraphicsOffsets::default();
    let mut section = String::new();
    let parse = |v: &str| -> Option<u32> {
        let v = v.trim();
        if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()
        } else {
            v.parse::<u32>().ok()
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        let Some((k, val)) = line.split_once('=') else {
            continue;
        };
        let Some(n) = parse(val) else { continue };
        match (section.as_str(), k.trim()) {
            ("d3d8", "present") => offsets.d3d8.present = n,
            ("d3d9", "present") => offsets.d3d9.present = n,
            ("d3d9", "present_ex") => offsets.d3d9.present_ex = n,
            ("d3d9", "present_swap") => offsets.d3d9.present_swap = n,
            ("d3d9", "d3d9_clsoff") => offsets.d3d9.d3d9_clsoff = n,
            ("d3d9", "is_d3d9ex_clsoff") => offsets.d3d9.is_d3d9ex_clsoff = n,
            ("dxgi", "present") => offsets.dxgi.present = n,
            ("dxgi", "resize") => offsets.dxgi.resize = n,
            ("dxgi", "present1") => offsets.dxgi.present1 = n,
            ("dxgi2", "release") => offsets.dxgi2.release = n,
            ("d3d12", "execute_command_lists") => offsets.d3d12.execute_command_lists = n,
            _ => {}
        }
    }
    if offsets.dxgi.present == 0 {
        return Err("get-graphics-offsets produced no dxgi.present offset".into());
    }
    Ok(offsets)
}

// =============================================================================
// HookCapture / RunningHook
// =============================================================================

/// Entry point for the game-capture path. `start` performs lifecycle steps 1–9
/// and returns a [`RunningHook`] from which frames can be acquired.
pub struct HookCapture;

impl HookCapture {
    /// Inject the hook into the window's process and bring the IPC up to the
    /// point where frames are flowing. `fps` sets the hook's `frame_interval`
    /// (its capture cap); `0` means "every present".
    ///
    /// ⚠️ Injects into the target process. For Valorant this MUST use the safe
    /// (`SetWindowsHookEx`) injection path — see [`inject`]. Ship opt-in only.
    pub fn start(hwnd: HWND, fps: u32) -> Result<RunningHook, String> {
        // ── Step 1: resolve target thread + process id ──────────────────────
        let mut pid = 0u32;
        let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread_id == 0 || pid == 0 {
            return Err("could not resolve target window's thread/process".into());
        }
        if pid == std::process::id() {
            return Err("refusing to inject into Hako itself".into());
        }

        let dir = vendor_hook_dir()?;

        // ── Step 2: keepalive mutex (held for the whole session) ────────────
        // The hook opens this; when it can no longer open it (we exited / dropped
        // it), the DLL self-destructs. We CREATE + own it.
        let keepalive_name = contract::name_with_pid(contract::WINDOW_HOOK_KEEPALIVE, pid);
        let kw = wide(&keepalive_name);
        let keepalive = unsafe { CreateMutexW(None, true, PCWSTR(kw.as_ptr())) }
            .map_err(|e| format!("CreateMutexW({keepalive_name}): {e}"))?;
        let keepalive = OwnedHandle::new(keepalive, &keepalive_name)?;

        // ── Step 3: log pipe server ─────────────────────────────────────────
        let pipe = HookPipe::start(pid);

        tracing::info!(pid, thread_id, dir = %dir.display(), "graphics-hook: resolved target, injecting");

        // ── Step 4/5: re-hook if already injected, else inject ──────────────
        // If a restart event already exists the hook is resident from a previous
        // session — signal it to re-hook instead of injecting again.
        if let Ok(restart) = open_event(contract::EVENT_CAPTURE_RESTART, pid) {
            restart.set_event();
            tracing::info!(pid, "graphics-hook already present; signaled restart");
        } else {
            // Inject a per-PID copy of the DLL, not the installed resource, so a
            // game holding the hook never locks the file the updater must replace.
            let hook_dll = prepare_hook_dll_copy(&dir, pid)?;
            inject(&hook_dll, &dir, thread_id)?;
            tracing::info!(pid, "inject-helper returned OK; waiting for hook to create IPC objects");
        }

        // ── Step 6: open the objects the DLL creates (with retry) ───────────
        let objs = open_dll_objects(pid)?;
        tracing::info!(pid, "graphics-hook: IPC objects opened (DLL loaded)");

        // ── Step 7: write capture config into the shared HookInfo ───────────
        let offsets = get_graphics_offsets(&dir)?;
        tracing::info!(
            pid,
            dxgi_present = offsets.dxgi.present,
            dxgi_resize = offsets.dxgi.resize,
            "graphics-hook: computed graphics offsets"
        );
        // SAFETY: `info_ptr` is a live, writable view of a zero-initialized
        // HookInfo-sized mapping (we map exactly size_of::<HookInfo>()).
        let info_ptr = objs.info_view.ptr() as *mut HookInfo;
        // Mirror OBS `init_hook_info`: write ONLY these fields. `hook_ver_*`,
        // `type`, `cx`/`cy`, `format`, `map_id`/`map_size` are written by the
        // hook — we read those back later (see `acquire`).
        unsafe {
            let info = &mut *info_ptr;
            info.offsets = offsets;
            info.frame_interval = if fps == 0 {
                0
            } else {
                1_000_000_000 / fps as u64
            };
            info.force_shmem = false; // prefer the zero-copy shared-texture path
            info.capture_overlay = false;
            info.allow_srgb_alias = true;
            info.unused_use_scale = false;
        }

        // ── Step 8: tell the hook to initialize ─────────────────────────────
        objs.init.set_event();

        tracing::info!(pid, thread_id, fps, "graphics-hook initialized; awaiting frames");

        Ok(RunningHook {
            hwnd,
            pid,
            fps,
            _keepalive: keepalive,
            stop: objs.stop,
            restart: objs.restart,
            ready: objs.ready,
            _exit: objs.exit,
            _init: objs.init,
            _tex_mutex1: objs.tex_mutex1,
            _tex_mutex2: objs.tex_mutex2,
            _info_mapping: objs.info_mapping,
            info_view: objs.info_view,
            _pipe: pipe,
            shared_tex: None,
            shared_handle: 0,
        })
    }
}

/// The bundle of DLL-created IPC objects the host opens in step 6.
struct DllObjects {
    stop: OwnedHandle,
    /// The hook's own restart event — signaling it makes the resident DLL re-run
    /// `capture_init_shtex` (re-hook the current swapchain) without re-injecting.
    restart: OwnedHandle,
    ready: OwnedHandle,
    exit: OwnedHandle,
    init: OwnedHandle,
    tex_mutex1: OwnedHandle,
    tex_mutex2: OwnedHandle,
    info_mapping: OwnedHandle,
    info_view: MappedView,
}

/// Poll until the injected DLL has created all of its IPC objects (or time out).
fn open_dll_objects(pid: u32) -> Result<DllObjects, String> {
    let deadline = Instant::now() + OPEN_OBJECTS_TIMEOUT;
    #[allow(unused_assignments)]
    let mut last_err = String::new();
    loop {
        match try_open_dll_objects(pid) {
            Ok(o) => return Ok(o),
            Err(e) => last_err = e,
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "graphics-hook never came up (pid {pid}): {last_err}. \
                 Likely the DLL failed to load (anti-cheat block) or a hook \
                 version mismatch — see contract::HOOK_VER_MAJOR."
            ));
        }
        std::thread::sleep(OPEN_OBJECTS_POLL);
    }
}

fn try_open_dll_objects(pid: u32) -> Result<DllObjects, String> {
    let init = open_event(contract::EVENT_HOOK_INIT, pid)?;
    let ready = open_event(contract::EVENT_HOOK_READY, pid)?;
    let stop = open_event(contract::EVENT_CAPTURE_STOP, pid)?;
    let restart = open_event(contract::EVENT_CAPTURE_RESTART, pid)?;
    let exit = open_event(contract::EVENT_HOOK_EXIT, pid)?;
    let tex_mutex1 = open_mutex(contract::MUTEX_TEXTURE1, pid)?;
    let tex_mutex2 = open_mutex(contract::MUTEX_TEXTURE2, pid)?;
    let (info_mapping, info_view) = open_hook_info(pid)?;
    Ok(DllObjects {
        stop,
        restart,
        ready,
        exit,
        init,
        tex_mutex1,
        tex_mutex2,
        info_mapping,
        info_view,
    })
}

/// Spawn `inject-helper64.exe "<dll>" <anti_cheat> <id>`.
///
/// We always use the **safe** path (`anti_cheat = 1`, `id = thread_id`): the
/// helper `SetWindowsHookEx(WH_GETMESSAGE)`s the DLL onto the target UI thread
/// and nudges it with `PostThreadMessage`, so the OS loader maps the DLL — no
/// `OpenProcess`/`WriteProcessMemory`/`CreateRemoteThread` against the game.
/// This is what OBS/Medal use for anti-cheated titles like Valorant.
///
/// `dll` is the hook DLL to load into the game — a per-PID copy from
/// [`prepare_hook_dll_copy`], not the installed resource. `helper_dir` is the
/// vendor dir holding `inject-helper64.exe` (the helper just runs and exits, so
/// it isn't subject to the same lock as the loaded DLL).
fn inject(dll: &Path, helper_dir: &Path, thread_id: u32) -> Result<(), String> {
    let helper = helper_dir.join("inject-helper64.exe");
    if !dll.exists() {
        return Err(format!("missing {}", dll.display()));
    }
    if !helper.exists() {
        return Err(format!("missing {}", helper.display()));
    }
    let status = Command::new(&helper)
        .arg(dll.as_os_str())
        .arg("1") // anti_cheat = safe SetWindowsHookEx path
        .arg(thread_id.to_string())
        .status()
        .map_err(|e| format!("spawn inject-helper64: {e}"))?;
    // The helper returns a small status code; OBS treats non-zero as failure.
    if !status.success() {
        return Err(format!(
            "inject-helper64 failed (code {:?}) — DLL may have been blocked",
            status.code()
        ));
    }
    Ok(())
}

/// A live game-capture session. Frames are pulled with [`RunningHook::acquire`];
/// dropping (or [`RunningHook::stop`]) tears the hook down.
pub struct RunningHook {
    hwnd: HWND,
    pid: u32,
    fps: u32,
    /// Held for the session; dropping it tells the hook to self-destruct.
    _keepalive: OwnedHandle,
    stop: OwnedHandle,
    /// The hook's restart event — `request_restart()` signals it to re-hook the
    /// current swapchain (recovery from a stale-texture freeze, Part B).
    restart: OwnedHandle,
    ready: OwnedHandle,
    _exit: OwnedHandle,
    _init: OwnedHandle,
    _tex_mutex1: OwnedHandle,
    _tex_mutex2: OwnedHandle,
    _info_mapping: OwnedHandle,
    info_view: MappedView,
    _pipe: HookPipe,
    /// The opened shared backbuffer texture (shtex path), cached across frames.
    shared_tex: Option<ID3D11Texture2D>,
    /// The shtex handle currently open, so we can detect a swap/resize and reopen.
    shared_handle: u32,
}

// SAFETY: `RunningHook` is moved to a single dedicated frame-source thread and
// used only there. Its non-`Send` member is `info_view` (a raw mapped-view
// pointer); the mapping stays valid for the struct's whole lifetime and is never
// shared, so single-threaded ownership transfer is sound. All other members
// (windows-rs `HANDLE`/COM interfaces) are already `Send`.
unsafe impl Send for RunningHook {}

impl RunningHook {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Current capture dimensions the hook reported (0×0 until the first frame).
    pub fn dimensions(&self) -> (u32, u32) {
        let info = unsafe { &*(self.info_view.ptr() as *const HookInfo) };
        (info.cx, info.cy)
    }

    /// DXGI format the hook reported for the backbuffer (0 until first frame).
    pub fn format(&self) -> u32 {
        let info = unsafe { &*(self.info_view.ptr() as *const HookInfo) };
        info.format
    }

    /// Sample the current shared backbuffer: return the live shared texture plus a
    /// 100-ns wall-clock timestamp. The texture is owned by the hook and
    /// overwritten on the game's next present, so the caller must copy it into its
    /// own surface immediately (`CopySubresourceRegion` into a staging texture)
    /// before handing it to the encoder.
    ///
    /// IMPORTANT (OBS semantics): `HookReady` is **not** a per-frame event — the
    /// hook only `SetEvent`s it inside `capture_init_shtex` (i.e. on capture
    /// (re)init, e.g. resize). So we use it solely to (re)open the shared texture;
    /// between inits the hook keeps copying each present into that same texture and
    /// the host samples it continuously. The CALLER paces to the target fps —
    /// calling this in a tight loop just resamples the same backbuffer.
    ///
    /// Returns `Ok(None)` until the hook has initialized capture at least once.
    /// `device` is Hako's shared D3D11 device — the same one feeding the encoder.
    pub fn acquire(
        &mut self,
        device: &ID3D11Device,
    ) -> Result<Option<(ID3D11Texture2D, i64)>, String> {
        let info = unsafe { &*(self.info_view.ptr() as *const HookInfo) };
        // OBS `start_capture`: refuse a DLL whose hook ABI is newer than ours.
        if info.hook_ver_major > contract::HOOK_VER_MAJOR {
            return Err(format!(
                "vendored graphics-hook is ABI v{}.{}, newer than supported v{}.{} \
                 — update contract::HOOK_VER_* / re-vendor",
                info.hook_ver_major,
                info.hook_ver_minor,
                contract::HOOK_VER_MAJOR,
                contract::HOOK_VER_MINOR
            ));
        }

        // `HookReady` fires once per capture (re)init — use it ONLY to (re)open the
        // shared texture. Between inits, keep sampling the texture we already have.
        if self.ready.is_signaled() {
            match info.type_ {
                CAPTURE_TYPE_TEXTURE => {
                    let map_id = info.map_id;
                    let map_size = info.map_size as usize;
                    // Best-effort: if the mapping isn't fully populated yet, keep
                    // any texture we already had and retry on the next tick.
                    if let Err(e) = self.shared_texture_for(device, map_id, map_size) {
                        if self.shared_tex.is_none() {
                            tracing::debug!("hook: shtex not ready on init signal: {e}");
                            return Ok(None);
                        }
                    }
                }
                CAPTURE_TYPE_MEMORY => {
                    // shmem fallback (double-buffered CPU mapping under the texture
                    // mutexes). Not needed for D3D11/Valorant; wire later.
                    return Err("shmem (CAPTURE_TYPE_MEMORY) path not implemented — \
                                expected the shared-texture path for D3D11"
                        .into());
                }
                other => return Err(format!("unknown capture type {other}")),
            }
        }

        match &self.shared_tex {
            Some(tex) => Ok(Some((tex.clone(), qpc_100ns()))),
            None => Ok(None), // capture not initialized yet
        }
    }

    /// Open (or reuse) the shared backbuffer texture for the current `map_id`.
    fn shared_texture_for(
        &mut self,
        device: &ID3D11Device,
        map_id: u32,
        map_size: usize,
    ) -> Result<ID3D11Texture2D, String> {
        // Read the current shtex handle from the texture-data mapping.
        let (_mapping, view) =
            open_texture_mapping(self.hwnd.0 as u64, map_id, map_size.max(std::mem::size_of::<ShtexData>()))?;
        let shtex = unsafe { *(view.ptr() as *const ShtexData) };
        let handle = shtex.tex_handle;
        if handle == 0 {
            return Err("shtex handle is null".into());
        }

        // Reopen only when the handle changes (resize / device reset).
        if self.shared_tex.is_none() || handle != self.shared_handle {
            // Legacy shared handle: widen the 32-bit value to a HANDLE. The
            // game-capture path uses D3D11_RESOURCE_MISC_SHARED (no keyed mutex),
            // so OpenSharedResource gives a directly-sampleable texture.
            let raw = HANDLE(handle as usize as *mut c_void);
            let mut opened: Option<ID3D11Texture2D> = None;
            unsafe {
                device
                    .OpenSharedResource(raw, &mut opened)
                    .map_err(|e| format!("OpenSharedResource(shtex {handle:#x}): {e}"))?;
            }
            let tex = opened.ok_or("OpenSharedResource returned null texture")?;
            self.shared_tex = Some(tex);
            self.shared_handle = handle;
            tracing::info!(pid = self.pid, handle, "opened shared backbuffer texture");
        }

        Ok(self.shared_tex.as_ref().expect("shared_tex set above").clone())
    }

    /// Step 12: stop the hook (drop keepalive happens via `Drop`).
    pub fn stop(&mut self) {
        self.stop.set_event();
    }

    /// Ask the resident hook to re-hook the current swapchain (OBS restart path):
    /// it re-runs `capture_init_shtex` and re-signals `HookReady`, so the next
    /// `acquire()` reopens the (possibly new) shared texture. No re-injection —
    /// safe under anti-cheat. Used to recover from a stale-texture freeze after a
    /// fullscreen↔borderless switch (Part B static watchdog).
    pub fn request_restart(&self) {
        self.restart.set_event();
    }
}

impl Drop for RunningHook {
    fn drop(&mut self) {
        // Signal stop, then dropping `_keepalive` releases the mutex so the hook
        // self-terminates. Order: stop first so the hook stops presenting frames
        // into a texture we're about to stop reading.
        self.stop.set_event();
        unsafe {
            // The keepalive mutex must be released before it's closed.
            let _ = ReleaseMutex(self._keepalive.raw());
        }
    }
}

// =============================================================================
// Log pipe (hook → host, null-terminated strings only)
// =============================================================================

/// The named-pipe *server* for `\\.\pipe\CaptureHook_Pipe<pid>`. The hook
/// connects as a client and writes null-terminated log lines; we surface them
/// via `tracing`. Best-effort — capture works even if the pipe never connects.
struct HookPipe {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HookPipe {
    fn start(pid: u32) -> HookPipe {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("hako-hook-pipe".into())
                .spawn(move || pipe_server_loop(pid, stop))
                .ok()
        };
        HookPipe { stop, thread }
    }
}

impl Drop for HookPipe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            // The server thread is almost certainly blocked in a *synchronous*
            // `ConnectNamedPipe`/`ReadFile` (it only re-checks `stop` between
            // ops), so a plain join would hang forever when the hook never
            // connected. Cancel its in-flight sync I/O so it unblocks, sees
            // `stop`, and exits — then join is safe.
            unsafe {
                use std::os::windows::io::AsRawHandle;
                let h = HANDLE(t.as_raw_handle());
                let _ = CancelSynchronousIo(h);
            }
            let _ = t.join();
        }
    }
}

fn pipe_server_loop(pid: u32, stop: Arc<AtomicBool>) {
    use windows::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE,
        PIPE_TYPE_MESSAGE, PIPE_WAIT,
    };

    let path = contract::pipe_path(pid);
    let wpath = wide(&path);
    // 0x00080000 = PIPE_REJECT_REMOTE_CLIENTS-free; modest buffers, 1 instance.
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(wpath.as_ptr()),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            1,
            0,
            4096,
            0,
            None,
        )
    };
    let pipe = match OwnedHandle::new(pipe, "CreateNamedPipeW") {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("hook log pipe unavailable: {e}");
            return;
        }
    };

    while !stop.load(Ordering::Acquire) {
        // Blocking connect; the hook may take a moment. We re-check `stop` after.
        let connected = unsafe { ConnectNamedPipe(pipe.raw(), None) };
        if connected.is_err() {
            // ERROR_PIPE_CONNECTED is success-ish; other errors → retry briefly.
            std::thread::sleep(Duration::from_millis(100));
            if stop.load(Ordering::Acquire) {
                break;
            }
        }

        let mut buf = [0u8; 4096];
        loop {
            if stop.load(Ordering::Acquire) {
                break;
            }
            let mut read = 0u32;
            let ok = unsafe { ReadFile(pipe.raw(), Some(&mut buf), Some(&mut read), None) };
            if ok.is_err() || read == 0 {
                break; // client disconnected
            }
            let msg = String::from_utf8_lossy(&buf[..read as usize]);
            for line in msg.split('\0').filter(|s| !s.is_empty()) {
                tracing::debug!(target: "graphics_hook", "{line}");
            }
        }
        unsafe {
            let _ = DisconnectNamedPipe(pipe.raw());
        }
    }
}
