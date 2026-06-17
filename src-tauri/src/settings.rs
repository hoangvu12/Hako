//! App configuration: load/save via serde JSON in the app config dir.
//!
//! `#[serde(default)]` everywhere so older config files load forward-compatibly
//! (missing keys fall back to defaults). The struct mirrors the `/settings`
//! page; field names match `Settings` in `src/lib/api.ts`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::valorant::reconcile::EventToggles;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Target capture FPS (30 / 60 / 120).
    pub target_fps: u32,
    /// RAM ring buffer length in seconds (instant-replay depth).
    pub buffer_seconds: u32,
    /// Clip padding before the event, in seconds (auto-clips).
    pub pad_before_secs: u32,
    /// Clip padding after the event, in seconds.
    pub pad_after_secs: u32,
    /// Video codec: `h264` (default/compat) | `hevc` | `av1`.
    pub codec: String,
    /// Target bitrate ceiling in Mbps (generous default).
    pub bitrate_mbps: u32,
    /// Capture desktop (loopback) audio into clips ("Audio Source": All PC
    /// audio vs Off).
    pub capture_audio: bool,
    /// Which microphone to mix in, independent of desktop audio: `"off"`,
    /// `"auto"` (system default), or a specific WASAPI capture-endpoint id.
    /// See [`crate::core::audio::MicSource`].
    ///
    /// Legacy single-track field, retained for back-compat with the titlebar
    /// popover and old config files. The richer per-source model lives in
    /// [`Settings::audio`]; when that's absent it's synthesized from this +
    /// `capture_audio` (see [`Settings::effective_audio`]).
    pub mic_source: String,
    /// Medal-style "Recording Audio" config: recording mode, per-source enable +
    /// volume, microphone, and the separate-tracks toggle. `None` on configs
    /// written before this feature existed — [`Settings::effective_audio`] then
    /// synthesizes it from `capture_audio` + `mic_source` so old installs keep
    /// their exact behavior.
    pub audio: Option<AudioConfig>,
    /// Global hotkey for "save last N seconds" (e.g. `F9`).
    pub save_hotkey: String,
    /// Per-event auto-clip toggles.
    pub events: EventToggles,
    /// Where clips are written (null → `<Videos>/Hako`).
    pub storage_dir: Option<String>,
    /// Capture backend: `wgc` (default, Vanguard-safe, capped at the DWM
    /// composition rate) or `hook` (opt-in graphics-hook injection that beats the
    /// cap at the cost of anti-cheat risk — see `core::hook`). Anything other than
    /// `hook` is treated as `wgc`.
    pub capture_mode: String,
    /// Which Medal-style quality preset the UI shows as selected: `low` |
    /// `standard` | `high` | `custom`. Purely cosmetic — the concrete knobs
    /// (`resolution`, `target_fps`, `bitrate_mbps`, `codec`) are the source of
    /// truth; selecting a preset just writes those. Defaults to `custom` so
    /// pre-feature configs (which set the knobs directly) aren't mislabeled.
    pub quality_preset: String,
    /// Output resolution cap: `native` (no scaling — the historical behavior) or
    /// a named target (`360p` | `480p` | `720p` | `1080p` | `1440p` | `2160p`).
    /// When a target is set, the captured frame is downscaled on-GPU to fit the
    /// target box **by height, never upscaling** (see [`Settings::resolution_dims`]
    /// and the encode thread in `core::capture`).
    pub resolution: String,
    /// Which GPU to capture/encode on: `-1` = Auto (the display-owning adapter),
    /// else a DXGI adapter index (see `core::device` / `gpu_info`). Used as the
    /// fallback adapter when `start_capture` isn't given an explicit one.
    pub gpu_adapter: i32,
    /// Video encoder backend the UI shows: currently only `gpu` (hardware
    /// NVENC/QSV) is implemented; persisted so the dropdown round-trips and a
    /// future `cpu` (software x264/x265) path can slot in without a migration.
    pub video_encoder: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_fps: 60,
            buffer_seconds: 120,
            pad_before_secs: 8,
            pad_after_secs: 4,
            codec: "h264".into(),
            bitrate_mbps: 20,
            capture_audio: true,
            mic_source: "auto".into(),
            audio: None,
            save_hotkey: "F9".into(),
            events: EventToggles::default(),
            storage_dir: None,
            capture_mode: "wgc".into(),
            quality_preset: "custom".into(),
            resolution: "native".into(),
            gpu_adapter: -1,
            video_encoder: "gpu".into(),
        }
    }
}

