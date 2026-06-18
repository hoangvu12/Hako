import { invoke } from "@tauri-apps/api/core";

/**
 * Mirrors the Rust `RecorderStatus` struct (src-tauri/src/commands.rs).
 * Keep field names in sync — serde serializes with these exact keys.
 */
export interface RecorderStatus {
  capturing: boolean;
  /** Capturing AND delivering fresh frames. False while the game is minimized
   * (frozen): the recorder is alive but footage is stale, so the UI shows an
   * honest "paused" state instead of "recording". */
  capturing_live: boolean;
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
  /** Headline event (the dominant one when a clip's window merged several). */
  event: string | null;
  /** Every event captured in the clip's window, in time order. Falls back to
   * `[event]` for clips saved before multi-event tracking existed. */
  events: string[];
  duration_secs: number;
  width: number;
  height: number;
  size_bytes: number;
  thumb_path: string | null;
  /** Sprite-sheet filmstrip (one JPEG, N tiles) for the editor scrubber. */
  filmstrip_path: string | null;
  created_unix_ms: number;

  // --- Valorant game context (all nullable) -----------------------------
  // Filled for clips cut from a match: auto-clips carry everything; manual F9
  // saves carry agent/map/mode (win + K/D/A are unknowable mid-match). All null
  // for clips saved outside a match and for clips predating this metadata.
  /** Agent display name (e.g. "Jett"). */
  agent: string | null;
  /** Agent UUID (`characterId`) — pairs with `agent` for artwork lookup. */
  agent_id: string | null;
  /** Map asset path (e.g. "/Game/Maps/Ascent/Ascent"); prettify for display. */
  map: string | null;
  /** Game-mode display name (e.g. "Competitive", "Standard"). */
  mode: string | null;
  /** Match result when known (auto-clips): true = win, false = loss. */
  won: boolean | null;
  /** Match K/D/A totals (auto-clips only). */
  kills: number | null;
  deaths: number | null;
  assists: number | null;
  /** Headshot % over recorded damage, 0–100 (auto-clips only). */
  headshot_pct: number | null;
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
  /** Resolved capture adapter for the current setting. */
  capture_adapter: number | null;
  /** Resolved encode adapter (== capture on the zero-copy fast path). */
  encode_adapter: number | null;
  /** True when encode differs from capture (cross-adapter NV12 hand-off needed). */
  cross_adapter: boolean;
  /** Whether the cross-adapter capability probe passed (true on the fast path). */
  cross_adapter_ok: boolean;
  /** Why the cross-adapter probe failed, if it did. */
  cross_adapter_reason: string | null;
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

/** Mirrors the Rust `AudioInputDevice` (src-tauri/src/core/audio.rs). */
export interface AudioInputDevice {
  /** Stable WASAPI endpoint id — round-tripped back as `Settings.mic_source`. */
  id: string;
  /** Human-friendly name (e.g. "Microphone (USB Audio Device)"). */
  name: string;
}

/** Active microphone / capture endpoints for the "Microphone Source" picker. */
export async function listAudioInputs(): Promise<AudioInputDevice[]> {
  return invoke<AudioInputDevice[]>("list_audio_inputs");
}

/** Mirrors the Rust `AudioOutputDevice` (src-tauri/src/core/audio.rs). */
export interface AudioOutputDevice {
  /** Stable WASAPI render-endpoint id — stored in `AudioDeviceSel.id`. */
  id: string;
  /** Human-friendly name (e.g. "Speakers (Realtek(R) Audio)"). */
  name: string;
}

/** Active render endpoints for the "PC Audio" multi-select (all_pc_audio mode). */
export async function listAudioOutputs(): Promise<AudioOutputDevice[]> {
  return invoke<AudioOutputDevice[]>("list_audio_outputs");
}

/** Mirrors the Rust `AudioSession` (src-tauri/src/core/audio.rs). */
export interface AudioSession {
  /** Owning process id. */
  pid: number;
  /** Executable name (e.g. "Discord.exe") — also the persisted source id. */
  process_name: string;
  /** Session display name, or the process name when the app sets none. */
  display_name: string;
  /** The app's icon as a `data:image/png;base64,...` URL, or null if unreadable. */
  icon: string | null;
}

/**
 * Apps currently playing audio — the live source list for `specific_apps` mode.
 * Poll this (e.g. React Query refetch) so apps appear as they start playing.
 */
export async function listActiveAudioSessions(): Promise<AudioSession[]> {
  return invoke<AudioSession[]>("list_active_audio_sessions");
}

/**
 * Whether Windows per-process loopback (build ≥ 20348) is available. The UI gates
 * the `specific_apps` recording mode on this and falls back to `all_pc_audio`.
 */
export async function processLoopbackSupported(): Promise<boolean> {
  return invoke<boolean>("process_loopback_supported");
}

/** Start capture of a window (HWND) at an optional target FPS / adapter. */
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

/** Reveal a clip's file in the OS file manager (Explorer), selecting it. */
export async function revealClip(id: number): Promise<void> {
  await invoke("reveal_clip", { id });
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

/**
 * One of a clip's audio tracks (mirrors Rust `library::remux::AudioTrackInfo`).
 * `index` is the 0-based position among the file's audio streams — track 0 is
 * the master "All Audio" mix; 1..N are the stems ("Microphone", per-app, …).
 */
export interface AudioTrackInfo {
  index: number;
  name: string;
}

/** The audio tracks in a clip (count + names) for the editor's per-track UI. */
export async function clipAudioTracks(id: number): Promise<AudioTrackInfo[]> {
  return invoke<AudioTrackInfo[]>("clip_audio_tracks", { id });
}

/**
 * Read a byte range `[start, end)` of a clip file as an `ArrayBuffer`. Backs the
 * editor's live per-stem mixer: mediabunny decodes the stems in the webview via
 * a `CustomSource` that pulls bytes over IPC, because it can't `fetch()` the
 * `hakoclip://` streaming scheme (WebView2 blocks cross-scheme fetch by CORS;
 * the `<video>` element is exempt). `end` is clamped to the file size in Rust.
 */
export async function readClipRange(
  id: number,
  start: number,
  end: number,
): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("read_clip_range", { id, start, end });
}

/** A stem selected for export, with its 0–100 volume (mirrors `TrackVolume`). */
export interface TrackVolume {
  index: number;
  volume: number;
}

/**
 * Export a clip to `[start, end)` with its audio being the chosen `tracks`
 * (stems) mixed at their volumes — the editor's per-track mute/solo/volume,
 * applied on export. Empty `tracks` ⇒ video-only; one stem at 100% ⇒ a
 * loss-less stream copy; otherwise the stems are decoded, mixed, and re-encoded
 * to one master track. `"copy"` adds a new clip; `"overwrite"` replaces it.
 */
export async function remuxWithTracks(args: {
  id: number;
  start: number;
  end: number;
  tracks: TrackVolume[];
  mode: TrimMode;
}): Promise<ClipRecord> {
  return invoke<ClipRecord>("remux_with_tracks", {
    id: args.id,
    start: args.start,
    end: args.end,
    tracks: args.tracks,
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

/** A selected render endpoint in `all_pc_audio` mode (Rust `AudioDeviceSel`). */
export interface AudioDeviceSel {
  /** WASAPI render-endpoint id, or "auto" for the system default output. */
  id: string;
  name: string;
  enabled: boolean;
  /** 0..100. */
  volume: number;
}

/** A selected per-app source in `specific_apps` mode (Rust `AudioAppSel`). */
export interface AudioAppSel {
  /** "game" for the game audio, or a process name like "discord.exe". */
  id: string;
  name: string;
  enabled: boolean;
  /** 0..100. */
  volume: number;
}

/**
 * Medal-style "Recording Audio" config (mirrors Rust `AudioConfig`). On configs
 * written before this feature, `Settings.audio` is null and the backend
 * synthesizes one from `capture_audio` + `mic_source`.
 */
export interface AudioConfig {
  /** "all_pc_audio" | "specific_apps". */
  mode: string;
  /** Master mix volume, 0..100. */
  master_volume: number;
  /** Render endpoints captured in all_pc_audio mode. */
  pc_audio: AudioDeviceSel[];
  /** Per-app sources captured in specific_apps mode. */
  apps: AudioAppSel[];
  mic_enabled: boolean;
  /** "off" | "auto" | device id. */
  mic_source: string;
  /** Microphone volume, 0..100. */
  mic_volume: number;
  /** Down-mix the microphone to mono. */
  mic_mono: boolean;
  /** Write each source as its own named audio track (track 0 stays the mix). */
  separate_tracks: boolean;
}

/** The render-endpoint id used for the system default output ("Auto" in Medal). */
export const AUTO_DEVICE = "auto";
/** The `AudioAppSel.id` for the game's own audio in `specific_apps` mode. */
export const GAME_SOURCE_ID = "game";

/**
 * The default `AudioConfig` (mirrors Rust `AudioConfig::default` in settings.rs):
 * all-PC-audio from the default output at full volume, mic off at 50%, single
 * track. Used as the base when seeding the Recording Audio UI.
 */
export function defaultAudioConfig(): AudioConfig {
  return {
    mode: "all_pc_audio",
    master_volume: 100,
    pc_audio: [
      { id: AUTO_DEVICE, name: "Default Output Device", enabled: true, volume: 100 },
    ],
    apps: [],
    mic_enabled: false,
    mic_source: AUTO_DEVICE,
    mic_volume: 50,
    mic_mono: false,
    separate_tracks: false,
  };
}

/**
 * The `AudioConfig` actually in effect for `settings`: the explicit `audio`
 * config if present, else one synthesized from the legacy `capture_audio` +
 * `mic_source` fields. Mirrors Rust `Settings::effective_audio` so the UI shows
 * (and persists from) a config that matches current capture behavior — a legacy
 * user with audio off doesn't see it spuriously enabled.
 */
export function effectiveAudioConfig(settings: Settings): AudioConfig {
  if (settings.audio) return settings.audio;
  const micEnabled =
    settings.mic_source !== "" && settings.mic_source !== "off";
  return {
    ...defaultAudioConfig(),
    mic_enabled: micEnabled,
    mic_source: settings.mic_source,
    // Migrated legacy configs keep unity mic (100), not the new-config 50%.
    mic_volume: 100,
    pc_audio: [
      {
        id: AUTO_DEVICE,
        name: "Default Output Device",
        enabled: settings.capture_audio,
        volume: 100,
      },
    ],
  };
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
