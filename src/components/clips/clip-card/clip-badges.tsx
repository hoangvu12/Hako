import { cn } from "@/lib/utils";
import { mapNameFromPath, type ValorantAssets } from "@/hooks/use-valorant-assets";
import type { ClipRecord } from "@/lib/api";

/**
 * Game-context overlay on the thumbnail: agent portrait + name, map, and a W/L
 * pill — whatever the clip carries. Pointer-events-none so it never blocks the
 * card's click target; degrades to text (or nothing) when artwork/fields are
 * absent (old clips, manual saves outside a match).
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
  assets?: ValorantAssets;
}) {
  const agent = assets?.agentFor(clip);
  const mapName = assets?.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map);
  const agentName = agent?.name ?? clip.agent ?? null;
  const hasResult = clip.won != null;

  if (!agentName && !mapName && !hasResult) return null;

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

      {agentName || mapName ? (
        <div className="pointer-events-none absolute inset-x-2 bottom-2 z-10 flex max-w-[75%] flex-wrap items-center gap-1.5 transition-opacity group-hover/media:opacity-0">
          {agentName ? (
            <span className="flex items-center gap-1 rounded-full bg-black/70 py-0.5 pr-2 pl-0.5 text-[10px] font-semibold text-white">
              {agent?.icon ? (
                <img
                  src={agent.icon}
                  alt=""
                  className="size-4 rounded-full object-cover"
                />
              ) : (
                <span className="size-4" />
              )}
              {agentName}
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
