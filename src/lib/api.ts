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
  /** Managed settings/library state finished hydrating from disk at startup. */
  StateHydrated: "state-hydrated",
  RecorderStatus: "recorder-status",
  CaptureStats: "capture-stats",
  ClipCreated: "clip-created",
  RecorderError: "recorder-error",
  MatchStateChanged: "match-state-changed",
  MatchSummary: "match-summary",
  /** In-game overlay toast, emitted only to the `overlay` window. */
  OverlayNotify: "overlay-notify",
  /** Overlay placement config, emitted to the `overlay` window. */
  OverlayConfig: "overlay-config",
  /** Cloud-upload byte progress: `CloudUploadProgress`. */
  CloudUploadProgress: "cloud-upload-progress",
  /** Cloud-upload status transition: `CloudUploadStatus`. */
  CloudUploadStatus: "cloud-upload-status",
  /** Cloud-download byte progress (re-fetching an evicted clip): `CloudDownloadProgress`. */
  CloudDownloadProgress: "cloud-download-progress",
  /** Cloud-download status transition: `CloudDownloadStatus`. */
  CloudDownloadStatus: "cloud-download-status",
} as const;

/**
 * Mirrors the Rust `ClipRecord` (src-tauri/src/library/db.rs). Also the payload
 * of the `clip-created` event.
 */
/** One event's position inside a clip (mirrors Rust `EventMark`). */
export interface EventMark {
  /** EventKind label, e.g. "Kill", "Ace", "Spike Defused". */
  label: string;
  /** Seconds from the clip's start where the event happened. */
  at: number;
}

export interface ClipRecord {
  id: number;
  path: string;
  title: string;
  /** Headline event (the dominant one when a clip's window merged several). */
  event: string | null;
  /** Every event captured in the clip's window, in time order. Falls back to
   * `[event]` for clips saved before multi-event tracking existed. */
  events: string[];
  /** Per-event positions within the clip (label + offset seconds), for the
   * editor's seek-bar markers. Empty for manual saves and for clips cut before
   * positions were persisted. */
  event_marks: EventMark[];
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

