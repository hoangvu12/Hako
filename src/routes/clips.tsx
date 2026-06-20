import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { convertFileSrc } from "@tauri-apps/api/core";
import { FilmStrip } from "@phosphor-icons/react";

import { predecodeImage } from "@/lib/image-predecode";

import { ClipCard } from "@/components/clips/clip-card";
import { ClipsToolbar } from "@/components/clips/clips-toolbar";
import { useClipFilters } from "@/components/clips/use-clip-filters";
import {
  useClips,
  useDeleteClip,
  useRenameClip,
  useSaveClip,
} from "@/hooks/use-library";
import { useSettings } from "@/hooks/use-settings";
import {
  useValorantAssets,
  type ValorantAssets,
} from "@/hooks/use-valorant-assets";
import type { ClipRecord } from "@/lib/api";

// Grid metrics (kept in sync with the row layout below). `GAP` mirrors the
// `gap-3`; `MIN_CARD` is the smallest comfortable card width before we drop a
// column. `EST_HEADER_ROW` seeds header scroll math.
const GAP = 12;
const MIN_CARD = 340;
const EST_CLIP_ROW = 280; // fallback until the container width is known
const EST_HEADER_ROW = 44;
// Fixed chrome below each card's aspect-video thumbnail: border (2) + meta block
// (~76: p-3.5 padding + single-line title row + meta line) + the row's bottom
// padding (pb-3 = 12). This is constant regardless of width — the title is
// truncated and the meta is one line — so a clip row's height is fully
// determined by the card width. That lets us size rows mathematically and skip
// per-row measurement (no ResizeObserver layout reads on the scroll path).
const CARD_CHROME = 90;
// How many rows beyond the rendered (visible + overscan) window to pre-decode
// thumbnails for, in each scroll direction. Wide enough to stay ahead of a fast
// flick without flooding the decode queue.
const DECODE_AHEAD_ROWS = 6;

/**
 * Responsive column count *and* content width of the scroll container (so the
 * grid tracks the panel, not the window). One ResizeObserver feeds both; the
 * width drives deterministic fixed row heights below.
 */
function useGridMetrics(ref: React.RefObject<HTMLElement | null>): {
  columns: number;
  width: number;
} {
  const [metrics, setMetrics] = React.useState({ columns: 1, width: 0 });
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const compute = (w: number) =>
      Math.max(1, Math.floor((w + GAP) / (MIN_CARD + GAP)));
    const apply = (w: number) =>
      setMetrics((m) => {
        const columns = compute(w);
        return m.columns === columns && m.width === w
          ? m
          : { columns, width: w };
      });
    // Measure synchronously here, before paint, so the first frame already has
    // the right column count. The ResizeObserver's initial callback is delivered
    // *after* paint, so relying on it alone flashes a single column on mount
    // (e.g. returning from the clip detail). `clientWidth` minus horizontal
    // padding matches the content-box width the observer reports below.
    const style = getComputedStyle(el);
    const padX =
      parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    apply(el.clientWidth - padX);
    const ro = new ResizeObserver((entries) => {
      apply(entries[0]?.contentRect.width ?? 0);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return metrics;
}

/** A flattened virtual row: either a date header or a row of up to `columns`
 * clips. Flattening lets one virtualizer drive the whole album view. */
type VirtualRow =
  | { type: "header"; key: string; label: string; count: number }
  | { type: "clips"; key: string; clips: ClipRecord[] };

function flattenSections(
  sections: { key: string; label: string; clips: ClipRecord[] }[],
  columns: number
): VirtualRow[] {
  const rows: VirtualRow[] = [];
  for (const sec of sections) {
    rows.push({
      type: "header",
      key: `h:${sec.key}`,
      label: sec.label,
      count: sec.clips.length,
    });
    for (let i = 0; i < sec.clips.length; i += columns) {
      rows.push({
        type: "clips",
        key: `${sec.key}:${i}`,
        clips: sec.clips.slice(i, i + columns),
      });
    }
  }
  return rows;
}

// Memoized row pieces. The virtualizer re-renders `ClipsPage` on every scroll
// tick; with these boundaries (and stable props — the `rows`/`row.clips` arrays
// are memoized, the handlers and `assets` are stable) an already-mounted header
// or clip row short-circuits instead of re-rendering as you scroll.
const HeaderRow = React.memo(function HeaderRow({
  label,
  count,
}: {
  label: string;
  count: number;
}) {
  return (
    <div className="flex items-baseline gap-2 pb-3 pt-1">
      <h2 className="text-sm font-semibold text-foreground">{label}</h2>
      <span className="text-xs font-medium text-muted-foreground">
        {count} {count === 1 ? "clip" : "clips"}
      </span>
    </div>
  );
});

const ClipRow = React.memo(function ClipRow({
  clips,
  columns,
  assets,
  onDelete,
  onRename,
}: {
  clips: ClipRecord[];
  columns: number;
  assets: ValorantAssets;
  onDelete: (clip: ClipRecord) => void;
  onRename: (clip: ClipRecord) => void;
}) {
  return (
    <div
      className="grid gap-3 pb-3"
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {clips.map((clip) => (
        <ClipCard
          key={clip.id}
          clip={clip}
          assets={assets}
          onDelete={onDelete}
          onRename={onRename}
        />
      ))}
    </div>
  );
});

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
  const assets = useValorantAssets();

  const allClips = React.useMemo(() => clips ?? [], [clips]);
  const { filters, facets, sections, total, activeCount, update, toggle, reset } =
    useClipFilters(allClips);

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
    <div className="flex h-full flex-col">
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
        assets={assets}
      />

      {saveError ? (
        <p className="shrink-0 bg-panel px-6 pb-2 text-sm text-destructive">
          {String(saveError)}
        </p>
      ) : null}

      {/* Grid (virtualized, grouped by date) */}
      <div
        ref={scrollRef}
        className="scrollbar-thin min-h-0 flex-1 overflow-y-auto p-6"
      >
        {isLoading ? (
          <p className="text-sm text-muted-foreground">Loading…</p>
        ) : noClipsAtAll ? (
          <div className="rounded-xl border border-dashed border-border/60 p-10 text-center text-sm text-muted-foreground">
            No clips yet. Press <kbd>F9</kbd> in-game or hit “Save last{" "}
            {clipSeconds}s” to capture a highlight.
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
                      assets={assets}
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
  );
}
