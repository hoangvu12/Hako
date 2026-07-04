import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { Events, valorantStatus, type MatchStatePayload, type MatchSummary } from "@/lib/api";
import { queryKeys } from "@/lib/query-keys";

/**
 * Live Valorant status, polled every 2s (matches the presence cadence).
 * Best-effort: `connected:false` / `error` set when Riot isn't running.
 */
export function useValorantStatus() {
  return useQuery({
    queryKey: queryKeys.valorantStatus,
    queryFn: valorantStatus,
    retry: false,
    refetchInterval: 2000,
  });
}

/**
 * Latest `match-state-changed` snapshot from the orchestrator's presence loop.
 * Event-driven (the loop emits on every tick), so this is fresher than the
 * `valorantStatus` poll and is the source of truth for the live recording badge.
 * Returns `null` until the first event arrives.
 */
export function useMatchState() {
  const [state, setState] = useState<MatchStatePayload | null>(null);

  useEffect(() => {
    const unlisten = listen<MatchStatePayload>(Events.MatchStateChanged, (e) =>
      setState(e.payload),
    );
    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, []);

  return state;
}

/**
 * Latest post-match summary (`match-summary` event): K/D/A, headshot %, agent,
 * map, win/loss, title. Emitted once after each match's details are fetched.
 * Returns `null` until the first summary arrives.
 */
export function useMatchSummary() {
  const [summary, setSummary] = useState<MatchSummary | null>(null);

  useEffect(() => {
    const unlisten = listen<MatchSummary>(Events.MatchSummary, (e) => setSummary(e.payload));
    return () => {
      unlisten.then((off) => off()).catch(() => {});
    };
  }, []);

  return summary;
}
