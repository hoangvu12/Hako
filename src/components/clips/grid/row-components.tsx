import * as React from "react";

import { ClipCard } from "@/components/clips/clip-card";
import type { GameAssets } from "@/games/use-game-assets";
import type { ClipRecord } from "@/lib/api";

// Memoized row pieces. The virtualizer re-renders `ClipsPage` on every scroll
// tick; with these boundaries (and stable props — the `rows`/`row.clips` arrays
// are memoized, the handlers and `assets` are stable) an already-mounted header
// or clip row short-circuits instead of re-rendering as you scroll.
export const HeaderRow = React.memo(function HeaderRow({
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

export const ClipRow = React.memo(function ClipRow({
  clips,
  columns,
  assets,
  onDelete,
  onRename,
}: {
  clips: ClipRecord[];
  columns: number;
  assets: GameAssets;
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
