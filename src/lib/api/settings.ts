import { invoke } from "@tauri-apps/api/core";

import type { AudioConfig } from "./audio";

/** Per-event auto-clip toggles (mirrors Rust `EventToggles`). */
export interface EventToggles {
  kill: boolean;
  double_kill: boolean;
  triple_kill: boolean;
  quadra_kill: boolean;
  ace: boolean;
  knife: boolean;
  death: boolean;
  assist: boolean;
  victory: boolean;
  clutch: boolean;
  spike_detonated: boolean;
  spike_defused: boolean;
}

/** One event's clip window in seconds (mirrors Rust `EventTiming`). */
export interface EventTiming {
  before: number;
  after: number;
}

/**
 * Per-event clip windows — Outplayed's "Events timing" (mirrors Rust
 * `EventTimings`). One entry per toggle row; the auto-clip cut uses these instead
 * of the single global pad_before/after (which still drive manual saves).
 */
export interface EventTimings {
  kill: EventTiming;
  double_kill: EventTiming;
  triple_kill: EventTiming;
  quadra_kill: EventTiming;
  ace: EventTiming;
  knife: EventTiming;
  death: EventTiming;
  assist: EventTiming;
  victory: EventTiming;
  clutch: EventTiming;
  spike_detonated: EventTiming;
  spike_defused: EventTiming;
}

/** Outplayed-style capture mode (mirrors Rust `Settings.auto_capture_mode`). */
export type AutoCaptureMode = "manual" | "highlights" | "full_match" | "session";

/**
 * Per-game-mode auto-clip gate, keyed on the live presence `queueId` (mirrors
 * Rust `GameModeToggles`). A match whose queue is off isn't recorded in the
 * per-match modes (Highlights / Full match); Session mode is continuous and
 * unaffected. `other` is the catch-all for rotating / seasonal / custom queues
 * not named here. Defaults to all-on.
 */
export interface GameModeToggles {
  competitive: boolean;
  unrated: boolean;
  swiftplay: boolean;
  spikerush: boolean;
  deathmatch: boolean;
  /** Escalation. */
  ggteam: boolean;
  /** Replication. */
  onefa: boolean;
  /** Team Deathmatch. */
  hurm: boolean;
  /** Snowball Fight. */
  snowball: boolean;
  /** The rotating "New Map" featured queue. */
  newmap: boolean;
  premier: boolean;
  /** Catch-all for any queue id not listed above. */
  other: boolean;
}

/** Per-event auto-clip toggles for League (mirrors Rust `LolEventToggles`). */
export interface LolEventToggles {
  kill: boolean;
  double_kill: boolean;
  triple_kill: boolean;
  quadra_kill: boolean;
  pentakill: boolean;
  ace: boolean;
  first_blood: boolean;
  death: boolean;
  assist: boolean;
  dragon: boolean;
  baron: boolean;
  herald: boolean;
  turret: boolean;
  inhibitor: boolean;
  victory: boolean;
}

/** Per-event clip windows for League (mirrors Rust `LolEventTimings`). */
export interface LolEventTimings {
  kill: EventTiming;
  double_kill: EventTiming;
  triple_kill: EventTiming;
  quadra_kill: EventTiming;
  pentakill: EventTiming;
  ace: EventTiming;
  first_blood: EventTiming;
  death: EventTiming;
  assist: EventTiming;
  dragon: EventTiming;
  baron: EventTiming;
  herald: EventTiming;
  turret: EventTiming;
  inhibitor: EventTiming;
  victory: EventTiming;
}

/** League auto-capture config (mirrors Rust `LolGameSettings`). */
export interface LolGameSettings {
  auto_capture_mode: AutoCaptureMode;
  /** When true, Hako completely ignores League (no buffer, no auto-record).
   * Distinct from `auto_capture_mode = "manual"`, which keeps the buffer + save
   * hotkey working. */
  disabled: boolean;
  events: LolEventToggles;
  event_timings: LolEventTimings;
}

