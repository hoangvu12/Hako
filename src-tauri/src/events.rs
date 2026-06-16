//! Typed event channel names emitted from the Rust core to the webview.
//!
//! Payloads are defined alongside their producers (e.g. [`crate::commands::RecorderStatus`]).
//! The frontend subscribes via `@tauri-apps/api/event` `listen(...)`.

// Several of these are placeholders for future producers.
#![allow(dead_code)]

/// Periodic recorder status / heartbeat. Payload: [`crate::commands::RecorderStatus`].
pub const RECORDER_STATUS: &str = "recorder-status";

/// Valorant match state transitions. Payload: TBD.
pub const MATCH_STATE_CHANGED: &str = "match-state-changed";

/// A new clip was written to the library. Payload: TBD.
pub const CLIP_CREATED: &str = "clip-created";

/// Live capture throughput. Payload: [`crate::core::capture::CaptureStats`].
pub const CAPTURE_STATS: &str = "capture-stats";

/// RAM ring buffer health stats. Payload: TBD.
pub const BUFFER_STATS: &str = "buffer-stats";

/// Non-fatal recorder error to surface in the UI. Payload: TBD.
pub const RECORDER_ERROR: &str = "recorder-error";
