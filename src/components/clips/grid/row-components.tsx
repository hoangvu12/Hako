import * as React from "react";

import { ClipCard } from "@/components/clips/clip-card";
import type { ClipRecord } from "@/lib/api";

// Memoized row pieces. The virtualizer re-renders `ClipsPage` on every scroll
// tick; with these boundaries (and stable props — the `rows`/`row.clips` arrays
// are memoized, the handlers stable) an already-mounted header or clip row
// short-circuits instead of re-rendering as you scroll. Card badges read the
// game-asset bundle from context (`GameAssetsProvider`), so it no longer rides
// down through here as a prop.
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
  onDelete,
  onRename,
}: {
  clips: ClipRecord[];
  columns: number;
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
          onDelete={onDelete}
          onRename={onRename}
        />
      ))}
    </div>
  );
});
