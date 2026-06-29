import * as React from "react";
import { useQuery } from "@tanstack/react-query";

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
  /** Square champion icon URL. */
  icon: string;
}

interface ChampionApi {
  data: Record<
    string,
    { id: string; key: string; name: string; image: { full: string } }
  >;
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
    queryKey: ["lol-assets"],
    queryFn: fetchChampions,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: 1,
  });

  return React.useMemo<LolAssets>(() => {
    const byName = new Map((q.data ?? []).map((c) => [c.name.toLowerCase(), c]));
    return {
      isLoading: q.isLoading,
      champFor: (name) => (name ? byName.get(name.toLowerCase()) : undefined),
    };
  }, [q.data, q.isLoading]);
}
