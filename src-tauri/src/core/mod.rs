//! Performance-critical capture → encode → buffer pipeline.
//!
//! Everything here runs on dedicated native threads sharing a single D3D11
//! device.

#![allow(dead_code)]

use std::ffi::c_void;

use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, ProcessPowerThrottling, SetProcessInformation,
    SetThreadInformation, SetThreadPriority, ThreadPowerThrottling,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_STATE, THREAD_POWER_THROTTLING_CURRENT_VERSION,
    THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
    THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_BELOW_NORMAL,
};

/// Raise the calling thread to ABOVE_NORMAL so a CPU-saturating game can't starve
/// a real-time capture/encode/audio thread under the Windows scheduler. These
/// threads are light on CPU (hardware encode; WASAPI drains) — this guards them
/// from starvation rather than hogging cores. Best-effort; only logs on failure.
pub(crate) fn boost_current_thread_priority(what: &str) {
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for this call only.
    if let Err(e) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) } {
        tracing::debug!("could not raise {what} thread priority: {e}");
    }
}

/// Tag the calling thread **HighQoS** (execution-speed power-throttling explicitly
/// disabled) so it is exempt from the process-level EcoQoS we set while hidden to
/// tray ([`set_process_eco_qos`]). Applied to the real-time recorder threads
/// (capture source, encode, audio) so throttling the *UI* process during gameplay
/// never parks the encode path on an efficiency core. A thread tagged HighQoS
/// overrides the process default; threads we don't tag merely follow the process
/// (fine for non-realtime work). Best-effort; only logs on failure.
pub(crate) fn protect_thread_high_qos(what: &str) {
    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        // ControlMask selects the mechanism; StateMask = 0 turns it OFF → HighQoS.
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    // SAFETY: GetCurrentThread is a pseudo-handle valid for this call; `state`
    // outlives the synchronous call and its size matches the struct.
    if let Err(e) = unsafe {
        SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            &state as *const _ as *const c_void,
            std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        )
    } {
        tracing::debug!("could not tag {what} thread HighQoS: {e}");
    }
}

/// Demote a non-essential background worker (thumbnail/filmstrip generation,
/// cleanup, etc.) so it yields CPU/cache resources to the game and the realtime
/// capture threads. Best-effort; safe to call from any worker thread.
pub(crate) fn throttle_current_thread_background(what: &str) {
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for this call only.
    if let Err(e) = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) } {
        tracing::debug!("could not lower {what} thread priority: {e}");
    }

    let state = THREAD_POWER_THROTTLING_STATE {
        Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
        // StateMask = EXECUTION_SPEED ON → EcoQoS for this thread.
        StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
    };
    // SAFETY: GetCurrentThread is a pseudo-handle valid for this call; `state`
    // outlives the synchronous call and its size matches the struct.
    if let Err(e) = unsafe {
        SetThreadInformation(
            GetCurrentThread(),
            ThreadPowerThrottling,
            &state as *const _ as *const c_void,
            std::mem::size_of::<THREAD_POWER_THROTTLING_STATE>() as u32,
        )
    } {
        tracing::debug!("could not tag {what} thread EcoQoS: {e}");
    }
}

/// Toggle process-wide **EcoQoS**. When hidden to tray (`enabled = true`) we mark
/// the whole process EcoQoS so the scheduler prefers efficiency cores / lower
/// clocks for the UI, WebView2, and async threads during gameplay — Windows only
/// *auto*-throttles a hidden window on battery, so a desktop gamer on AC gets
/// nothing without this. The real-time recorder threads opt out via
/// [`protect_thread_high_qos`], so the encode path keeps running at full speed. On
/// show (`enabled = false`) we clear the throttle back to HighQoS. Best-effort.
pub(crate) fn set_process_eco_qos(enabled: bool) {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if enabled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    // SAFETY: GetCurrentProcess is a pseudo-handle valid for this call; `state`
    // outlives the synchronous call and its size matches the struct.
    if let Err(e) = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const c_void,
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    } {
        tracing::debug!("could not set process EcoQoS={enabled}: {e}");
    }
}

pub mod audio; // WASAPI loopback + mic, resample, AAC
pub mod buffer; // RAM ring + IDR index
pub mod capture; // capture pipeline: hook source loop → channel → encode thread
pub mod clock;
pub mod convert; // ID3D11VideoProcessor BGRA → NV12/P010
pub mod cursor_overlay; // in-frame host-side mouse-cursor composite (Direct2D)
pub mod denoise; // offline mic noise suppression (DeepFilterNet 3), editor export only
pub mod device; // shared D3D11 device + DXGI adapter enumeration
pub mod disk_buffer; // disk-backed rolling segment ring (RAM-vs-disk buffer toggle)
pub mod encode; // FFmpeg hw device/frames ctx, encoder, packet out
pub mod gpu_priority; // best-effort GPU scheduling-priority boost (D3DKMT + DXGI)
pub mod hook; // OBS-style graphics-hook (Game Capture) — the capture backend
pub mod mux; // MP4 stream-copy clip writer, padding/merge
pub mod overlay_card; // in-frame "tabbed out" freeze card (Direct2D composite)
pub mod session; // Mode B full-match writer + timeline index
pub mod wgc; // Windows.Graphics.Capture source — robustness fallback (Part D) // master clock (QPC), PTS mapping
