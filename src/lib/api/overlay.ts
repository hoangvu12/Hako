import { invoke } from "@tauri-apps/api/core";

import type { OverlayPosition } from "./settings";

/**
 * Kind of in-game overlay toast (mirrors Rust `OverlayKind`, snake_case serde).
 * Drives the toast's icon + accent color in the overlay window.
 */
export type OverlayKind =
  | "recording_started"
  | "recording_stopped"
  | "clip_saved"
  | "disk_low";

/**
 * One in-game overlay toast (mirrors Rust `OverlayNotice`). Payload of the
 * `overlay-notify` event, consumed only by the `overlay` window.
 */
export interface OverlayNotice {
  kind: OverlayKind;
  title: string;
  subtitle: string | null;
  /** Auto-dismiss after this many ms. */
  ttlMs: number;
}

/**
 * Overlay placement config (mirrors Rust `OverlayConfig`). Payload of the
 * `overlay-config` event; tells the overlay window which corner to stack in.
 */
export interface OverlayConfig {
  position: OverlayPosition;
}

/**
 * Fire a sample overlay toast (Settings → "Test overlay"). Force-shows the
 * overlay over Valorant — or the primary monitor when the game isn't running —
 * so placement and click-through can be verified without launching a match.
 */
export async function overlayTest(): Promise<void> {
  await invoke("overlay_test");
}
