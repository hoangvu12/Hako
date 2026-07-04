import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc } from "@tauri-apps/api/core";
import { FilmStrip } from "@phosphor-icons/react";

import { predecodeImage } from "@/lib/image-predecode";

import { ClipsToolbar } from "@/components/clips/clips-toolbar";
import { ClipsBulkBar } from "@/components/clips/clips-bulk-bar";
import { useClipFilters } from "@/components/clips/use-clip-filters";
import {
  clearSelection,
  getSelectedIds,
  pruneSelection,
  setSelection,
  useSelection,
} from "@/components/clips/use-clip-selection";
import {
  useClips,
  useDeleteClip,
  useRenameClip,
  useSaveClip,
} from "@/hooks/use-library";
import { useUploadClip } from "@/hooks/use-cloud";
import { useSettings } from "@/hooks/use-settings";
import { GameAssetsProvider } from "@/games/use-game-assets";
import type { ClipRecord } from "@/lib/api";
import {
  CARD_CHROME,
  DECODE_AHEAD_ROWS,
  EST_CLIP_ROW,
  EST_HEADER_ROW,
  GAP,
  useGridMetrics,
} from "@/components/clips/grid/metrics";
import { flattenSections } from "@/components/clips/grid/virtual-rows";
import { ClipRow, HeaderRow } from "@/components/clips/grid/row-components";

// Remember the grid's scroll offset across mounts. The route component unmounts
// when you open a clip detail, so a component-local ref would reset to 0 — this
// module-scoped value survives, letting us land back where you left off.
let savedScrollTop = 0;

