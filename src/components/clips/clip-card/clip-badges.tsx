import { cn } from "@/lib/utils";
import type { GameAssets } from "@/games/use-game-assets";
import type { ClipRecord } from "@/lib/api";

/**
 * Game-context overlay on the thumbnail: champion/agent portrait + name, map, and
 * a W/L pill — whatever the clip carries. Game-aware via the shared
 * `GameAssets.resolve` (Valorant agents from valorant-api, League champions from
 * Data Dragon), so this stays game-agnostic. Pointer-events-none so it never
 * blocks the card's click target; degrades to text (or nothing) when
 * artwork/fields are absent (old clips, manual saves).
 *
 * Layout leaves the top-left corner free for the selection checkbox: the W/L
 * pill sits top-right, while the agent + map move to the bottom-left and fade on
 * hover (like the duration badge) so they never clash with the hover controls.
 */
export function ClipBadges({
  clip,
  assets,
}: {
  clip: ClipRecord;
  assets: GameAssets;
}) {
  const { icon, primaryName, mapName } = assets.resolve(clip);
  const hasResult = clip.won != null;

  if (!primaryName && !mapName && !hasResult) return null;

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

      {primaryName || mapName ? (
        <div className="pointer-events-none absolute inset-x-2 bottom-2 z-10 flex max-w-[75%] flex-wrap items-center gap-1.5 transition-opacity group-hover/media:opacity-0">
          {primaryName ? (
            <span className="flex items-center gap-1 rounded-full bg-black/70 py-0.5 pr-2 pl-0.5 text-[10px] font-semibold text-white">
              {icon ? (
                <img
                  src={icon}
                  alt=""
                  className="size-4 rounded-full object-cover"
                />
              ) : (
                <span className="size-4" />
              )}
              {primaryName}
            </span>
          ) : null}
          {mapName ? (
            <span className="rounded-full bg-black/70 px-2 py-0.5 text-[10px] font-medium text-white">
              {mapName}
            </span>
          ) : null}
        </div>
      ) : null}
    </>
  );
}
