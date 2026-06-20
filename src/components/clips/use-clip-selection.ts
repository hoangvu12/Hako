import * as React from "react";

/**
 * Bulk-selection store for the clips grid.
 *
 * Lives in module scope (like the grid's `savedScrollTop`) so a selection
 * survives the route unmount when you open a clip detail and come back. It's a
 * tiny external store read through `useSyncExternalStore`, mirroring the
 * upload-progress store in `use-cloud.ts`: a toggle notifies every subscriber,
 * but each card's `getSnapshot` returns *its own* boolean, so an unchanged
 * boolean bails out of re-render. Net effect — toggling one clip re-renders only
 * that one card, never the whole grid.
 */

const selected = new Set<number>();
const listeners = new Set<() => void>();
// Re-created on every change so `useSyncExternalStore` consumers that read the
// whole set (the page / bulk bar) see a new reference and update.
let snapshot: ReadonlySet<number> = new Set();

function emit() {
  snapshot = new Set(selected);
  for (const l of listeners) l();
}

function subscribe(cb: () => void) {
  listeners.add(cb);
  return () => listeners.delete(cb);
}

/** Flip one clip's membership. */
export function toggleClipSelected(id: number) {
  if (selected.has(id)) selected.delete(id);
  else selected.add(id);
  emit();
}

/** Replace the whole selection (used by "select all"). */
export function setSelection(ids: Iterable<number>) {
  selected.clear();
  for (const id of ids) selected.add(id);
  emit();
}

/** Clear the selection (exits selection mode). No-op when already empty. */
export function clearSelection() {
  if (selected.size === 0) return;
  selected.clear();
  emit();
}

/** Drop any selected ids that are no longer in the library, so a clip deleted
 * elsewhere (e.g. via a card's own menu) doesn't linger as a phantom count. */
export function pruneSelection(existing: ReadonlySet<number>) {
  let changed = false;
  for (const id of selected) {
    if (!existing.has(id)) {
      selected.delete(id);
      changed = true;
    }
  }
  if (changed) emit();
}

/** Read the current selection imperatively (for stable action handlers that
 * shouldn't close over render-time state). */
export function getSelectedIds(): number[] {
  return [...selected];
}

/** Per-card subscription: re-renders only when *this* clip's membership flips. */
export function useClipSelected(id: number): boolean {
  return React.useSyncExternalStore(
    subscribe,
    () => snapshot.has(id),
    () => snapshot.has(id)
  );
}

/** Whole-selection subscription for the page + bulk bar (re-renders on any
 * change — both are outside the per-card scroll path). */
export function useSelection(): ReadonlySet<number> {
  return React.useSyncExternalStore(
    subscribe,
    () => snapshot,
    () => snapshot
  );
}

/** Is *anything* selected — i.e. is the grid in selection mode? A boolean, so a
 * card subscribing to it only re-renders on the empty↔non-empty transition (not
 * when other cards toggle). Lets a card flip its whole surface into a
 * select-on-click target while selecting. */
export function useSelectionActive(): boolean {
  return React.useSyncExternalStore(
    subscribe,
    () => snapshot.size > 0,
    () => snapshot.size > 0
  );
}
