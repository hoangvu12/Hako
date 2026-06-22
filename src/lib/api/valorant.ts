import { invoke } from "@tauri-apps/api/core";

/** Mirrors the Rust `ValorantStatus` (src-tauri/src/valorant/service.rs). */
export interface ValorantStatus {
  running: boolean;
  connected: boolean;
  loop_state: string | null;
  score_ally: number;
  score_enemy: number;
  map: string;
  error: string | null;
}

/** Best-effort live Valorant status for the /valorant panel. */
export async function valorantStatus(): Promise<ValorantStatus> {
  return invoke<ValorantStatus>("valorant_status");
}

/**
 * Mirrors the Rust `summary::MatchSummary`
 * (src-tauri/src/valorant/summary.rs). Payload of the `match-summary` event,
 * emitted once `match-details` is fetched after a match ends.
 */
export interface MatchSummary {
  kills: number;
  deaths: number;
  assists: number;
  /** Headshot % over all recorded damage (0–100). */
  headshot_pct: number;
  /** Agent UUID. */
  agent_id: string;
  /** Resolved agent display name (e.g. "Jett"); "" if the lookup failed. */
  agent: string;
  /** Map asset path (prettify with the panel's `mapName`). */
  map: string;
  /** Game-mode display name (e.g. "Standard", "Spike Rush"). */
  mode: string;
  won: boolean;
  /** Match length in ms. */
  duration_ms: number;
  /** Built title, e.g. "🟩 Victory - Jett [21/14/5]". */
  title: string;
}

/**
 * Mirrors the Rust `orchestrator::MatchStatePayload`
 * (src-tauri/src/valorant/orchestrator.rs). Payload of the `match-state-changed`
 * event, pushed live from the presence loop.
 */
export interface MatchStatePayload {
  /** `MENUS` / `PREGAME` / `INGAME` / etc. */
  loop_state: string;
  /** True while a match is in progress (state machine INGAME). */
  in_match: boolean;
  /** True while a full-match session is actually being recorded. */
  recording: boolean;
  score_ally: number;
  score_enemy: number;
  map: string;
}
