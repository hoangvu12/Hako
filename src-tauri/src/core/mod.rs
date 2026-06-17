//! Performance-critical capture → encode → buffer pipeline.
//!
//! Everything here runs on dedicated native threads sharing a single D3D11
//! device.

#![allow(dead_code)]

use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
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

pub mod device; // shared D3D11 device + DXGI adapter enumeration
pub mod capture; // WGC: item, frame pool, FrameArrived → channel
pub mod hook; // OBS-style graphics-hook (Game Capture) — opt-in high-FPS path
pub mod convert; // ID3D11VideoProcessor BGRA → NV12/P010
pub mod encode; // FFmpeg hw device/frames ctx, encoder, packet out
pub mod audio; // WASAPI loopback + mic, resample, AAC
pub mod buffer; // RAM ring + IDR index
pub mod session; // Mode B full-match writer + timeline index
pub mod mux; // MP4 stream-copy clip writer, padding/merge
pub mod clock; // master clock (QPC), PTS mapping
