//! App configuration: load/save via serde JSON in the app config dir.
//!
//! `#[serde(default)]` everywhere so older config files load forward-compatibly
//! (missing keys fall back to defaults). The struct mirrors the `/settings`
//! page; field names match `Settings` in `src/lib/api.ts`.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::valorant::model::GameModeToggles;
use crate::valorant::reconcile::{EventTimings, EventToggles};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Target capture FPS (30 / 60 / 120).
    pub target_fps: u32,
    /// Instant-replay buffer depth in seconds.
    pub buffer_seconds: u32,
    /// Where the instant-replay buffer is held: `ram` (default — fast saves, but
    /// the window costs RAM) or `disk` (spool compressed video to rolling segment
    /// files, freeing RAM at the cost of continuous disk writes). Medal's
    /// "Recording buffer" toggle. Anything other than `disk` is treated as `ram`.
    pub buffer_storage: String,
    /// Clip padding before the event, in seconds (auto-clips).
    pub pad_before_secs: u32,
    /// Clip padding after the event, in seconds.
    pub pad_after_secs: u32,
    /// Video codec: `h264` (default/compat) | `hevc` | `av1`.
    pub codec: String,
    /// Target bitrate ceiling in Mbps (generous default).
    pub bitrate_mbps: u32,
    /// Stamp the "tabbed out" freeze card onto frozen frames (game minimized /
    /// alt-tabbed / stale swapchain) so a clip viewer sees an intentional notice
    /// instead of a silently-held frame. On by default.
    pub freeze_overlay: bool,
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
    /// Global hotkey for "save last N seconds" (e.g. `F9`). Accelerator string in
    /// the `global-hotkey` format (modifiers + key joined by `+`, e.g. `Alt+F7`).
    /// Registered live: editing it re-registers the OS shortcut (see `main.rs`).
    pub save_hotkey: String,
    /// How many seconds the save-clip hotkey captures (Medal's CLIPS duration
    /// dropdown). Clamped to `buffer_seconds` at save time — you can't save more
    /// gameplay than the buffer holds.
    pub clip_seconds: u32,
    /// Global hotkey for "long recording" start/stop, shown in the titlebar
    /// RECORDING popover. Persisted (and editable) now; the manual long-recording
    /// capture feature itself is not wired yet, so this is display-only.
    pub long_recording_hotkey: String,
    /// Per-event auto-clip toggles.
    pub events: EventToggles,
    /// Per-event clip windows (before/after seconds) — Outplayed's "Events
    /// timing". When absent (older configs) it loads as the default table; the
    /// auto-clip cut falls back to these per-event pads instead of the single
    /// global `pad_before_secs`/`pad_after_secs` (which still drive manual saves).
    pub event_timings: EventTimings,
    /// What the live Valorant orchestrator captures, Outplayed-style:
    /// - `manual` — never auto-capture matches (buffer + save-hotkey only),
    /// - `highlights` (default) — record the match, cut per-event highlights,
    /// - `full_match` — keep the whole match as a single clip (no cutting),
    /// - `session` — record continuously while the game is open as one clip.
    pub auto_capture_mode: String,
    /// Per-game-mode auto-clip gate, keyed on the live presence `queueId`. A
    /// match whose queue is toggled off is not recorded in the per-match modes
    /// (Highlights / Full match); Session mode is continuous and unaffected.
    /// Defaults to all-on (record every mode), matching the historical behavior.
    pub auto_clip_modes: GameModeToggles,
    /// Where clips are written (null → `<Videos>/Hako`).
    pub storage_dir: Option<String>,
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
    /// Master switch for the in-game overlay toasts. When false, nothing is ever
    /// shown over the game (the per-trigger toggles below are then moot).
    pub overlay_enabled: bool,
    /// Per-trigger toggles, consulted only when `overlay_enabled`.
    /// "Now recording" / "Recording stopped" on capture start/stop.
    pub overlay_on_capture_state: bool,
    /// "Clip saved" toast on a manual F9 / UI save.
    pub overlay_on_clip_saved: bool,
    /// "Storage almost full" toast when the clips drive runs low.
    pub overlay_on_disk_low: bool,
    /// Corner the toast stack sits in over the game:
    /// `top_left` | `top_right` | `bottom_left` | `bottom_right`.
    pub overlay_position: String,

    // --- Cloud upload (see `crate::cloud`) --------------------------------
    // Provider *configs* (buckets/endpoints) and their secrets do NOT live here —
    // configs are in `cloud_providers.json` and secrets in the OS keyring. These
    // are just the behavior toggles. All `#[serde(default)]` via the container
    // attribute above, so older config files load forward-compatibly.
    /// Auto-upload saved clips to [`Settings::cloud_default_provider`] after
    /// they're written. Off by default (opt-in) — no surprise uplink during
    /// matches; the manual per-clip upload action is always available.
    pub cloud_auto_upload: bool,
    /// Provider id (see `cloud::ProviderConfig::id`) used for auto-upload and as
    /// the default target of a manual upload. `None` until the user picks one.
    pub cloud_default_provider: Option<String>,
    /// Local-cache budget for "free up space": once cloud-backed clips on disk
    /// exceed this, the oldest are evicted (kept in the cloud). Gibibytes.
    pub cloud_retention_gb: u64,
    /// Master switch for the retention worker. Off by default — eviction only
    /// runs on opt-in, and never touches a clip that isn't safely in the cloud.
    pub cloud_free_up_space_enabled: bool,
    /// Send evicted files to the Recycle Bin (recoverable) rather than hard-
    /// deleting. On by default, matching Medal's `filesToRecycleBin`.
    pub cloud_delete_to_recycle_bin: bool,

    // --- First-run onboarding --------------------------------------------
    /// Whether the user has finished (or skipped) the first-run setup wizard.
    /// The wizard shows while this is false.
    ///
    /// Falls back to `false` when absent (via the container `#[serde(default)]`),
    /// so anyone who hasn't completed onboarding — a fresh install *or* a config
    /// written before this field existed — gets the wizard. Once finished or
    /// skipped it's persisted as `true` and never shows again.
    pub onboarding_completed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            target_fps: 60,
            buffer_seconds: 120,
            buffer_storage: "ram".into(),
            pad_before_secs: 8,
            pad_after_secs: 4,
            codec: "h264".into(),
            bitrate_mbps: 20,
            freeze_overlay: true,
            capture_audio: true,
            mic_source: "auto".into(),
            audio: None,
            save_hotkey: "F9".into(),
            clip_seconds: 30,
            long_recording_hotkey: "Alt+F7".into(),
            events: EventToggles::default(),
            event_timings: EventTimings::default(),
            auto_capture_mode: "highlights".into(),
            auto_clip_modes: GameModeToggles::default(),
            storage_dir: None,
            quality_preset: "custom".into(),
            resolution: "native".into(),
            gpu_adapter: -1,
            video_encoder: "gpu".into(),
            overlay_enabled: true,
            overlay_on_capture_state: true,
            overlay_on_clip_saved: true,
            overlay_on_disk_low: true,
            overlay_position: "top_right".into(),
            cloud_auto_upload: false,
            cloud_default_provider: None,
            cloud_retention_gb: 5,
            cloud_free_up_space_enabled: false,
            cloud_delete_to_recycle_bin: true,
            // Fresh installs (no settings.json) hit this and run the wizard;
            // existing configs deserialize the missing key as `true` instead.
            onboarding_completed: false,
        }
    }
}

