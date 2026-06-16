//! Graphics-hook ("Game Capture") path — capture *above* the DWM composition
//! cap by hooking the game's own `Present`, the way OBS and Medal do it.
//!
//! Hako's default capture is WGC (`core::capture`), which is Vanguard-safe but
//! bound by the desktop composition rate (~60 on mixed-refresh / multi-GPU
//! setups). This module is the opt-in high-FPS path that injects an
//! OBS-derived `graphics-hook` DLL into the game, grabs the real backbuffer at
//! the game's render rate, and shares it back as a D3D11 texture — which we then
//! run through the same `Converter` → `Encoder` pipeline as WGC.
//!
//! ## Why this mirrors OBS exactly
//! Medal's `medal-hook64.dll` is OBS's `graphics-hook` recompiled (same exports,
//! `MedalCaptureHook_Pipe`). We reuse OBS's GPL hook DLL + `inject-helper` +
//! `get-graphics-offsets` binaries (vendored under `src-tauri/vendor/obs-hook/`)
//! and implement only the host orchestration here. See [`contract`] for the
//! byte-level IPC contract.
//!
//! ## ⚠️ Anti-cheat risk
//! Injecting into the Valorant process is tolerated for OBS/Medal because Riot
//! trusts those specific signed binaries. A new injector is **not** automatically
//! trusted and can be blocked or put users' accounts at risk. This path must ship
//! **opt-in, defaulted off, behind a clear warning**, and WGC stays the default.
//!
//! ## Host lifecycle (implemented step-by-step against [`contract`])
//! 1. Resolve target: window → `GetWindowThreadProcessId` → (thread_id,
//!    process_id). Reject self / blacklisted processes.
//! 2. Create keepalive mutex `CaptureHook_KeepAlive<pid>` (held for the session;
//!    the DLL self-destructs when it can no longer open it).
//! 3. Start the named-pipe *server* `\\.\pipe\CaptureHook_Pipe<pid>` (hook logs).
//! 4. If `CaptureHook_Restart<pid>` already exists → `SetEvent` (re-hook) and
//!    skip injection; else inject (step 5).
//! 5. Inject: spawn `inject-helper64.exe "<hook.dll>" <anti_cheat:0|1> <id>`,
//!    where `id` = thread_id when anti-cheat (safe SetWindowsHookEx path), else
//!    process_id (full CreateRemoteThread path). Anti-cheat ON for Valorant.
//! 6. Open (created by the DLL): the 5 events, both texture mutexes, and the
//!    `CaptureHook_HookInfo<pid>` mapping.
//! 7. Write config into the hook-info mapping: `offsets` (from
//!    get-graphics-offsets, or hardcoded DXGI slots 8/13/22), `frame_interval`
//!    (from target fps; 0 = every frame), `force_shmem` (only if no shared-tex
//!    support), `capture_overlay=false`, `allow_srgb_alias=true`.
//! 8. `SetEvent(CaptureHook_Initialize<pid>)`.
//! 9. Poll `CaptureHook_HookReady<pid>` (0-timeout wait) each tick.
//! 10. Read `hook_info.type`:
//!     - `CAPTURE_TYPE_TEXTURE` → read `ShtexData.tex_handle`, widen to HANDLE,
//!       `OpenSharedResource` → an `ID3D11Texture2D` we sample each frame.
//!     - `CAPTURE_TYPE_MEMORY` → map `CaptureHook_Texture_<hwnd>_<map_id>`,
//!       double-buffered copy guarded by the two texture mutexes.
//! 11. Per frame: hand `(shared_texture, timestamp)` to the encode thread — the
//!     same handoff shape `core::capture` already feeds `Converter`/`Encoder`.
//! 12. Stop: drop keepalive + `SetEvent(CaptureHook_Stop<pid>)`; release shared
//!     resources.
//!
//! Build order: contract ✅ → vendor OBS hook
//! binaries → host orchestration (steps 1–9) → shtex intake (step 10–11) →
//! encode-thread reuse → settings toggle + warning → Valorant validation.

pub mod contract;
pub mod host;

pub use host::{HookCapture, RunningHook};
