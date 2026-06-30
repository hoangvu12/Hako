import { cn } from "@/lib/utils";
import type { GameAssets } from "@/games/use-game-assets";
import { clipPresenter } from "@/games/clip-presenter";
import type { ClipRecord } from "@/lib/api";

/**
 * Game-context overlay on the thumbnail: the source game's clip pills (champion /
 * agent portrait, map, mode, … — whatever that game surfaces) plus a W/L pill.
 * Content is game-defined via the per-game presenter (`clip-presenter.ts`), so
 * this component stays game-agnostic and a new game needs no change here.
 * Pointer-events-none so it never blocks the card's click target; renders nothing
 * when a clip carries no context (old clips, manual saves).
 *
 * Layout leaves the top-left corner free for the selection checkbox: the W/L pill
 * sits top-right, while the game pills sit bottom-left and fade on hover (like the
 * duration badge) so they never clash with the hover controls.
 */
export function ClipBadges({
  clip,
  assets,
}: {
  clip: ClipRecord;
  assets: GameAssets;
}) {
  const badges = clipPresenter(clip).cardBadges(clip, assets);
  const hasResult = clip.won != null;

  if (!badges.length && !hasResult) return null;

  return (
    <>
      {hasResult ? (
        <span
          className={cn(
            "pointer-events-none absolute top-2 right-2 z-20 rounded-full px-2 py-0.5 text-[10px] font-bold text-white",
            clip.won ? "bg-success/90" : "bg-destructive/90"
          )}
        >
          {clip.won ? "WIN" : "LOSS"}
        </span>
      ) : null}

      {badges.length ? (
        <div className="pointer-events-none absolute inset-x-2 bottom-2 z-10 flex max-w-[75%] flex-wrap items-center gap-1.5 transition-opacity group-hover/media:opacity-0">
          {badges.map((badge, i) =>
            badge.portrait ? (
              <span
                key={i}
                className="flex items-center gap-1 rounded-full bg-black/70 py-0.5 pr-2 pl-0.5 text-[10px] font-semibold text-white"
              >
                {badge.icon ? (
                  <img
                    src={badge.icon}
                    alt=""
                    className="size-4 rounded-full object-cover"
                  />
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
            )
          )}
        </div>
      ) : null}
    </>
  );
}
