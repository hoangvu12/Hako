import { invoke } from "@tauri-apps/api/core";

/**
 * Mirrors the Rust `RecorderStatus` struct (src-tauri/src/commands.rs).
 * Keep field names in sync — serde serializes with these exact keys.
 */
export interface RecorderStatus {
  capturing: boolean;
  valorant_detected: boolean;
  encoder: string | null;
  buffer_seconds: number;
  message: string;
}

/** Tauri event names emitted from the Rust core (src-tauri/src/events.rs). */
export const Events = {
  RecorderStatus: "recorder-status",
  CaptureStats: "capture-stats",
  ClipCreated: "clip-created",
  RecorderError: "recorder-error",
  MatchStateChanged: "match-state-changed",
  MatchSummary: "match-summary",
} as const;

/**
 * Mirrors the Rust `ClipRecord` (src-tauri/src/library/db.rs). Also the payload
 * of the `clip-created` event.
 */
export interface ClipRecord {
  id: number;
  path: string;
  title: string;
  event: string | null;
  duration_secs: number;
  width: number;
  height: number;
  size_bytes: number;
  thumb_path: string | null;
  created_unix_ms: number;
}

/** Invoke the stub `recorder_status` command. */
export async function getRecorderStatus(): Promise<RecorderStatus> {
  return invoke<RecorderStatus>("recorder_status");
}

/** Mirrors the Rust `GpuInfo` struct (src-tauri/src/core/device.rs). */
export interface GpuInfo {
  index: number;
  name: string;
  vendor: "Nvidia" | "Amd" | "Intel" | "Other";
  vendor_label: string;
  vendor_id: number;
  device_id: number;
  dedicated_vram_mb: number;
  is_software: boolean;
  encoder: string | null;
  preferred: boolean;
  drives_display: boolean;
}

/** Mirrors the Rust `GpuReport` struct (src-tauri/src/commands.rs). */
export interface GpuReport {
  adapters: GpuInfo[];
  selected_encoder: string | null;
  device_ok: boolean;
  feature_level: string | null;
  error: string | null;
}

/** Enumerate GPUs and validate the shared D3D11 device. */
export async function getGpuInfo(): Promise<GpuReport> {
  return invoke<GpuReport>("gpu_info");
}

/** Mirrors the Rust `FfmpegProbe` / `EncoderAvailability` (src-tauri/src/core/encode.rs). */
export interface EncoderAvailability {
  name: string;
  available: boolean;
}
export interface FfmpegProbe {
  avutil_version: string;
  avcodec_version: string;
  avformat_version: string;
  encoders: EncoderAvailability[];
}

/** Probe the bundled FFmpeg build (versions + hardware encoder availability). */
export async function getFfmpegInfo(): Promise<FfmpegProbe> {
  return invoke<FfmpegProbe>("ffmpeg_info");
}

/** Mirrors the Rust `WindowTarget` (src-tauri/src/core/capture.rs). */
export interface WindowTarget {
  hwnd: number;
  title: string;
}

/** Mirrors the Rust `CaptureStats` (src-tauri/src/core/capture.rs). */
export interface CaptureStats {
  fps: number;
  frames: number;
  arrived: number;
  width: number;
  height: number;
  target_fps: number;
  encoded_fps: number;
  encoded_frames: number;
  encoded_kbps: number;
}

/** List capturable top-level windows. */
export async function listWindows(): Promise<WindowTarget[]> {
  return invoke<WindowTarget[]>("list_windows");
}

/** Start WGC capture of a window (HWND) at an optional target FPS / adapter. */
export async function startCapture(
  hwnd: number,
  targetFps?: number,
  adapterIndex?: number
): Promise<void> {
  await invoke("start_capture", { hwnd, targetFps, adapterIndex });
}

/** Stop the running capture. */
export async function stopCapture(): Promise<void> {
  await invoke("stop_capture");
}

/**
 * Whether a capture session is currently running. The recorder lives in the Rust
 * core (background threads), so the UI uses this to re-sync after navigation —
 * navigating away must never stop capture.
 */
export async function captureStatus(): Promise<boolean> {
  return invoke<boolean>("capture_status");
}

/**
 * Save the last `seconds` (default 30) of buffered gameplay to an MP4 via
 * stream-copy, record it in the library, and return the new clip. Also fires the
 * `clip-created` event. The global hotkey **F9** triggers the same save for 30s.
 */
export async function saveClip(seconds?: number): Promise<ClipRecord> {
  return invoke<ClipRecord>("save_clip", { seconds });
}

/** All clips in the library, newest first. */
export async function clipsList(): Promise<ClipRecord[]> {
  return invoke<ClipRecord[]>("clips_list");
}

/** Delete a clip (row + file + thumbnail). */
export async function deleteClip(id: number): Promise<void> {
  await invoke("delete_clip", { id });
}

/** Rename a clip's title. */
export async function renameClip(id: number, title: string): Promise<void> {
  await invoke("rename_clip", { id, title });
}

/** Where a trim writes its result. */
export type TrimMode = "overwrite" | "copy";

/**
 * Loss-lessly trim a clip to `[start, end)` seconds (stream copy, optionally
 * dropping audio). `"copy"` creates a new library clip; `"overwrite"` replaces
 * the original file in place. Returns the resulting record.
 */
export async function trimClip(args: {
  id: number;
  start: number;
  end: number;
  dropAudio: boolean;
  mode: TrimMode;
}): Promise<ClipRecord> {
  return invoke<ClipRecord>("trim_clip", {
    id: args.id,
    start: args.start,
    end: args.end,
    dropAudio: args.dropAudio,
    mode: args.mode,
  });
}

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
}

/** Mirrors the Rust `Settings` (src-tauri/src/settings.rs). */
export interface Settings {
  target_fps: number;
  buffer_seconds: number;
  pad_before_secs: number;
  pad_after_secs: number;
  codec: string;
  bitrate_mbps: number;
  capture_audio: boolean;
  save_hotkey: string;
  events: EventToggles;
  storage_dir: string | null;
  /**
   * Capture backend: "wgc" (default, Vanguard-safe, capped at the desktop
   * composition rate) or "hook" (opt-in graphics-hook injection that beats the
   * cap but carries anti-cheat / ban risk).
   */
  capture_mode: string;
}

/** Read persisted settings. */
export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

/** Replace + persist settings. */
export async function updateSettings(next: Settings): Promise<void> {
  await invoke("update_settings", { next });
}

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
