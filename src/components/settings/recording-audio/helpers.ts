import type { AudioAppSel } from "@/lib/api";
import { GAMES } from "@/games/registry";

/**
 * Processes never offered as a generic app source: system mixers + Hako, plus
 * every supported game's own process — each is already represented by the
 * dedicated "Game Audio" row, so listing it here would duplicate the game. The
 * game processes are sourced from the game registry (lowercased there) so this
 * stays correct as games are added, rather than hard-coding Valorant.
 */
export const SESSION_BLACKLIST = new Set([
  "svchost.exe",
  "audiodg.exe",
  "hako.exe",
  ...GAMES.flatMap((g) => g.processNames),
]);

/** Upsert an app source by id, merging `patch` (creating it enabled if absent). */
export function upsertApp(
  apps: AudioAppSel[],
  id: string,
  name: string,
  patch: Partial<AudioAppSel>,
): AudioAppSel[] {
  const idx = apps.findIndex((a) => a.id === id);
  if (idx >= 0) {
    const next = [...apps];
    next[idx] = { ...next[idx], ...patch };
    return next;
  }
  return [...apps, { id, name, enabled: true, volume: 100, ...patch }];
}
