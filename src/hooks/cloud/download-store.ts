import { useSyncExternalStore } from "react";

// --- live download state ---------------------------------------------------
//
// "Download to edit" re-fetches an evicted clip. Presence in this store == the
// clip is actively downloading; entries appear on the `downloading` status, get
// byte updates from the progress event, and are dropped on done/error. Kept off
// the query cache for the same reason upload progress is (high-frequency ticks).

interface LiveDownload {
  received: number;
  total: number;
  bytesPerSec: number;
}

export const downloads = new Map<number, LiveDownload>();
const downloadListeners = new Set<() => void>();
let downloadSnapshot = new Map<number, LiveDownload>();

export function emitDownloads() {
  downloadSnapshot = new Map(downloads);
  for (const l of downloadListeners) l();
}

function subscribeDownloads(cb: () => void) {
  downloadListeners.add(cb);
  return () => downloadListeners.delete(cb);
}

function useDownloadMap() {
  return useSyncExternalStore(
    subscribeDownloads,
    () => downloadSnapshot,
    () => downloadSnapshot,
  );
}

/** A clip's live download view for the editor's "download to edit" affordance.
 * `downloading` is true from the start of the fetch until it completes/fails. */
export interface DownloadView {
  downloading: boolean;
  received: number;
  total: number;
  pct: number;
}

export function useClipDownload(clipId: number): DownloadView {
  const map = useDownloadMap();
  const d = map.get(clipId);
  if (!d) return { downloading: false, received: 0, total: 0, pct: 0 };
  const pct = d.total > 0 ? Math.min(100, (d.received / d.total) * 100) : 0;
  return { downloading: true, received: d.received, total: d.total, pct };
}
