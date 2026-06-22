import * as React from "react";

import type { AudioTrackInfo } from "@/lib/api";

export interface UseTrackMixerArgs {
  /** Clip id — stem bytes are pulled over IPC (mediabunny can't fetch the
   *  `hakoclip://` scheme; see `readClipRange`). */
  clipId: number;
  /** Clip file size in bytes — the `CustomSource`'s `getSize`. */
  fileSize: number;
  /** Audio stems (index ≥ 1); empty ⇒ mixer disabled, native audio kept. */
  stems: AudioTrackInfo[];
  videoRef: React.RefObject<HTMLVideoElement | null>;
  /** Per-stem linear gain (0..1) keyed by stem index — solo/mute already resolved. */
  stemGains: Map<number, number>;
  /** Master monitor gain (0..1) from the top-bar mute/volume. */
  masterGain: number;
  /** Stem indices to noise-cancel in the preview (RNNoise) — kept in lockstep
   *  with the export's per-stem denoise flag so preview ≈ what you save. */
  denoiseStemIdx: number[];
}

/** One playing buffer-source set's time anchor, for drift math. */
export interface Anchor {
  /** `AudioContext.currentTime` at which the sources begin. */
  ctxTime: number;
  /** Media time (video clock, seconds) the sources begin at. */
  mediaTime: number;
  /** Playback rate captured at (re)start. */
  rate: number;
}

export interface Graph {
  ctx: AudioContext;
  master: GainNode;
  /** Per-stem gain node, keyed by stem index. */
  gains: Map<number, GainNode>;
  /** Per-stem decoded buffer, keyed by stem index (empty stems omitted). */
  buffers: Map<number, AudioBuffer>;
}