/// The literal device id meaning "the system default render endpoint" (Medal's
/// `"Auto"`). Resolved to the real default loopback device at capture time.
pub const AUTO_DEVICE: &str = "auto";
/// The synthetic source id for the game itself ("Game Audio" — the Valorant
/// process) in `specific_apps` mode. Medal uses `"game-audio"`.
pub const GAME_SOURCE_ID: &str = "game";

/// Medal-style "Recording Audio" config (mirrors `MedalEncoder/AudioModeConfig`).
///
/// Two recording modes share most fields:
/// - `all_pc_audio` — capture system loopback from one or more render endpoints
///   (`pc_audio`), plus an optional microphone.
/// - `specific_apps` — per-process loopback of selected apps (`apps`), plus an
///   optional microphone. Requires Windows build ≥ 20348; the capture core falls
///   back to `all_pc_audio` when unsupported.
///
/// `separate_tracks` is Medal's "Separate audio tracks" toggle: when on, the
/// clip gets a master "All Audio" mix (track 0) *plus* one named stem per source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// `"all_pc_audio"` | `"specific_apps"`.
    pub mode: String,
    /// Master mix volume, 0..100 (applied to the loopback/app inputs in the
    /// master "All Audio" track).
    pub master_volume: u8,
    /// Render endpoints to capture in `all_pc_audio` mode.
    pub pc_audio: Vec<AudioDeviceSel>,
    /// Per-app sources to capture in `specific_apps` mode.
    pub apps: Vec<AudioAppSel>,
    /// Whether the microphone is mixed in.
    pub mic_enabled: bool,
    /// Which mic: `"off"`, `"auto"` (default capture endpoint), or a device id.
    /// Mirrors the legacy `Settings::mic_source` string.
    pub mic_source: String,
    /// Microphone volume, 0..100 (Medal's `MicSoundGain`, sent as 0–100).
    pub mic_volume: u8,
    /// Down-mix the microphone to mono before mixing (Medal's `MonoMicAudio`).
    pub mic_mono: bool,
    /// Write each source to its own named audio track (Medal's
    /// `MultipleAudioTracks`). Track 0 is always the master "All Audio" mix.
    pub separate_tracks: bool,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            mode: "all_pc_audio".into(),
            master_volume: 100,
            pc_audio: vec![AudioDeviceSel {
                id: AUTO_DEVICE.into(),
                name: "Default Output Device".into(),
                enabled: true,
                volume: 100,
            }],
            apps: Vec::new(),
            mic_enabled: false,
            mic_source: AUTO_DEVICE.into(),
            mic_volume: 50,
            mic_mono: false,
            separate_tracks: false,
        }
    }
}

/// A selected render endpoint in `all_pc_audio` mode (Medal `AudioModeDevice`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioDeviceSel {
    /// Stable WASAPI render-endpoint id, or [`AUTO_DEVICE`] for the default.
    pub id: String,
    /// Friendly name for the UI (e.g. "Speakers (Realtek(R) Audio)").
    pub name: String,
    pub enabled: bool,
    /// 0..100.
    pub volume: u8,
}

impl Default for AudioDeviceSel {
    fn default() -> Self {
        Self {
            id: AUTO_DEVICE.into(),
            name: String::new(),
            enabled: true,
            volume: 100,
        }
    }
}

