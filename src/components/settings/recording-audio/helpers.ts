import type { AudioAppSel } from "@/lib/api";

/**
 * Processes never offered as a generic app source: system mixers + Hako, plus
 * the Valorant game process itself — it's already represented by the dedicated
 * "Game Audio" row, so listing it here would duplicate the game.
 */
export const SESSION_BLACKLIST = new Set([
  "svchost.exe",
  "audiodg.exe",
  "hako.exe",
  "valorant-win64-shipping.exe",
]);

/** Upsert an app source by id, merging `patch` (creating it enabled if absent). */
export function upsertApp(
  apps: AudioAppSel[],
  id: string,
  name: string,
  patch: Partial<AudioAppSel>
): AudioAppSel[] {
  const idx = apps.findIndex((a) => a.id === id);
  if (idx >= 0) {
    const next = [...apps];
    next[idx] = { ...next[idx], ...patch };
    return next;
  }
  return [...apps, { id, name, enabled: true, volume: 100, ...patch }];
}
