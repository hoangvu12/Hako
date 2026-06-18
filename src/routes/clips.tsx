import * as React from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { FilmStrip } from "@phosphor-icons/react";

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
import { useValorantAssets } from "@/hooks/use-valorant-assets";
import type { ClipRecord } from "@/lib/api";

// Grid metrics (kept in sync with the row layout below). `GAP` mirrors the old
// `gap-5`; `MIN_CARD` is the smallest comfortable card width before we drop a
// column. The row heights are measured per row; these only seed scroll math.
const GAP = 20;
const MIN_CARD = 240;
const EST_CLIP_ROW = 280;
const EST_HEADER_ROW = 44;

/**
 * Responsive column count derived from the scroll container's *content* width
 * (so it tracks the panel, not the window). Recomputes on resize via a single
 * ResizeObserver — the virtualizer keys its row layout off this.
 */
function useGridColumns(ref: React.RefObject<HTMLElement | null>): number {
  const [cols, setCols] = React.useState(1);
  React.useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const compute = (w: number) =>
      Math.max(1, Math.floor((w + GAP) / (MIN_CARD + GAP)));
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width ?? 0;
      const next = compute(w);
      setCols((c) => (c === next ? c : next));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return cols;
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

export default function ClipsPage() {
  const { data: clips, isLoading } = useClips();
  const { data: settings } = useSettings();
  const clipSeconds = settings?.clip_seconds ?? 30;
  const save = useSaveClip();
  const del = useDeleteClip();
  const rename = useRenameClip();
  const assets = useValorantAssets();

  const allClips = React.useMemo(() => clips ?? [], [clips]);
  const { filters, facets, sections, total, activeCount, update, toggle, reset } =
    useClipFilters(allClips);

  const scrollRef = React.useRef<HTMLDivElement>(null);
  const columns = useGridColumns(scrollRef);

  const rows = React.useMemo(
    () => flattenSections(sections, columns),
    [sections, columns]
  );

  const rowVirtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) =>
      rows[i]?.type === "header" ? EST_HEADER_ROW : EST_CLIP_ROW,
    overscan: 4,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  // A column-count or filter change reflows every row to a new height; drop the
  // cached measurements so the virtualizer re-measures instead of trusting
  // stale sizes.
  React.useEffect(() => {
    rowVirtualizer.measure();
  }, [columns, rows, rowVirtualizer]);

  function handleRename(clip: ClipRecord) {
    const next = window.prompt("Rename clip", clip.title);
    if (next && next !== clip.title) rename.mutate({ id: clip.id, title: next });
  }

  const noClipsAtAll = !isLoading && allClips.length === 0;
  const noMatches = !isLoading && allClips.length > 0 && total === 0;

  return (
    <div className="flex h-full flex-col">
      <ClipsToolbar
        clipSeconds={clipSeconds}
        onSave={() => save.mutate(undefined)}
        saving={save.isPending}
        total={total}
        filters={filters}
        facets={facets}
        activeCount={activeCount}
        update={update}
        toggle={toggle}
        reset={reset}
        assets={assets}
      />

      {save.error ? (
        <p className="shrink-0 bg-panel px-6 pb-2 text-sm text-destructive">
          {String(save.error)}
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
            {rowVirtualizer.getVirtualItems().map((virtualRow) => {
              const row = rows[virtualRow.index];
              if (!row) return null;
              return (
                <div
                  key={virtualRow.key}
                  data-index={virtualRow.index}
                  ref={rowVirtualizer.measureElement}
                  className="absolute top-0 left-0 w-full"
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  {row.type === "header" ? (
                    <div className="flex items-baseline gap-2 pb-3 pt-1">
                      <h2 className="text-sm font-semibold text-foreground">
                        {row.label}
                      </h2>
                      <span className="text-xs font-medium text-muted-foreground">
                        {row.count} {row.count === 1 ? "clip" : "clips"}
                      </span>
                    </div>
                  ) : (
                    <div
                      className="grid gap-5 pb-5"
                      style={{
                        gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))`,
                      }}
                    >
                      {row.clips.map((clip) => (
                        <ClipCard
                          key={clip.id}
                          clip={clip}
                          assets={assets}
                          onDelete={() => del.mutate(clip.id)}
                          onRename={() => handleRename(clip)}
                        />
                      ))}
                    </div>
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
