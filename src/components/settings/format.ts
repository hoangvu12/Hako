// The replay buffer keeps ~`buffer_seconds` of *compressed* video in RAM at the
// bitrate ceiling (mirrors the Rust `PacketRing`, which stores encoded packets
// sized by bitrate × time). Audio tracks add only a few MB, so they're ignored
// here. This is the app's dominant, directly-tunable RAM cost.
export function estBufferBytes(bitrateMbps: number, bufferSeconds: number): number {
  return ((bitrateMbps * 1_000_000) / 8) * bufferSeconds;
}

/** Coarse size for the RAM/storage gauges — whole MB, or GB with one decimal
 * past 1 GiB. Deliberately blunter than the shared `formatBytes` (no KB/B tier,
 * MB rounded) so the buffer/quota readouts stay stable and glanceable. */
export function fmtBytesCoarse(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  return `${Math.round(bytes / (1 << 20))} MB`;
}

// Past this the buffer dominates a typical 8–16 GB machine; nudge the user.
export const RAM_WARN_BYTES = 2 * (1 << 30);
