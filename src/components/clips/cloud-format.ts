/** Rate/percent helpers for the cloud-upload UI (badge + toast). Byte sizes use
 * the shared `formatBytes`. */

import { formatBytes } from "@/lib/format";

/** Per-second throughput, e.g. "12.4 MB/s". Blank for a zero/idle rate. */
export function fmtRate(bytesPerSec: number): string {
  if (bytesPerSec <= 0) return "";
  return `${formatBytes(bytesPerSec)}/s`;
}

/** Whole-percent of a byte transfer (0–100), guarding against a zero total. */
export function pctOf(sent: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.round((sent / total) * 100));
}
