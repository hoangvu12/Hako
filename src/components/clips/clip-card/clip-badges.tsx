import { cn } from "@/lib/utils";
import { useGameAssetsContext } from "@/games/use-game-assets";
import { clipPresenter } from "@/games/clip-presenter";
import type { ClipRecord } from "@/lib/api";

/**
 * Win/loss pill, pinned top-right of the thumbnail (clear of the top-left
 * selection checkbox). Renders nothing when the clip has no known result.
 */
export function ClipResultBadge({ clip }: { clip: ClipRecord }) {
  if (clip.won == null) return null;
  return (
    <span
      className={cn(
        "pointer-events-none absolute top-2 right-2 z-20 rounded-full px-2 py-0.5 text-[10px] font-bold text-white",
        clip.won ? "bg-success/90" : "bg-destructive/90",
      )}
    >
      {clip.won ? "WIN" : "LOSS"}
    </span>
  );
}

/**
 * The source game's clip pills (champion / agent portrait, map, mode, … —
 * whatever that game surfaces). Content is game-defined via the per-game
 * presenter (`clip-presenter.ts`), so this stays game-agnostic and a new game
 * needs no change here. Inline (no positioning of its own) — the card places it
 * in the shared bottom-left badge row alongside the upload status icon.
 */
export function ClipBadges({ clip }: { clip: ClipRecord }) {
  const assets = useGameAssetsContext();
  const badges = clipPresenter(clip).cardBadges(clip, assets);
  if (!badges.length) return null;

  return (
    <>
      {badges.map((badge, i) =>
        badge.portrait ? (
          <span
            key={i}
            className="flex items-center gap-1 rounded-full bg-black/70 py-0.5 pr-2 pl-0.5 text-[10px] font-semibold text-white"
          >
            {badge.icon ? (
              <img src={badge.icon} alt="" className="size-4 rounded-full object-cover" />
            ) : (
              <span className="size-4" />
            )}
            {badge.label}
          </span>
        ) : (
          <span
            key={i}
            className="rounded-full bg-black/70 px-2 py-0.5 text-[10px] font-medium text-white"
          >
            {badge.label}
          </span>
        ),
      )}
    </>
  );
}