export default function ClipsPage() {
  const { data: clips, isLoading } = useClips();
  const { data: settings } = useSettings();
  const clipSeconds = settings?.clip_seconds ?? 30;
  // Destructure the (referentially stable) mutate fns so the handlers below can
  // be memoized without re-binding when a mutation's state changes.
  const {
    mutate: saveClip,
    isPending: saving,
    error: saveError,
  } = useSaveClip();
  const { mutate: deleteClip } = useDeleteClip();
  const { mutate: renameClip } = useRenameClip();
  const { mutate: uploadClip, isPending: uploading } = useUploadClip();
  // One library across every game — scoping is done via the toolbar's Game
  // filter, not a separate page mode.
  const allClips = React.useMemo(() => clips ?? [], [clips]);
  const { filters, facets, sections, total, activeCount, update, toggle, reset } =
    useClipFilters(allClips);

  // Bulk selection. `selection` (the whole set) drives the page-level swap to
  // the bulk bar; individual cards subscribe per-id, so toggling one card
  // doesn't re-render the others.
  const selection = useSelection();
  const selectionActive = selection.size > 0;

  // Every clip id currently passing the filters, in display order — the target
  // set for "select all".
  const filteredIds = React.useMemo(
    () => sections.flatMap((s) => s.clips.map((c) => c.id)),
    [sections]
  );
  const allSelected =
    filteredIds.length > 0 && filteredIds.every((id) => selection.has(id));

  // Drop selected ids for clips that no longer exist (deleted via a card's own
  // menu), so the bulk bar's count never counts phantoms.
  React.useEffect(() => {
    pruneSelection(new Set(allClips.map((c) => c.id)));
  }, [allClips]);

  // Stable bulk handlers — they read the live selection imperatively rather than
  // closing over it, so the memoized bulk bar isn't re-rendered on scroll ticks.
  const handleSelectAll = React.useCallback(
    () => setSelection(filteredIds),
    [filteredIds]
  );
  const handleClearSelection = React.useCallback(() => clearSelection(), []);
  // Confirmation is owned by the bulk bar's alert dialog; this just performs it.
  const handleBulkDelete = React.useCallback(() => {
    const ids = getSelectedIds();
    if (ids.length === 0) return;
    for (const id of ids) deleteClip(id);
    clearSelection();
  }, [deleteClip]);
  const handleBulkUpload = React.useCallback(() => {
    const ids = getSelectedIds();
    if (ids.length === 0) return;
    for (const id of ids) uploadClip({ clipId: id });
    clearSelection();
  }, [uploadClip]);

  const scrollRef = React.useRef<HTMLDivElement>(null);
  const { columns, width } = useGridMetrics(scrollRef);

  // Deterministic clip-row height: the aspect-video thumbnail height (card width
  // × 9/16) plus the fixed chrome. Computed, not measured — so the virtualizer
  // positions every row by arithmetic and we attach no per-row ResizeObserver.
  const clipRowHeight = React.useMemo(() => {
    if (width <= 0) return EST_CLIP_ROW;
    const cardWidth = (width - (columns - 1) * GAP) / columns;
    return Math.round((cardWidth * 9) / 16) + CARD_CHROME;
  }, [width, columns]);

  const rows = React.useMemo(
    () => flattenSections(sections, columns),
    [sections, columns]
  );

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) =>
      rows[i]?.type === "header" ? EST_HEADER_ROW : clipRowHeight,
    overscan: 4,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  // Row heights are exact (not measured), but a width/column/filter change
  // reflows them — reset the virtualizer's size cache so it recomputes offsets
  // from the new `clipRowHeight`.
  React.useEffect(() => {
    rowVirtualizer.measure();
  }, [clipRowHeight, rows, rowVirtualizer]);

  // Persist the scroll offset on every scroll so it's current the moment we
  // navigate away (the component unmounts before any cleanup could read it).
  React.useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      savedScrollTop = el.scrollTop;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  // Restore the saved offset once, after the grid is measured and rows exist —
  // only then is the virtualizer's total height in the DOM, so the container is
  // tall enough to actually scroll there (otherwise scrollTop clamps to 0).
  const gridReady = !isLoading && width > 0 && rows.length > 0;
  const didRestore = React.useRef(false);
  React.useLayoutEffect(() => {
    if (didRestore.current || !gridReady) return;
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = savedScrollTop;
    didRestore.current = true;
  }, [gridReady]);

  // Decode thumbnails ahead of the viewport. The rendered window (visible +
  // overscan) is bracketed here by a wider band whose thumbnails we force-decode
  // off-DOM, so by the time the virtualizer mounts those rows the bitmap is
  // already warm and the <img> paints without blocking the scroll frame.
  const virtualItems = rowVirtualizer.getVirtualItems();
  const firstRendered = virtualItems[0]?.index ?? 0;
  const lastRendered = virtualItems[virtualItems.length - 1]?.index ?? 0;
  React.useEffect(() => {
    if (rows.length === 0) return;
    const from = Math.max(0, firstRendered - DECODE_AHEAD_ROWS);
    const to = Math.min(rows.length - 1, lastRendered + DECODE_AHEAD_ROWS);
    for (let i = from; i <= to; i++) {
      const row = rows[i];
      if (row?.type !== "clips") continue;
      for (const clip of row.clips) {
        if (clip.thumb_path) predecodeImage(convertFileSrc(clip.thumb_path));
      }
    }
  }, [firstRendered, lastRendered, rows]);

  // Stable per-card handlers (each takes the clip) so `ClipCard`'s `React.memo`
  // isn't defeated by fresh closures on every grid render.
  const handleRename = React.useCallback(
    (clip: ClipRecord) => {
      const next = window.prompt("Rename clip", clip.title);
      if (next && next !== clip.title) renameClip({ id: clip.id, title: next });
    },
    [renameClip]
  );
  const handleDelete = React.useCallback(
    (clip: ClipRecord) => deleteClip(clip.id),
    [deleteClip]
  );
  const handleSave = React.useCallback(() => saveClip(undefined), [saveClip]);

  const noClipsAtAll = !isLoading && allClips.length === 0;
  const noMatches = !isLoading && allClips.length > 0 && total === 0;

  return (
    <GameAssetsProvider>
      <div className="flex h-full flex-col">
      {selectionActive ? (
        <ClipsBulkBar
          selectedCount={selection.size}
          allSelected={allSelected}
          onSelectAll={handleSelectAll}
          onClear={handleClearSelection}
          onDelete={handleBulkDelete}
          onUpload={handleBulkUpload}
          uploading={uploading}
        />
      ) : (
        <ClipsToolbar
          clipSeconds={clipSeconds}
          onSave={handleSave}
          saving={saving}
          total={total}
          filters={filters}
          facets={facets}
          activeCount={activeCount}
          update={update}
          toggle={toggle}
          reset={reset}
        />
      )}

      {saveError ? (
        <p className="shrink-0 bg-panel px-6 pb-2 text-sm text-destructive">
          {String(saveError)}
        </p>
      ) : null}

      {/* Grid (virtualized, grouped by date). `group/grid` + `data-selecting`
          let every card's checkbox stay visible in selection mode via CSS
          alone — no card re-render when entering/leaving selection mode. */}
      <div
        ref={scrollRef}
        data-selecting={selectionActive ? "" : undefined}
        className="group/grid scrollbar-thin min-h-0 flex-1 overflow-y-auto p-6"
      >
        {isLoading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : noClipsAtAll ? (
          <div className="rounded-xl border border-dashed border-border/60 p-10 text-center text-sm text-muted-foreground">
            No clips yet. Press <kbd>F9</kbd> in-game or hit "Save last{" "}
            {clipSeconds}s" to capture a highlight.
          </div>
        ) : noMatches ? (
          <div className="flex flex-col items-center gap-3 rounded-xl border border-dashed border-border/60 p-10 text-center">
            <FilmStrip className="size-8 text-muted-foreground/60" />
            <p className="text-sm text-muted-foreground">
              No clips match your filters.
            </p>
            <button
              type="button"
              onClick={reset}
              className="text-sm font-medium text-primary transition-opacity hover:opacity-80"
            >
              Clear filters
            </button>
          </div>
        ) : (
          <div
            style={{
              height: rowVirtualizer.getTotalSize(),
              width: "100%",
              position: "relative",
            }}
          >
            {virtualItems.map((virtualRow) => {
              const row = rows[virtualRow.index];
              if (!row) return null;
              return (
                <div
                  key={virtualRow.key}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  {row.type === "header" ? (
                    <HeaderRow label={row.label} count={row.count} />
                  ) : (
                    <ClipRow
                      clips={row.clips}
                      columns={columns}
                      onDelete={handleDelete}
                      onRename={handleRename}
                    />
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
      </div>
    </GameAssetsProvider>
  );
}
