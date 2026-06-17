//! Typed event channel names emitted from the Rust core to the webview.
//!
//! Payloads are defined alongside their producers (e.g. [`crate::commands::RecorderStatus`]).
//! The frontend subscribes via `@tauri-apps/api/event` `listen(...)`.

// Several of these are placeholders for future producers.
#![allow(dead_code)]

/// Periodic recorder status / heartbeat. Payload: [`crate::commands::RecorderStatus`].
pub const RECORDER_STATUS: &str = "recorder-status";

/// Valorant match state transitions (loop state, score, recording flag).
/// Payload: [`crate::valorant::orchestrator::MatchStatePayload`].
pub const MATCH_STATE_CHANGED: &str = "match-state-changed";

/// A new clip was written to the library (manual save or Valorant auto-clip).
/// Payload: [`crate::library::db::ClipRecord`].
pub const CLIP_CREATED: &str = "clip-created";

/// Post-match summary (K/D/A, headshot %, agent, map, win/loss, title) emitted
/// once `match-details` is fetched. Payload: [`crate::valorant::summary::MatchSummary`].
pub const MATCH_SUMMARY: &str = "match-summary";

/// Live capture throughput. Payload: [`crate::core::capture::CaptureStats`].
pub const CAPTURE_STATS: &str = "capture-stats";

/// RAM ring buffer health stats. Payload: TBD.
pub const BUFFER_STATS: &str = "buffer-stats";

/// Non-fatal recorder error to surface in the UI. Payload: TBD.
pub const RECORDER_ERROR: &str = "recorder-error";