/** Per-event auto-clip toggles for Rematch (mirrors Rust `RematchEventToggles`). */
export interface RematchEventToggles {
  goal: boolean;
}

/** Per-event clip windows for Rematch (mirrors Rust `RematchEventTimings`). */
export interface RematchEventTimings {
  goal: EventTiming;
}

/** Rematch auto-capture config (mirrors Rust `RematchGameSettings`). */
export interface RematchGameSettings {
  auto_capture_mode: AutoCaptureMode;
  /** When true, Hako completely ignores Rematch (no buffer, no auto-record).
   * Distinct from `auto_capture_mode = "manual"`, which keeps the buffer + save
   * hotkey working. */
  disabled: boolean;
  events: RematchEventToggles;
  event_timings: RematchEventTimings;
}

/** Per-game settings for non-Valorant games (mirrors Rust `GamesSettings`). */
export interface GamesSettings {
  lol: LolGameSettings;
  rematch: RematchGameSettings;
}

/** Mirrors the Rust `Settings` (src-tauri/src/settings.rs). */
export interface Settings {
  target_fps: number;
  buffer_seconds: number;
  /**
   * Where the instant-replay buffer lives: "ram" (default — fast saves, costs
   * memory) or "disk" (spool compressed video to rolling segment files, freeing
   * RAM at the cost of continuous disk writes). Medal's "Recording buffer" toggle.
   */
  buffer_storage: string;
  pad_before_secs: number;
  pad_after_secs: number;
  codec: string;
  bitrate_mbps: number;
  /**
   * Stamp the "tabbed out" freeze card onto frozen frames (game minimized /
   * alt-tabbed / stale swapchain) so a clip viewer sees an intentional notice
   * instead of a silently-held frame. On by default.
   */
  freeze_overlay: boolean;
  /**
   * Skip copy/convert/encode when a captured frame is byte-identical to the
   * previous tick (game presenting slower than target_fps). Cuts redundant GPU
   * work; CFR gap-fill keeps output smooth. On by default — kill-switch only, no
   * dedicated UI control.
   */
  dirty_frame_skip: boolean;
  capture_audio: boolean;
  /** Microphone to mix in: "off", "auto" (system default), or a device id. */
  mic_source: string;
  /**
   * Medal-style per-source audio config. Null on configs written before the
   * multi-track feature — the backend then synthesizes one from `capture_audio`
   * + `mic_source`.
   */
  audio: AudioConfig | null;
  /**
   * Global save-clip hotkey, as a `global-hotkey` accelerator string (modifiers
   * + key joined by "+", e.g. "F9", "Alt+F7"). Registered live — changing it
   * re-registers the OS shortcut.
   */
  save_hotkey: string;
  /**
   * Seconds the save-clip hotkey captures (the CLIPS duration dropdown). Clamped
   * to `buffer_seconds` by the backend at save time.
   */
  clip_seconds: number;
  /**
   * Long-recording start/stop hotkey shown in the titlebar RECORDING popover.
   * Persisted and editable, but the manual long-recording feature is not wired
   * yet — display-only for now.
   */
  long_recording_hotkey: string;
  events: EventToggles;
  /** Per-event clip windows (Outplayed "Events timing"). */
  event_timings: EventTimings;
  /**
   * What the live Valorant orchestrator captures: "manual" (buffer + hotkey
   * only), "highlights" (default — cut per-event clips), "full_match" (keep the
   * whole match as one clip), or "session" (record continuously while in-game).
   */
  auto_capture_mode: AutoCaptureMode;
  /**
   * When true, Hako completely ignores Valorant (no buffer auto-attach, no
   * auto-record), regardless of `auto_capture_mode`. Distinct from
   * `auto_capture_mode = "manual"`, which keeps the buffer warm for the save
   * hotkey. The "don't capture this game at all" switch.
   */
  auto_capture_disabled: boolean;
  /**
   * Per-game-mode auto-clip gate, keyed on the live `queueId`. A match in a
   * mode that's toggled off is skipped in Highlights / Full match (Session is
   * continuous and unaffected). Defaults to all-on.
   */
  auto_clip_modes: GameModeToggles;
  /** Per-game settings for non-Valorant games (currently League). Valorant's
   * config stays in the flat fields above. */
  games: GamesSettings;
  storage_dir: string | null;
  /**
   * Which quality preset card is highlighted: "low" | "standard" | "high" |
   * "custom". Cosmetic — selecting a preset writes the concrete knobs
   * (resolution / target_fps / bitrate_mbps / codec), which are the source of
   * truth. "custom" leaves the knobs editable.
   */
  quality_preset: string;
  /**
   * Output resolution cap: "native" (no scaling) or a named target ("360p" |
   * "480p" | "720p" | "1080p" | "1440p" | "2160p"). When set, capture is
   * downscaled on-GPU to fit the target by height, never upscaling.
   */
  resolution: string;
  /** GPU to capture/encode on: -1 = Auto (display adapter), else adapter index. */
  gpu_adapter: number;
  /** Video encoder backend: "gpu" (hardware NVENC/QSV). Only GPU is implemented. */
  video_encoder: string;
  /** Master switch for in-game overlay toasts. */
  overlay_enabled: boolean;
  /** Per-trigger toggles, consulted only when `overlay_enabled`. */
  overlay_on_capture_state: boolean;
  overlay_on_clip_saved: boolean;
  overlay_on_disk_low: boolean;
  /** Corner the toast stack sits in over the game. */
  overlay_position: OverlayPosition;