/// What the live Valorant orchestrator captures (Outplayed's "Capture mode").
/// The string form is persisted in [`Settings::auto_capture_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCaptureMode {
    /// Never auto-record matches — buffer + save-hotkey only.
    Manual,
    /// Record the match and cut per-event highlights (the default).
    Highlights,
    /// Keep the whole match as a single clip; no highlight cutting.
    FullMatch,
    /// Record continuously while the game is open as one clip.
    Session,
}

impl AutoCaptureMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "manual" => AutoCaptureMode::Manual,
            "full_match" | "fullmatch" => AutoCaptureMode::FullMatch,
            "session" | "full_session" => AutoCaptureMode::Session,
            // "highlights" and any unknown value → the historical behavior.
            _ => AutoCaptureMode::Highlights,
        }
    }

    /// Whether this mode records a per-match session (Highlights or FullMatch).
    pub fn records_match(self) -> bool {
        matches!(self, AutoCaptureMode::Highlights | AutoCaptureMode::FullMatch)
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

impl AudioConfig {
    /// True when `self` and `other` capture the same inputs and produce the same
    /// output-track layout — they differ (if at all) only in volume levels.
    /// Medal gates its live `AudioCaptureVolume` update on exactly this: a layout
    /// change (mic on/off, device add/remove, separate-tracks) forces a recording
    /// restart, whereas a level-only change is hot-applied.
    pub fn structure_eq(&self, other: &AudioConfig) -> bool {
        self.mode == other.mode
            && self.mic_enabled == other.mic_enabled
            && self.mic_source == other.mic_source
            && self.mic_mono == other.mic_mono
            && self.separate_tracks == other.separate_tracks
            && devices_structure_eq(&self.pc_audio, &other.pc_audio)
            && apps_structure_eq(&self.apps, &other.apps)
    }

    /// True when the configs differ *only* in volume levels (master / per-source
    /// / mic) — safe to apply to a running capture without a restart.
    pub fn differs_only_in_volume(&self, other: &AudioConfig) -> bool {
        self.structure_eq(other) && self != other
    }
}

/// Render-endpoint selections match structurally when the same ids are enabled
/// in the same order — the device *name* is cosmetic and `volume` is the level
/// we hot-apply, so neither affects the track layout.
fn devices_structure_eq(a: &[AudioDeviceSel], b: &[AudioDeviceSel]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id == y.id && x.enabled == y.enabled)
}

