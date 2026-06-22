/** Tauri event names emitted from the Rust core (src-tauri/src/events.rs). */
export const Events = {
  /** Managed settings/library state finished hydrating from disk at startup. */
  StateHydrated: "state-hydrated",
  RecorderStatus: "recorder-status",
  CaptureStats: "capture-stats",
  ClipCreated: "clip-created",
  RecorderError: "recorder-error",
  MatchStateChanged: "match-state-changed",
  MatchSummary: "match-summary",
  /** In-game overlay toast, emitted only to the `overlay` window. */
  OverlayNotify: "overlay-notify",
  /** Overlay placement config, emitted to the `overlay` window. */
  OverlayConfig: "overlay-config",
  /** Cloud-upload byte progress: `CloudUploadProgress`. */
  CloudUploadProgress: "cloud-upload-progress",
  /** Cloud-upload status transition: `CloudUploadStatus`. */
  CloudUploadStatus: "cloud-upload-status",
  /** Cloud-download byte progress (re-fetching an evicted clip): `CloudDownloadProgress`. */
  CloudDownloadProgress: "cloud-download-progress",
  /** Cloud-download status transition: `CloudDownloadStatus`. */
  CloudDownloadStatus: "cloud-download-status",
} as const;
