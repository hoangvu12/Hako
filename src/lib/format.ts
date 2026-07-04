/**
 * Shared display formatters. These were previously copy-pasted per component
 * folder (`fmtSize`/`fmtBytes` existed in four places with subtly different
 * rounding, `fmtTime` in two), which meant user-facing size/time strings could
 * silently drift apart. This is the one canonical implementation of each.
 *
 * Genuinely distinct formatters stay local to their feature: `fmtDuration`
 * (rounds instead of floors), `fmtClock` (tenths), `fmtDate`, `timeAgo`, and
 * the settings gauges' deliberately-coarse `fmtBytesCoarse`.
 */

/**
 * Human-readable byte size across B/KB/MB/GB. The app's canonical size
 * formatter — clip file sizes and cloud transfer byte counts.
 */
export function formatBytes(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

/** A playback timestamp as `m:ss` (floored). Guards NaN/negative to `0:00`. */
export function formatTime(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
