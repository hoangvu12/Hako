import { invoke } from "@tauri-apps/api/core";

import type { Settings } from "./settings";

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
