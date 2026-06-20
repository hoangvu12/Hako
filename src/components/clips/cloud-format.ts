/** Shared byte/rate formatting for the cloud-upload UI (badge + toast). */

export function fmtBytes(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(1)} GB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MB`;
  if (bytes >= 1 << 10) return `${(bytes / (1 << 10)).toFixed(0)} KB`;
  return `${bytes} B`;
}

/** Per-second throughput, e.g. "12.4 MB/s". Blank for a zero/idle rate. */
export function fmtRate(bytesPerSec: number): string {
  if (bytesPerSec <= 0) return "";
  return `${fmtBytes(bytesPerSec)}/s`;
}

/** Whole-percent of a byte transfer (0–100), guarding against a zero total. */
export function pctOf(sent: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.round((sent / total) * 100));
}
