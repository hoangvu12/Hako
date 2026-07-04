import * as React from "react";

import type { ClipRecord } from "@/lib/api";
import { mapNameFromPath } from "@/hooks/use-valorant-assets";
import { GAMES, clipGame, type GameId } from "@/games/registry";

/** Sort orders offered in the toolbar. */
export type SortKey = "newest" | "oldest" | "largest" | "longest";
export type ResultFilter = "any" | "win" | "loss";
export type SourceFilter = "any" | "auto" | "manual";

export const SORTS: { key: SortKey; label: string }[] = [
  { key: "newest", label: "Newest first" },
  { key: "oldest", label: "Oldest first" },
  { key: "largest", label: "Largest first" },
  { key: "longest", label: "Longest first" },
];

export interface ClipFilters {
  search: string;
  games: string[]; // game ids ("valorant" | "lol")
  agents: string[]; // agent display names
  maps: string[]; // map asset paths
  modes: string[]; // mode display names
  events: string[]; // event labels
  result: ResultFilter;
  source: SourceFilter;
  sort: SortKey;
}

const EMPTY: ClipFilters = {
  search: "",
  games: [],
  agents: [],
  maps: [],
  modes: [],
  events: [],
  result: "any",
  source: "any",
  sort: "newest",
};

/** A map facet keeps its asset path (the filter value) + a display name. */
export interface MapFacet {
  path: string;
  name: string;
}

/** The distinct values actually present in the library — drives which filter
 * options we offer (no empty/irrelevant choices). */
export interface Facets {
  games: GameId[];
  agents: string[];
  maps: MapFacet[];
  modes: string[];
  events: string[];
  hasManual: boolean;
  hasAuto: boolean;
  hasResult: boolean;
}

/** One date-grouped block of clips, e.g. "Today" → [clips]. */
export interface ClipSection {
  key: string;
  label: string;
  clips: ClipRecord[];
}

/** An auto-clip is one produced by event detection (tagged `event`); a manual
 * F9/“Save last Ns” clip has no event. */
export function isAutoClip(clip: ClipRecord): boolean {
  return clip.event != null;
}

function deriveFacets(clips: ClipRecord[]): Facets {
  // Collect every facet in a single pass instead of ~8 separate array walks
  // (map/game/agent/mode/events + the three `some` scans). Sets dedupe as we go.
  const maps = new Set<string>();
  const games = new Set<GameId>();
  const agents = new Set<string>();
  const modes = new Set<string>();
  const events = new Set<string>();
  let hasManual = false;
  let hasAuto = false;
  let hasResult = false;
  for (const c of clips) {
    if (c.map) maps.add(c.map);
    games.add(clipGame(c.game));
    if (c.agent) agents.add(c.agent);
    if (c.mode) modes.add(c.mode);
    if (c.events) for (const e of c.events) if (e) events.add(e);
    if (isAutoClip(c)) hasAuto = true;
    else hasManual = true;
    if (c.won != null) hasResult = true;
  }
  const sorted = (s: Set<string>) => [...s].sort((a, b) => a.localeCompare(b));
  return {
    // Games actually present in the library, in registry (display) order.
    games: GAMES.map((g) => g.id).filter((id) => games.has(id)),
    agents: sorted(agents),
    maps: sorted(maps).map((path) => ({ path, name: mapNameFromPath(path) })),
    modes: sorted(modes),
    // Event facet across both the headline event and merged events list.
    events: sorted(events),
    hasManual,
    hasAuto,
    hasResult,
  };
}

