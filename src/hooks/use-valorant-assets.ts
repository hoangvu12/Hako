import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import type { ClipRecord } from "@/lib/api";

/**
 * Valorant agent + map artwork, fetched once from the community mirror
 * `valorant-api.com` and cached for the session. Drives the clips filter chips
 * and per-card badges. Clip records store the agent name/uuid and the map asset
 * path (`map`); these lookups turn those into display names + icon URLs.
 *
 * The data is effectively static (changes only on a new agent/map release), so
 * the query never refetches (`staleTime: Infinity`). Icons load straight from
 * the returned CDN URLs — the app has no CSP restriction (`csp: null`).
 */

const AGENTS_URL =
  "https://valorant-api.com/v1/agents?isPlayableCharacter=true";
const MAPS_URL = "https://valorant-api.com/v1/maps";
const GAMEMODES_URL = "https://valorant-api.com/v1/gamemodes";

/**
 * Stored clip `mode` labels come from two vocabularies (see the Rust
 * `game_mode_name` / `queue_id_name`): the post-match gameMode asset name and the
 * live-queue display name. valorant-api keys its artwork by the gameMode display
 * name, so the queue-only labels — all four bomb-based queues plus the rotating
 * "New Map" — are aliased onto their shared "Standard" gamemode artwork. Keys and
 * values are lowercased.
 */
const MODE_ALIASES: Record<string, string> = {
  competitive: "standard",
  unrated: "standard",
  premier: "standard",
  "new map": "standard",
};

export interface ValAgent {
  uuid: string;
  name: string;
  role: string;
  /** Portrait icon URL. */
  icon: string;
  /** Faint agent-select background texture (transparent PNG). */
  background: string;
  /** Signature colors as a ready-to-use CSS gradient ("" if none). */
  gradient: string;
}

export interface ValMap {
  uuid: string;
  /** Asset path matching `MatchDetails.mapId` / the stored `ClipRecord.map`. */
  mapUrl: string;
  name: string;
  /** Small wide list icon. */
  listIcon: string;
  /** Full splash image. */
  splash: string;
}

export interface ValGameMode {
  uuid: string;
  name: string;
  /** Square mode icon (the chip/row thumbnail). */
  icon: string;
  /** Tall portrait list-view art (full-bleed row background). */
  tall: string;
}

interface AssetData {
  agents: ValAgent[];
  maps: ValMap[];
  modes: ValGameMode[];
}

interface ApiList<T> {
  data: T[] | null;
}

async function fetchAssets(): Promise<AssetData> {
  const [agentsRes, mapsRes, modesRes] = await Promise.all([
    fetch(AGENTS_URL),
    fetch(MAPS_URL),
    fetch(GAMEMODES_URL),
  ]);
  if (!agentsRes.ok || !mapsRes.ok || !modesRes.ok) {
    throw new Error("valorant-api.com asset fetch failed");
  }
  const agentsJson = (await agentsRes.json()) as ApiList<{
    uuid: string;
    displayName: string;
    displayIcon: string | null;
    role: { displayName: string } | null;
    background: string | null;
    backgroundGradientColors: string[] | null;
  }>;
  const mapsJson = (await mapsRes.json()) as ApiList<{
    uuid: string;
    displayName: string;
    mapUrl: string;
    listViewIcon: string | null;
    splash: string | null;
  }>;
  const modesJson = (await modesRes.json()) as ApiList<{
    uuid: string;
    displayName: string;
    displayIcon: string | null;
    listViewIconTall: string | null;
  }>;

  const agents: ValAgent[] = (agentsJson.data ?? []).map((a) => {
    // backgroundGradientColors are 8-digit hex without "#" (RGBA); the webview
    // is Chromium, so #RRGGBBAA is fine. Build a diagonal sweep of the agent's
    // signature colors.
    const cols = (a.backgroundGradientColors ?? [])
      .filter(Boolean)
      .map((c) => `#${c}`);
    const gradient =
      cols.length >= 2 ? `linear-gradient(135deg, ${cols.join(", ")})` : "";
    return {
      uuid: a.uuid,
      name: a.displayName,
      role: a.role?.displayName ?? "",
      icon: a.displayIcon ?? "",
      background: a.background ?? "",
      gradient,
    };
  });
  const maps: ValMap[] = (mapsJson.data ?? [])
    // The Range (practice) and other null-mapUrl entries aren't real matches.
    .filter((m) => !!m.mapUrl)
    .map((m) => ({
      uuid: m.uuid,
      mapUrl: m.mapUrl,
      name: m.displayName,
      listIcon: m.listViewIcon ?? "",
      splash: m.splash ?? "",
    }));
  const modes: ValGameMode[] = (modesJson.data ?? [])
    // Drop modes with no artwork (Basic Training / Onboarding) — nothing to show.
    .filter((m) => !!m.displayIcon)
    .map((m) => ({
      uuid: m.uuid,
      name: m.displayName,
      icon: m.displayIcon ?? "",
      tall: m.listViewIconTall ?? "",
    }));

  return { agents, maps, modes };
}

export interface ValorantAssets {
  isLoading: boolean;
  /** Resolve a clip's agent (by uuid, falling back to name). */
  agentFor: (clip: Pick<ClipRecord, "agent" | "agent_id">) => ValAgent | undefined;
  /** Resolve an agent by display name. */
  agentByName: (name: string | null | undefined) => ValAgent | undefined;
  /** Resolve a map by its asset path (the stored `ClipRecord.map`). */
  mapFor: (path: string | null | undefined) => ValMap | undefined;
  /** Resolve a game mode by its stored display-name label (gameMode name or
   * live-queue name; bomb queues alias onto "Standard"). */
  modeFor: (name: string | null | undefined) => ValGameMode | undefined;
}

export function useValorantAssets(): ValorantAssets {
  const q = useQuery({
    queryKey: ["valorant-assets"],
    queryFn: fetchAssets,
    staleTime: Infinity,
    gcTime: Infinity,
    retry: 1,
  });

  return React.useMemo<ValorantAssets>(() => {
    const agents = q.data?.agents ?? [];
    const maps = q.data?.maps ?? [];
    const modes = q.data?.modes ?? [];
    const byId = new Map(agents.map((a) => [a.uuid.toLowerCase(), a]));
    const byName = new Map(agents.map((a) => [a.name.toLowerCase(), a]));
    const byUrl = new Map(maps.map((m) => [m.mapUrl, m]));
    const modeByName = new Map(modes.map((m) => [m.name.toLowerCase(), m]));

    const agentByName = (name: string | null | undefined) =>
      name ? byName.get(name.toLowerCase()) : undefined;

    return {
      isLoading: q.isLoading,
      agentFor: (clip) =>
        (clip.agent_id ? byId.get(clip.agent_id.toLowerCase()) : undefined) ??
        agentByName(clip.agent),
      agentByName,
      mapFor: (path) => (path ? byUrl.get(path) : undefined),
      modeFor: (name) => {
        if (!name) return undefined;
        const key = name.toLowerCase();
        return modeByName.get(MODE_ALIASES[key] ?? key);
      },
    };
  }, [q.data, q.isLoading]);
}

/**
 * Prettify a map asset path without the asset table, e.g.
 * "/Game/Maps/Ascent/Ascent" → "Ascent". Used as a fallback label before the
 * asset query resolves (and matches the `/valorant` panel's `mapName`).
 */
export function mapNameFromPath(raw: string | null | undefined): string {
  if (!raw) return "";
  const parts = raw.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? "";
}
