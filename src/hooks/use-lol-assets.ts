import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import { queryKeys } from "@/lib/query-keys";

/**
 * League of Legends champion artwork from Riot's Data Dragon CDN, fetched once
 * and cached for the session (the version list changes only on a patch). League
 * clips store the champion *display name* in `ClipRecord.agent`, so the lookup is
 * by name → square champion icon URL.
 *
 * Mirrors the Valorant asset hook's shape (`useValorantAssets`) so the per-clip
 * badges + detail panel can pick a provider by `clip.game`.
 */

const VERSIONS_URL = "https://ddragon.leagueoflegends.com/api/versions.json";

export interface LolChampion {
  /** Display name as it appears in the live feed (e.g. "Miss Fortune"). */
  name: string;
  /** Data Dragon id / asset key (e.g. "MissFortune", "Wukong"→"MonkeyKing"). */
  id: string;
  /** Square champion icon URL. */
  icon: string;
}

interface ChampionApi {
  data: Record<
    string,
    { id: string; key: string; name: string; image: { full: string } }
  >;
}

/** Internal map ids → readable map ids, mirroring Rust's `friendly_map`. */
const LOL_MAP_NAMES: Record<string, string> = {
  Map11: "Summoner's Rift",
  Map12: "Howling Abyss",
  Map21: "Nexus Blitz",
  Map22: "Convergence",
  Map30: "Rings of Wrath",
};

/**
 * Readable map name from a clip's stored `map`. The Live Client feed gives the
 * internal asset id (`"Map12"`), so translate the known ones; anything else (a
 * future map, or an already-readable value from the patched backend) passes
 * through. Idempotent, so it's safe over both old and new clips.
 */
export function friendlyLolMap(map: string | null | undefined): string {
  if (!map) return "";
  return LOL_MAP_NAMES[map] ?? map;
}

async function fetchChampions(): Promise<LolChampion[]> {
  const versRes = await fetch(VERSIONS_URL);
  if (!versRes.ok) throw new Error("ddragon versions fetch failed");
  const versions = (await versRes.json()) as string[];
  const ver = versions[0];
  if (!ver) throw new Error("ddragon: no versions");
  const champRes = await fetch(
    `https://ddragon.leagueoflegends.com/cdn/${ver}/data/en_US/champion.json`
  );
  if (!champRes.ok) throw new Error("ddragon champion fetch failed");
  const json = (await champRes.json()) as ChampionApi;
  return Object.values(json.data).map((c) => ({
    name: c.name,
    id: c.id,
    icon: `https://ddragon.leagueoflegends.com/cdn/${ver}/img/champion/${c.image.full}`,
  }));
}

export interface LolAssets {
  isLoading: boolean;
  /** Resolve a champion by display name (the stored `ClipRecord.agent`). */
  champFor: (name: string | null | undefined) => LolChampion | undefined;
}

export function useLolAssets(): LolAssets {
  const q = useQuery({
    queryKey: queryKeys.lolAssets,
    queryFn: fetchChampions,
    // Champion data only changes on a patch, so once it loads we never refetch.
    // But don't let a transient CDN failure stick for the whole session — a
    // failed fetch would otherwise blank every League icon until restart.
    staleTime: Infinity,
    gcTime: Infinity,
    retry: 3,
    retryDelay: (attempt) => Math.min(1000 * 2 ** attempt, 8000),
  });

  return React.useMemo<LolAssets>(() => {
    // Index by both display name (what the live feed / clip stores) and Data
    // Dragon id, so a champion whose id differs from its name still resolves.
    const byKey = new Map<string, LolChampion>();
    for (const c of q.data ?? []) {
      byKey.set(c.name.toLowerCase(), c);
      byKey.set(c.id.toLowerCase(), c);
    }
    return {
      isLoading: q.isLoading,
      champFor: (name) => (name ? byKey.get(name.toLowerCase()) : undefined),
    };
  }, [q.data, q.isLoading]);
}
