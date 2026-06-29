import * as React from "react";

import { mapNameFromPath, useValorantAssets } from "@/hooks/use-valorant-assets";
import { useLolAssets } from "@/hooks/use-lol-assets";
import type { ClipRecord } from "@/lib/api";
import { clipGame } from "./registry";

/** The game context a clip carries, resolved to renderable artwork + names. */
export interface ResolvedClipArt {
  /** Champion (League) / agent (Valorant) portrait URL, if known. */
  icon?: string;
  /** Champion / agent display name, if known. */
  primaryName: string | null;
  /** Readable map name ("" when the clip has no map). */
  mapName: string;
}

/**
 * One place that turns a clip's stored game context into artwork + labels,
 * branching on its source game. Valorant resolves agent + map art from
 * valorant-api; League resolves champion icons from Data Dragon (its champion
 * name lives in `agent`, and `map` is already a readable name).
 *
 * Both asset hooks are called unconditionally (hook rules) and merged here, so
 * every per-clip surface — the card badge, the details panel — shares one
 * resolver instead of duplicating the per-game `if (isLol)` branching. Adding a
 * game = add its asset hook + one branch in `resolve`.
 */
export function useGameAssets() {
  const valorant = useValorantAssets();
  const lol = useLolAssets();

  return React.useMemo(() => {
    const resolve = (clip: ClipRecord): ResolvedClipArt => {
      const game = clipGame(clip.game);
      if (game === "lol") {
        return {
          icon: lol.champFor(clip.agent)?.icon,
          primaryName: clip.agent,
          mapName: clip.map ?? "",
        };
      }
      if (game === "rematch") {
        // Rematch has no agent/champion art; the stadium is already a readable
        // name and lives in `map`.
        return {
          icon: undefined,
          primaryName: clip.agent ?? null,
          mapName: clip.map ?? "",
        };
      }
      return {
        icon: valorant.agentFor(clip)?.icon,
        primaryName: valorant.agentFor(clip)?.name ?? clip.agent ?? null,
        mapName: valorant.mapFor(clip.map)?.name ?? mapNameFromPath(clip.map),
      };
    };
    return { resolve, valorant, lol };
  }, [valorant, lol]);
}

/** Merged game-asset bundle threaded through the clips grid + toolbar. */
export type GameAssets = ReturnType<typeof useGameAssets>;