/// App selections match structurally when the same ids/names are enabled in the
/// same order. `name` matters here because it labels the per-app stem track, so
/// a rename changes the output layout; only `volume` is hot-applicable.
fn apps_structure_eq(a: &[AudioAppSel], b: &[AudioAppSel]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.id == y.id && x.enabled == y.enabled && x.name == y.name)
}

/// A selected render endpoint in `all_pc_audio` mode (Medal `AudioModeDevice`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// True when the instant-replay buffer should be spooled to disk rather than
    /// held in RAM (Medal's "Recording buffer: Disk"). Anything other than `disk`
    /// means RAM.
    pub fn buffers_to_disk(&self) -> bool {
        self.buffer_storage.eq_ignore_ascii_case("disk")
    }

    /// The live-capture auto mode, normalized. Unknown values fall back to
    /// `highlights` (the historical behavior).
    pub fn auto_mode(&self) -> AutoCaptureMode {
        AutoCaptureMode::parse(&self.auto_capture_mode)
    }

    /// Seconds the save-clip hotkey should capture: the configured `clip_seconds`
    /// clamped to what the buffer actually holds (`buffer_seconds`), and never
    /// zero. You can't save more gameplay than is buffered.
    pub fn clip_capture_seconds(&self) -> u32 {
        self.clip_seconds.clamp(1, self.buffer_seconds.max(1))
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

    /// Whether the *video* side of what a running capture snapshots at start
    /// differs (fps, buffer, codec/bitrate/resolution, GPU). Audio is handled
    /// separately: a volume-only audio change applies live (no restart), while
    /// an audio *structure* change is folded into the restart decision by the
    /// caller (see `commands::update_settings`). Mirrors Medal, which restarts
    /// on `VideoEncoderProperties` / `VideoOutputResolution` but hot-applies
    /// audio volume.
    pub fn video_capture_config_differs(&self, other: &Settings) -> bool {
        self.target_fps != other.target_fps
            || self.buffer_seconds != other.buffer_seconds
            || self.buffer_storage != other.buffer_storage
            || self.codec != other.codec
            || self.bitrate_mbps != other.bitrate_mbps
            || self.resolution != other.resolution
            || self.gpu_adapter != other.gpu_adapter
            || self.video_encoder != other.video_encoder
    }

    /// Whether anything a running capture snapshots at start differs between
    /// `self` and `other` — i.e. a change that only takes effect once capture is
    /// restarted (fps, buffer, codec/bitrate/resolution, GPU, and the whole audio
    /// config incl. mic). The settings path uses this to hot-restart the live
    /// buffer so toggles like "enable microphone" apply without relaunching.
    pub fn capture_config_differs(&self, other: &Settings) -> bool {
        self.target_fps != other.target_fps
            || self.buffer_seconds != other.buffer_seconds
            || self.buffer_storage != other.buffer_storage
            || self.codec != other.codec
            || self.bitrate_mbps != other.bitrate_mbps
            || self.resolution != other.resolution
            || self.gpu_adapter != other.gpu_adapter
            || self.video_encoder != other.video_encoder
            || self.capture_audio != other.capture_audio
            || self.mic_source != other.mic_source
            || self.effective_audio() != other.effective_audio()
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

    #[test]
    fn audio_volume_change_is_live_not_restart() {
        // A pure level change (master / per-device / mic) is hot-applicable.
        let base = AudioConfig::default();
        let mut louder = base.clone();
        louder.master_volume = 50;
        assert!(base.structure_eq(&louder));
        assert!(base.differs_only_in_volume(&louder));

        let mut quieter_mic = base.clone();
        quieter_mic.mic_volume = 10;
        assert!(base.differs_only_in_volume(&quieter_mic));

        let mut dev_vol = base.clone();
        if let Some(d) = dev_vol.pc_audio.first_mut() {
            d.volume = 25;
        }
        assert!(base.differs_only_in_volume(&dev_vol));
    }

    #[test]
    fn audio_structure_change_forces_restart() {
        // Layout changes (mic on/off, separate-tracks, device add/remove, mode)
        // are NOT volume-only — they require a capture restart, like Medal.
        let base = AudioConfig::default();

        let mut mic_on = base.clone();
        mic_on.mic_enabled = true;
        assert!(!base.structure_eq(&mic_on));
        assert!(!base.differs_only_in_volume(&mic_on));

        let mut stems = base.clone();
        stems.separate_tracks = true;
        assert!(!base.differs_only_in_volume(&stems));

        let mut extra_dev = base.clone();
        extra_dev.pc_audio.push(AudioDeviceSel::default());
        assert!(!base.differs_only_in_volume(&extra_dev));

        // Identical configs are neither a restart nor a (no-op) live push.
        assert!(base.structure_eq(&base.clone()));
        assert!(!base.differs_only_in_volume(&base.clone()));
    }
}
