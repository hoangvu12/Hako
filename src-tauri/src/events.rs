//! Typed event channel names emitted from the Rust core to the webview.
//!
//! Payloads are defined alongside their producers (e.g. [`crate::commands::RecorderStatus`]).
//! The frontend subscribes via `@tauri-apps/api/event` `listen(...)`.

// Several of these are placeholders for future producers.
#![allow(dead_code)]

/// Emitted once the managed settings/library state has been hydrated from disk
/// in `setup` (it starts as a placeholder so an IPC call that wins the startup
/// race can't panic — see `main.rs`). The webview refetches `settings`/`clips`
/// on this so a first read that saw placeholders self-heals. Payload: none.
pub const STATE_HYDRATED: &str = "state-hydrated";

/// Periodic recorder status / heartbeat. Payload: [`crate::commands::RecorderStatus`].
pub const RECORDER_STATUS: &str = "recorder-status";

/// Valorant match state transitions (loop state, score, recording flag).
/// Payload: [`crate::valorant::integration::MatchStatePayload`].
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

/// In-game overlay toast, emitted only to the `overlay` window.
/// Payload: [`crate::overlay::OverlayNotice`].
pub const OVERLAY_NOTIFY: &str = "overlay-notify";

/// Overlay window configuration (corner placement), emitted to the `overlay`
/// window when it's shown and when settings change.
/// Payload: [`crate::overlay::OverlayConfig`].
pub const OVERLAY_CONFIG: &str = "overlay-config";

/// Cloud-upload byte progress (throttled ~250 ms). Payload:
/// `{ clip_id, provider_id, sent, total, bytes_per_sec }` (see `cloud::upload`).
pub const CLOUD_UPLOAD_PROGRESS: &str = "cloud-upload-progress";

/// Cloud-upload status transition (queued/uploading/done/error/canceled).
/// Payload: `{ clip_id, provider_id, status, error? }` (see `cloud::upload`).
pub const CLOUD_UPLOAD_STATUS: &str = "cloud-upload-status";

/// Cloud-download byte progress (throttled ~250 ms) while re-fetching an evicted
/// clip for editing. Payload: `{ clip_id, received, total, bytes_per_sec }`
/// (see `cloud::download`).
pub const CLOUD_DOWNLOAD_PROGRESS: &str = "cloud-download-progress";

/// Cloud-download status transition (downloading/done/error) for an evicted clip
/// being re-fetched. Payload: `{ clip_id, status, error? }` (see `cloud::download`).
pub const CLOUD_DOWNLOAD_STATUS: &str = "cloud-download-status";
