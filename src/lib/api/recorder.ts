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
  /** @deprecated Use `detected_game !== null`. Kept only for back-compat with
   * the serialized Rust struct; no longer read by the UI. */
  valorant_detected: boolean;
  /** Display name of the detected game ("Valorant", "League of Legends",
   * "Rematch"), or null when none is present. Drives the "Now Clipping <game>"
   * titlebar label. */
  detected_game: string | null;
  encoder: string | null;
  buffer_seconds: number;
  message: string;
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