  /** True once cloud retention deleted the local files. The clip is now
   * cloud-only — `path`/`thumb_path` no longer point at real files, so playback
   * falls back to the provider's presigned `remote_url`. */
  evicted: boolean;
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

/**
 * A stem selected for export (mirrors Rust `TrackVolume`): its 0–100 volume and
 * whether to apply offline noise suppression (the mic stem's "noise cancel").
 */
export interface TrackVolume {
  index: number;
  volume: number;
  /** Run RNNoise noise suppression on this stem when re-mixing the export. */
  denoise?: boolean;
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

// ---------------------------------------------------------------------------
// Cloud upload (src-tauri/src/cloud)
// ---------------------------------------------------------------------------

/**
 * Non-secret provider config (mirrors Rust `ProviderKind`, serde-tagged on
 * `kind` with snake_case variants). Secrets (keys / service-account JSON) are
 * passed separately to `cloudAddProvider` and stored in the OS keyring — never
 * here, never in `cloud_providers.json`.
 */
export type ProviderKind =
  | { kind: "s3"; endpoint: string; region: string; bucket: string; prefix: string }
  | { kind: "r2"; account_id: string; bucket: string; prefix: string }
  | { kind: "b2"; bucket: string; bucket_id: string; prefix: string }
  | { kind: "gcs"; bucket: string; prefix: string }
  // Phase 2/3 consumer OAuth clouds. Folder-rooted (the upload engine writes
  // under `folder`); auth is an OAuth refresh token in the keyring, obtained via
  // the `cloudConnect*` flow — never entered by hand.
  | { kind: "gdrive"; folder: string }
  | { kind: "dropbox"; folder: string }
  | { kind: "onedrive"; folder: string };

/** A configured cloud target (mirrors Rust `ProviderConfig`). */
export interface ProviderConfig {
  /** Stable id; used as `provider_id` in `cloud_uploads` + the keyring key. */
  id: string;
  /** User-facing name, e.g. "My R2 clips". */
  label: string;
  kind: ProviderKind;
}

/** Secrets for a provider (mirrors Rust `Secrets`). Only the fields a given
 * `kind` needs are read: S3/R2/B2 use the key pair; GCS uses the JSON. */
export interface ProviderSecrets {
  access_key_id?: string;
  secret_access_key?: string;
  /** GCS service-account JSON (the whole file contents). */
  gcs_credential_json?: string;
  // Consumer OAuth clouds (gdrive/dropbox/onedrive). Written by the
  // `cloudConnect*` flow, not the add-provider form; OpenDAL refreshes the
  // access token from these. Listed for type-parity with the Rust `Secrets`.
  refresh_token?: string;
  client_id?: string;
  client_secret?: string;
}

/** Cloud-upload status values (mirrors Rust `cloud_status`). */
export type CloudUploadState =
  | "queued"
  | "uploading"
  | "done"
  | "error"
  | "canceled";

/** One `cloud_uploads` row (mirrors Rust `CloudUpload`). */
export interface CloudUpload {
  clip_id: number;
  provider_id: string;
  remote_path: string | null;
  remote_url: string | null;
  status: CloudUploadState;
  bytes_sent: number;
  size_bytes: number;
  /** Unix ms; set ONLY on success — the local-eviction gate. */
  uploaded_at: number | null;
  error: string | null;
  updated_at: number;
}

/** Payload of the `cloud-upload-progress` event. */
export interface CloudUploadProgress {
  clip_id: number;
  provider_id: string;
  /** Bytes streamed so far. */
  sent: number;
  /** Total file size in bytes. */
  total: number;
  /** Throughput estimate for the toast (e.g. "12.4 MB/s"). */
  bytes_per_sec: number;
}

/** Payload of the `cloud-upload-status` event. */
export interface CloudUploadStatus {
  clip_id: number;
  provider_id: string;
  status: CloudUploadState;
  error?: string | null;
}

/** Cloud-download lifecycle for an evicted clip being re-fetched to edit. */
export type CloudDownloadState = "downloading" | "done" | "error";

/** Payload of the `cloud-download-progress` event. */
export interface CloudDownloadProgress {
  clip_id: number;
  /** Bytes received so far. */
  received: number;
  /** Total object size in bytes. */
  total: number;
  /** Throughput estimate (bytes/sec). */
  bytes_per_sec: number;
}

/** Payload of the `cloud-download-status` event. */
export interface CloudDownloadStatus {
  clip_id: number;
  status: CloudDownloadState;
  error?: string | null;
}

/** Configured cloud providers (no secrets). */
export async function cloudListProviders(): Promise<ProviderConfig[]> {
  return invoke<ProviderConfig[]>("cloud_list_providers");
}

/** Add a provider: persists the config to `cloud_providers.json` and its
 * secrets to the OS keyring. Returns the stored config (with its assigned id). */
export async function cloudAddProvider(
  config: ProviderConfig,
  secrets: ProviderSecrets,
): Promise<ProviderConfig> {
  return invoke<ProviderConfig>("cloud_add_provider", { config, secrets });
}

/** Remove a provider (config + keyring secrets). */
export async function cloudRemoveProvider(id: string): Promise<void> {
  await invoke("cloud_remove_provider", { id });
}

/** Test connectivity/credentials for a configured provider (`op.check()`). */
export async function cloudTestProvider(id: string): Promise<void> {
  await invoke("cloud_test_provider", { id });
}

/** Which consumer cloud to connect via OAuth (mirrors the Rust connect commands). */
export type OAuthProviderKind = "gdrive" | "dropbox" | "onedrive";

/**
 * Connect a consumer cloud (Google Drive / Dropbox / OneDrive) via OAuth. Opens
 * the system browser for consent; on success the refresh token is stored in the
 * OS keyring and the new provider config is returned (it then behaves like any
 * other provider). `folder` defaults to `/Hako`. The promise resolves only after
 * the user finishes the browser consent (or rejects on cancel/timeout/error).
 */
export async function cloudConnectOAuth(
  kind: OAuthProviderKind,
  folder?: string,
  label?: string,
): Promise<ProviderConfig> {
  const command =
    kind === "gdrive"
      ? "cloud_connect_gdrive"
      : kind === "dropbox"
        ? "cloud_connect_dropbox"
        : "cloud_connect_onedrive";
  return invoke<ProviderConfig>(command, {
    folder: folder ?? null,
    label: label ?? null,
  });
}

/** Enqueue a clip for upload. `providerId` defaults to `cloud_default_provider`. */
export async function cloudUploadClip(
  clipId: number,
  providerId?: string,
): Promise<void> {
  await invoke("cloud_upload_clip", { clipId, providerId: providerId ?? null });
}

/** Cancel an in-flight or queued upload for a clip. */
export async function cloudCancelUpload(clipId: number): Promise<void> {
  await invoke("cloud_cancel_upload", { clipId });
}

/** Cloud-upload rows for one clip, or all rows when `clipId` is omitted. */
export async function cloudUploadStatus(clipId?: number): Promise<CloudUpload[]> {
  return invoke<CloudUpload[]>("cloud_upload_status", { clipId: clipId ?? null });
}

/** Re-download an evicted clip's file from the cloud so it can be edited locally.
 * Resolves with the refreshed record (`evicted = false`); progress streams over
 * the `cloud-download-progress` event. No-op for a clip that's already local. */
export async function cloudDownloadClip(clipId: number): Promise<ClipRecord> {
  return invoke<ClipRecord>("cloud_download_clip", { clipId });
}

/** Retention gauge / outcome (mirrors Rust `EvictStats`). */
export interface EvictStats {
  /** Local bytes still on disk (non-evicted clips). */
  local_bytes: number;
  /** Clips with local files still on disk. */
  local_count: number;
  /** Configured budget in bytes (`cloud_retention_gb` × 1 GiB). */
  budget_bytes: number;
  /** Bytes reclaimed by a pass (0 for a stats-only probe). */
  freed_bytes: number;
  /** Clips evicted by a pass (0 for a stats-only probe). */
  evicted_count: number;
}

/** Current retention gauge (local usage vs. budget); changes nothing. */
export async function cloudRetentionStats(): Promise<EvictStats> {
  return invoke<EvictStats>("cloud_retention_stats");
}

/** Run a retention pass now ("Free up space"): evict oldest uploaded clips until
 * under budget. Returns what was reclaimed. */
export async function cloudFreeUpSpace(): Promise<EvictStats> {
  return invoke<EvictStats>("cloud_free_up_space");
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
 * Kind of in-game overlay toast (mirrors Rust `OverlayKind`, snake_case serde).
 * Drives the toast's icon + accent color in the overlay window.
 */
export type OverlayKind =
  | "recording_started"
  | "recording_stopped"
  | "clip_saved"
  | "disk_low";

/**
 * One in-game overlay toast (mirrors Rust `OverlayNotice`). Payload of the
 * `overlay-notify` event, consumed only by the `overlay` window.
 */
export interface OverlayNotice {
  kind: OverlayKind;
  title: string;
  subtitle: string | null;
  /** Auto-dismiss after this many ms. */
  ttlMs: number;
}

/**
 * Overlay placement config (mirrors Rust `OverlayConfig`). Payload of the
 * `overlay-config` event; tells the overlay window which corner to stack in.
 */
export interface OverlayConfig {
  position: OverlayPosition;
}

/**
 * Fire a sample overlay toast (Settings → "Test overlay"). Force-shows the
 * overlay over Valorant — or the primary monitor when the game isn't running —
 * so placement and click-through can be verified without launching a match.
 */
export async function overlayTest(): Promise<void> {
  await invoke("overlay_test");
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
