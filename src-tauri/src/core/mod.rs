//! Performance-critical capture → encode → buffer pipeline.
//!
//! Everything here runs on dedicated native threads sharing a single D3D11
//! device.

#![allow(dead_code)]

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
