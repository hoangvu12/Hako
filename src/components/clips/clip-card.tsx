import * as React from "react";

import { cn } from "@/lib/utils";
import type { ValorantAssets } from "@/hooks/use-valorant-assets";
import type { ClipRecord } from "@/lib/api";
import {
  toggleClipSelected,
  useClipSelected,
  useSelectionActive,
} from "@/components/clips/use-clip-selection";
import { ClipUploadBadge } from "./clip-upload-badge";
import { ClipPreview } from "./clip-card/clip-preview";
import { ClipBadges } from "./clip-card/clip-badges";
import { ClipActionsMenu } from "./clip-card/clip-actions-menu";
import { SelectCheckbox } from "./clip-card/select-checkbox";
import { fmtSize, timeAgo } from "./clip-card/format";

function Dot() {
  return <span className="size-[3px] shrink-0 rounded-full bg-secondary" />;
}

// Memoized: the clips grid re-renders on every scroll tick (virtualizer state),
// hover, and resize. Without this, each of those re-rendered all ~25 visible
// cards and their full Radix dropdown + icon subtrees. With stable props (see
// the parent's `useCallback` handlers + the session-stable `assets`), a card
// now re-renders only when its own `clip` changes (or its selection toggles).
export const ClipCard = React.memo(function ClipCard({
  clip,
  onDelete,
  onRename,
  assets,
}: {
  clip: ClipRecord;
  onDelete: (clip: ClipRecord) => void;
  onRename: (clip: ClipRecord) => void;
  assets?: ValorantAssets;
}) {
  // Per-card selection: subscribed individually so toggling another card never
  // re-renders this one (see `use-clip-selection`). Drives both the corner
  // checkbox and the selected ring below.
  const selected = useClipSelected(clip.id);
  // Whether the grid is in selection mode at all. A boolean, so this only
  // re-renders the card on the empty↔non-empty transition — while it's true the
  // whole card becomes a select-on-click target (the overlay below), so clicking
  // anywhere toggles selection instead of opening the clip.
  const selectionActive = useSelectionActive();

  // Every event the clip's window covered, falling back to the headline tag for
  // clips saved before multi-event tracking (mirrors the detail panel).
  const eventLabels = clip.events.length
    ? clip.events
    : clip.event
      ? [clip.event]
      : [];

  return (
    <div
      className={cn(
        "group relative flex flex-col overflow-hidden rounded-xl border bg-card shadow-sm transition-colors [contain-intrinsic-size:auto_280px] [content-visibility:auto]",
        selected
          ? "border-primary ring-2 ring-primary"
          : "border-border/60 hover:border-border"
      )}
    >
      {/* Thumbnail / hover-preview, with the game-context overlay on top */}
      <div className="relative">
        <ClipPreview clip={clip} />
        <ClipBadges clip={clip} assets={assets} />
        <ClipUploadBadge clipId={clip.id} />
        <SelectCheckbox id={clip.id} selected={selected} />
      </div>

      {/* Meta */}
      <div className="flex flex-1 flex-col gap-1.5 p-3.5">
        <div className="flex items-center justify-between gap-2">
          <h3
            className="truncate text-sm font-semibold text-card-foreground"
            title={clip.title}
          >
            {clip.title || "Untitled"}
          </h3>

          <ClipActionsMenu clip={clip} onRename={onRename} onDelete={onDelete} />
        </div>

        {/* One quiet metadata line: the event(s) lead (slightly emphasized,
            truncating when many), then when / how big. No chips or per-item
            icons — the thumbnail badges already carry the visual weight. */}
        <div className="flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
          {eventLabels.length ? (
            <>
              <span
                className="min-w-0 truncate text-foreground/80"
                title={eventLabels.join(", ")}
              >
                {eventLabels.join(", ")}
              </span>
              <Dot />
            </>
          ) : null}
          <span className="shrink-0">{timeAgo(clip.created_unix_ms)}</span>
          <Dot />
          <span className="shrink-0">{fmtSize(clip.size_bytes)}</span>
        </div>
      </div>

      {/* While selecting, the whole card is a toggle target — a transparent
          layer over everything (incl. the thumbnail's open-detail Link) so a
          click anywhere selects/deselects instead of navigating. */}
      {selectionActive ? (
        <button
          type="button"
          aria-label={selected ? "Deselect clip" : "Select clip"}
          onClick={() => toggleClipSelected(clip.id)}
          className="absolute inset-0 z-40 cursor-pointer"
        />
      ) : null}
    </div>
  );
});