function matches(clip: ClipRecord, f: ClipFilters): boolean {
  const q = f.search.trim().toLowerCase();
  if (q) {
    const hay = [clip.title, clip.agent, clip.mode, mapNameFromPath(clip.map)]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
    if (!hay.includes(q)) return false;
  }
  if (f.games.length && !f.games.includes(clipGame(clip.game))) return false;
  if (f.agents.length && !(clip.agent && f.agents.includes(clip.agent))) return false;
  if (f.maps.length && !(clip.map && f.maps.includes(clip.map))) return false;
  if (f.modes.length && !(clip.mode && f.modes.includes(clip.mode))) return false;
  if (f.events.length) {
    const ev = clip.events ?? [];
    if (!ev.some((e) => f.events.includes(e))) return false;
  }
  if (f.result === "win" && clip.won !== true) return false;
  if (f.result === "loss" && clip.won !== false) return false;
  if (f.source === "auto" && !isAutoClip(clip)) return false;
  if (f.source === "manual" && isAutoClip(clip)) return false;
  return true;
}

function compare(a: ClipRecord, b: ClipRecord, sort: SortKey): number {
  switch (sort) {
    case "oldest":
      return a.created_unix_ms - b.created_unix_ms;
    case "largest":
      return b.size_bytes - a.size_bytes;
    case "longest":
      return b.duration_secs - a.duration_secs;
    default:
      return b.created_unix_ms - a.created_unix_ms;
  }
}

/** Local-day bucket key (year-month-day) for grouping. */
function dayKey(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${d.getMonth()}-${d.getDate()}`;
}

function dayLabel(ms: number): string {
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((startOfDay(new Date()) - startOfDay(new Date(ms))) / 86_400_000);
  if (diff <= 0) return "Today";
  if (diff === 1) return "Yesterday";
  return new Date(ms).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

/** Filter, sort, then group the clips into date sections. Group order follows
 * the sort: oldest-first puts the earliest day on top; everything else newest
 * day on top. Clips within a group are sorted by the same key. */
function buildSections(clips: ClipRecord[], f: ClipFilters): ClipSection[] {
  const kept = clips.filter((c) => matches(c, f)).sort((a, b) => compare(a, b, f.sort));
  const byDay = new Map<string, ClipSection>();
  for (const clip of kept) {
    const key = dayKey(clip.created_unix_ms);
    let sec = byDay.get(key);
    if (!sec) {
      sec = { key, label: dayLabel(clip.created_unix_ms), clips: [] };
      byDay.set(key, sec);
    }
    sec.clips.push(clip);
  }
  // `kept` is already globally sorted, so first-seen day order == group order.
  return [...byDay.values()];
}

function countActive(f: ClipFilters): number {
  return (
    (f.search.trim() ? 1 : 0) +
    f.games.length +
    f.agents.length +
    f.maps.length +
    f.modes.length +
    f.events.length +
    (f.result !== "any" ? 1 : 0) +
    (f.source !== "any" ? 1 : 0)
  );
}

/**
 * Owns the clips filter/sort state and derives the facet lists + date-grouped,
 * filtered sections for the page. Recomputes only when the clips or a filter
 * change.
 */
export function useClipFilters(clips: ClipRecord[]) {
  const [filters, setFilters] = React.useState<ClipFilters>(EMPTY);

  const facets = React.useMemo(() => deriveFacets(clips), [clips]);
  const sections = React.useMemo(() => buildSections(clips, filters), [clips, filters]);
  const total = React.useMemo(() => sections.reduce((n, s) => n + s.clips.length, 0), [sections]);
  const activeCount = countActive(filters);

  const update = React.useCallback(
    (patch: Partial<ClipFilters>) => setFilters((f) => ({ ...f, ...patch })),
    [],
  );
  const toggle = React.useCallback(
    (key: "games" | "agents" | "maps" | "modes" | "events", value: string) =>
      setFilters((f) => {
        const set = new Set(f[key]);
        if (set.has(value)) set.delete(value);
        else set.add(value);
        return { ...f, [key]: [...set] };
      }),
    [],
  );
  const reset = React.useCallback(() => setFilters((f) => ({ ...EMPTY, sort: f.sort })), []);

  return {
    filters,
    facets,
    sections,
    total,
    activeCount,
    update,
    toggle,
    reset,
  };
}
