/** Per-stem editor state. Solo overrides mute across the stem set. */
export interface TrackCtl {
  muted: boolean;
  solo: boolean;
  /** 0–100. */
  volume: number;
  /** Offline noise suppression (RNNoise) on this stem — the mic's "noise
   *  cancel". Applied in the live preview *and* baked into the export. */
  denoise: boolean;
}
// Noise cancel is opt-in (off by default) on every stem — the editor never
// re-encodes or loads the denoiser unless the user turns it on for a track.
export const DEFAULT_CTL: TrackCtl = { muted: false, solo: false, volume: 100, denoise: false };

/** Playback-rate choices in the settings popover (YouTube-style, ascending). */
export const SPEED_OPTIONS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
export const MIN_TRIM = 0.3; // shortest selectable range, seconds
/** Tiles in the Rust-generated sprite-sheet filmstrip (commands.rs FILMSTRIP_TILES). */
export const FILMSTRIP_TILES = 16;
/** How many frames the scrubber actually draws — fewer than the sprite has, so
 *  each slot is wide enough to show a frame (almost) uncropped (Medal-style). At
 *  the ~50px strip height a 16:9 frame is ~89px wide, so ~13 of them tile the bar
 *  with each one showing essentially edge-to-edge (matches Medal's compact bar). */
export const FILMSTRIP_VISIBLE = 13;
/**
 * Custom range-aware streaming scheme (src-tauri/src/media.rs). The clip video
 * loads through this instead of the `asset:` protocol so WebView2 gets proper
 * `206 Partial Content` seeking and doesn't starve during playback.
 */
export const STREAM_SCHEME = "hakoclip";