  // --- Cloud upload (src-tauri/src/cloud) -------------------------------
  /** Auto-upload saved clips to `cloud_default_provider`. Off by default. */
  cloud_auto_upload: boolean;
  /** Provider id used for auto-upload / as the default manual-upload target. */
  cloud_default_provider: string | null;
  /** Local-cache budget (GiB) before "free up space" evicts oldest cloud-backed clips. */
  cloud_retention_gb: number;
  /** Master switch for the retention worker. Off by default. */
  cloud_free_up_space_enabled: boolean;
  /** Evict to the Recycle Bin (recoverable) rather than hard-deleting. */
  cloud_delete_to_recycle_bin: boolean;

  /**
   * Whether the first-run setup wizard has been finished or skipped. The wizard
   * shows while this is false. Fresh installs start false; configs written
   * before this field existed load as true (already-onboarded). See the Rust
   * `Settings::onboarding_completed`.
   */
  onboarding_completed: boolean;
}

/** Corner placement for the overlay toast stack (mirrors Rust `overlay_position`). */
export type OverlayPosition =
  | "top_left"
  | "top_right"
  | "bottom_left"
  | "bottom_right";

/** Read persisted settings. */
export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

/**
 * Whether the backend has hydrated its managed settings/library state from disk.
 * False only during the brief startup window before `setup` runs; reads that race
 * it see placeholder defaults. Paired with the `state-hydrated` event so the UI
 * can refetch once the real state lands. See `HydratedState` (Rust).
 */
export async function appHydrated(): Promise<boolean> {
  return invoke<boolean>("app_hydrated");
}

/** Replace + persist settings. */
export async function updateSettings(next: Settings): Promise<void> {
  await invoke("update_settings", { next });
}

/**
 * How many existing clips live under the folder `dir` resolves to (null → the
 * default `Videos/Hako`). Drives the "move N clips?" prompt shown when the clip
 * folder changes; 0 means nothing would move.
 */
export async function countClipsIn(dir: string | null): Promise<number> {
  return invoke<number>("count_clips_in", { dir });
}

/**
 * Move existing clips (and their thumbnails) from the `from` folder to the `to`
 * folder and repoint the library. Opt-in — only called after the user confirms
 * the move prompt. Runs off the UI thread; resolves with the count moved.
 */
export async function migrateClipsTo(
  from: string | null,
  to: string | null
): Promise<number> {
  return invoke<number>("migrate_clips_to", { from, to });
}
