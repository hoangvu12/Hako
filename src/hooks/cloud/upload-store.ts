import { useSyncExternalStore } from "react";

// --- live upload progress store --------------------------------------------
//
// High-frequency byte progress (≈4×/s for the one in-flight clip) rides this
// tiny external store so it never churns the React Query cache. The event bridge
// writes via `progress`/`emitProgress`; components read merged per-clip views in
// `uploads.ts`.

export interface LiveProgress {
  sent: number;
  total: number;
  bytesPerSec: number;
}

export const progress = new Map<number, LiveProgress>();
const progressListeners = new Set<() => void>();
// Re-created on every mutation so `useSyncExternalStore` sees a new reference.
let progressSnapshot = new Map<number, LiveProgress>();

export function emitProgress() {
  progressSnapshot = new Map(progress);
  for (const l of progressListeners) l();
}

function subscribeProgress(cb: () => void) {
  progressListeners.add(cb);
  return () => progressListeners.delete(cb);
}

export function useProgressMap() {
  return useSyncExternalStore(
    subscribeProgress,
    () => progressSnapshot,
    () => progressSnapshot,
  );
}

/**
 * One clip's live progress, subscribed individually. Every progress tick (~4×/s)
 * notifies all subscribers, but `getSnapshot` returns this clip's entry — a
 * reference that only changes when *this* clip updates (we replace the object on
 * each `progress.set`). So `useSyncExternalStore` bails out of re-rendering every
 * badge except the one clip actually streaming. Reading the whole map instead
 * (see `useProgressMap`) would re-render all visible badges on every tick.
 */
export function useClipProgress(clipId: number): LiveProgress | undefined {
  return useSyncExternalStore(
    subscribeProgress,
    () => progressSnapshot.get(clipId),
    () => progressSnapshot.get(clipId),
  );
}