/// A selected per-app source in `specific_apps` mode (Medal `AudioModeSource`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioAppSel {
    /// [`GAME_SOURCE_ID`] for the game, or a process name like `"discord.exe"`.
    pub id: String,
    /// Friendly name for the UI (e.g. "Game Audio", "Discord").
    pub name: String,
    pub enabled: bool,
    /// 0..100.
    pub volume: u8,
}

impl Default for AudioAppSel {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            volume: 100,
        }
    }
}

impl Settings {
    /// True when the user opted into the graphics-hook injection capture path.
    pub fn uses_hook_capture(&self) -> bool {
        self.capture_mode.eq_ignore_ascii_case("hook")
    }

    /// The output-resolution target box for [`Settings::resolution`], or `None`
    /// for native capture (no scaling). The encode thread fits the captured frame
    /// into this box by height, never upscaling — so this is a *cap*, not a forced
    /// size. The (width, height) pairs mirror Medal's `ResolutionHandler` table.
    pub fn resolution_dims(&self) -> Option<(u32, u32)> {
        match self.resolution.trim().to_ascii_lowercase().as_str() {
            "360p" => Some((640, 360)),
            "480p" => Some((854, 480)),
            "720p" => Some((1280, 720)),
            "1080p" => Some((1920, 1080)),
            "1440p" => Some((2560, 1440)),
            "2160p" => Some((3840, 2160)),
            // "native" and any unknown value → no scaling (capture at source size).
            _ => None,
        }
    }

    /// The fallback capture/encode adapter index for this config: `None` when
    /// `gpu_adapter` is Auto (`< 0`), else the DXGI adapter index.
    pub fn gpu_adapter_index(&self) -> Option<u32> {
        (self.gpu_adapter >= 0).then_some(self.gpu_adapter as u32)
    }

    /// The effective [`AudioConfig`] for capture: the explicit `audio` config if
    /// present, else one synthesized from the legacy `capture_audio` +
    /// `mic_source` fields so configs written before this feature (and the
    /// titlebar popover, which still drives those two fields) keep their exact
    /// single-track behavior — `all_pc_audio` from the default endpoint, mic per
    /// `mic_source`, no separate tracks.
    pub fn effective_audio(&self) -> AudioConfig {
        if let Some(cfg) = &self.audio {
            return cfg.clone();
        }
        let mic_enabled = !matches!(self.mic_source.as_str(), "" | "off");
        AudioConfig {
            mic_enabled,
            mic_source: self.mic_source.clone(),
            // Preserve the historical single-track behavior exactly: desktop +
            // mic both at unity (Medal's 50% mic default applies only to new
            // configs created through the UI, not to migrated legacy ones).
            mic_volume: 100,
            pc_audio: vec![AudioDeviceSel {
                id: AUTO_DEVICE.into(),
                name: "Default Output Device".into(),
                enabled: self.capture_audio,
                volume: 100,
            }],
            ..AudioConfig::default()
        }
    }
}

impl Settings {
    /// `settings.json` inside the given config directory.
    pub fn file_in(config_dir: &Path) -> PathBuf {
        config_dir.join("settings.json")
    }

    /// Load from `path`, falling back to defaults if missing/unreadable/invalid
    /// (settings should never block startup).
    pub fn load(path: &Path) -> Settings {
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("settings parse failed ({e}); using defaults");
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    /// Persist to `path` (creates the parent dir).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("write settings: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = std::env::temp_dir().join("hako_settings_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = Settings::file_in(&dir);
        let _ = std::fs::remove_file(&path);

        let mut s = Settings::default();
        s.target_fps = 120;
        s.codec = "hevc".into();
        s.events.kill = true;
        s.save(&path).unwrap();

        let loaded = Settings::load(&path);
        assert_eq!(loaded.target_fps, 120);
        assert_eq!(loaded.codec, "hevc");
        assert!(loaded.events.kill);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_defaults() {
        let s = Settings::load(Path::new("C:/nonexistent/hako/settings.json"));
        assert_eq!(s.target_fps, 60);
        assert_eq!(s.save_hotkey, "F9");
    }
}
