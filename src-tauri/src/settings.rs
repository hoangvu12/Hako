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
    /// Capture desktop audio + mic into clips.
    pub capture_audio: bool,
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
            save_hotkey: "F9".into(),
            events: EventToggles::default(),
            storage_dir: None,
            capture_mode: "wgc".into(),
        }
    }
}

impl Settings {
    /// True when the user opted into the graphics-hook injection capture path.
    pub fn uses_hook_capture(&self) -> bool {
        self.capture_mode.eq_ignore_ascii_case("hook")
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
