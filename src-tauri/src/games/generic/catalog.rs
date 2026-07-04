//! The catalog oracle: "is this running process a game, and what's it called?"
//!
//! Phase 1 resolves against the **user-added custom list** only (Steam scan and
//! the curated bundle land in Phases 2–3). Every source excludes the three smart
//! games (they own the arbiter) and a blacklist of obvious non-games, so the
//! generic bucket never fights a smart integration or records a browser.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::commands::{LibraryState, SettingsState};
use crate::core::capture;
use crate::games::process_snapshot;

use super::{curated, steam, DetectedGame, GameSource};

/// Freshness for the detection process-table reads (the shared name-only snapshot
/// coalesces concurrent callers onto one refresh — see [`process_snapshot`]).
const SNAPSHOT_AGE: Duration = Duration::from_secs(2);

/// The three smart games' process names (lowercase). Excluded from every generic
/// source so a smart game is never also detected as "other". Mirrors the per-game
/// detection constants (`valorant::service`, `games::lol::detect`,
/// `games::rematch::detect`).
const SMART_GAME_PROCESSES: &[&str] = &[
    "valorant-win64-shipping.exe",
    "league of legends.exe",
    "runtimeclient-win64-shipping.exe",
    "runtimeclient-wingdk-shipping.exe",
    "cs2.exe",
    "dota2.exe",
    "aces.exe",
    "tslgame.exe",
];

/// Non-games we never auto-record even if a user pointed the picker at one, and
/// the guard for the (future) curated/Steam sources: browsers, chat, capture
/// tools, the shell, game launchers, and Hako itself. Lowercase exe names.
const BLACKLIST: &[&str] = &[
    // Browsers
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "opera.exe",
    "opera_gx.exe",
    "brave.exe",
    "vivaldi.exe",
    // Chat / voice
    "discord.exe",
    "discordptb.exe",
    "discordcanary.exe",
    // Capture / streaming tools
    "obs64.exe",
    "obs32.exe",
    "obs.exe",
    "medal.exe",
    "streamlabs obs.exe",
    "xsplit.core.exe",
    // Shell / OS
    "explorer.exe",
    "taskmgr.exe",
    "applicationframehost.exe",
    "searchhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "textinputhost.exe",
    "systemsettings.exe",
    "dwm.exe",
    // Game launchers (the game itself launches from under these — never the
    // launcher window)
    "steam.exe",
    "steamwebhelper.exe",
    "epicgameslauncher.exe",
    "battle.net.exe",
    "riotclientservices.exe",
    "riotclientux.exe",
    "galaxyclient.exe",
    "eadesktop.exe",
    "origin.exe",
    "ubisoftconnect.exe",
    "upc.exe",
];

/// Hako's own executable name (lowercase), so we never detect ourselves.
fn own_exe_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
            })
            .unwrap_or_default()
    })
}

/// Whether a (lowercase) process name is excluded from generic detection — a
/// smart game, a blacklisted non-game, or Hako itself.
pub fn is_excluded(process_name: &str) -> bool {
    SMART_GAME_PROCESSES.contains(&process_name)
        || BLACKLIST.contains(&process_name)
        || process_name == own_exe_name()
}

/// Scan once: the highest-priority unknown game currently running with a visible,
/// non-minimized main window, or `None` if nothing matches. Sources are consulted
/// in priority order custom → curated → steam (a user's explicit entry wins over
/// the bundled list, which wins over a generic Steam-path resolution).
pub fn detect_generic_game(app: &AppHandle) -> Option<DetectedGame> {
    detect_custom(app)
        .or_else(|| detect_curated(app))
        .or_else(|| detect_steam(app))
}

/// Custom-list source: the first enabled, non-excluded custom game whose process
/// is running and whose window we can find. Its `display_name` is the real title.
fn detect_custom(app: &AppHandle) -> Option<DetectedGame> {
    let customs = {
        let lib = app.try_state::<LibraryState>()?;
        let guard = lib.0.lock().ok()?;
        guard.enabled_custom_games().ok()?
    };
    for cg in &customs {
        // `process_name` is stored lowercase by `add_custom_game`.
        let pn = cg.process_name.as_str();
        if is_excluded(pn) {
            continue;
        }
        // Cheap presence check on the shared snapshot before the EnumWindows sweep.
        if !process_snapshot::any_running(&[pn], SNAPSHOT_AGE) {
            continue;
        }
        if let Some(hwnd) = capture::find_window_by_process(&[pn]) {
            return Some(DetectedGame {
                hwnd,
                name: cg.display_name.clone(),
                process_name: cg.process_name.clone(),
                source: GameSource::Custom,
            });
        }
    }
    None
}

/// Curated source: the first running, non-excluded process whose exe is in the
/// bundled `games.json`. Matches by exe name only (no paths), like the custom
/// source — so it catches non-Steam launcher games (Epic / Riot / Battle.net /
/// miHoYo / …) that the Steam scan can't. Gated on `OtherGamesSettings.detect_curated`.
fn detect_curated(app: &AppHandle) -> Option<DetectedGame> {
    if !detect_curated_enabled(app) {
        return None;
    }
    for name in process_snapshot::running_names(SNAPSHOT_AGE) {
        if is_excluded(&name) {
            continue;
        }
        let Some(display) = curated::curated_name(&name) else {
            continue;
        };
        if let Some(hwnd) = capture::find_window_by_process(&[name.as_str()]) {
            return Some(DetectedGame {
                hwnd,
                name: display,
                process_name: name,
                source: GameSource::Curated,
            });
        }
    }
    None
}

/// Whether the "Detect known games" toggle is on. Defaults to off if settings
/// can't be read (a failure never triggers an unexpected scan).
fn detect_curated_enabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.other.detect_curated))
        .unwrap_or(false)
}

/// Per-pid cache of resolved Steam names, so each game's `.acf` is read once per
/// launch rather than every tick (Medal caches per-pid too). Only successful Steam
/// resolutions are stored (non-Steam processes are rejected by a cheap, fs-free
/// path check first), so the map stays small — a handful of running games. Dead
/// pids from exited games linger harmlessly until the process restarts (a reused
/// pid at worst re-resolves).
static STEAM_CACHE: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// Steam source: the first running, non-excluded process whose exe lives under a
/// `steamapps\common\` library and whose window we can find. The real title is
/// resolved locally from the matching `appmanifest_*.acf`. Gated on
/// `OtherGamesSettings.detect_steam` — bails before touching the (heavier) path
/// table when the toggle is off.
fn detect_steam(app: &AppHandle) -> Option<DetectedGame> {
    if !detect_steam_enabled(app) {
        return None;
    }
    for (pid, name, path) in
        process_snapshot::processes_with_paths(process_snapshot::PATHS_MAX_AGE)
    {
        if is_excluded(&name) {
            continue;
        }
        let Some(game_name) = resolve_steam_cached(pid, &path) else {
            continue;
        };
        if let Some(hwnd) = capture::find_window_by_process(&[name.as_str()]) {
            return Some(DetectedGame {
                hwnd,
                name: game_name,
                process_name: name,
                source: GameSource::Steam,
            });
        }
    }
    None
}

/// Resolve `exe`'s Steam display name, memoized per pid. Returns `None` for a
/// non-Steam exe (cheap path check, no fs); reads the `.acf` only on a cache miss.
fn resolve_steam_cached(pid: u32, exe: &Path) -> Option<String> {
    // Cheap, fs-free gate: reject non-`steamapps\common\` exes before locking or
    // touching the cache, so the map only ever holds real Steam games.
    let (steamapps, installdir) = steam::steam_library_from_exe(exe)?;
    if let Ok(mut guard) = STEAM_CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if let Some(hit) = map.get(&pid) {
            return Some(hit.clone());
        }
        let name = steam::resolve_steam_name(&steamapps, &installdir);
        map.insert(pid, name.clone());
        return Some(name);
    }
    Some(steam::resolve_steam_name(&steamapps, &installdir))
}

/// Whether the "Detect Steam games automatically" toggle is on. Defaults to off if
/// settings can't be read, so a failure never triggers an unexpected scan.
fn detect_steam_enabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.other.detect_steam))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_smart_games_and_non_games() {
        // Smart games are never "other".
        assert!(is_excluded("valorant-win64-shipping.exe"));
        assert!(is_excluded("league of legends.exe"));
        assert!(is_excluded("runtimeclient-win64-shipping.exe"));
        assert!(is_excluded("cs2.exe"));
        assert!(is_excluded("dota2.exe"));
        assert!(is_excluded("aces.exe"));
        assert!(is_excluded("tslgame.exe"));
        // Blacklisted non-games (browser / launcher / shell).
        assert!(is_excluded("chrome.exe"));
        assert!(is_excluded("steam.exe"));
        assert!(is_excluded("explorer.exe"));
        // A real game exe is not excluded.
        assert!(!is_excluded("gta5.exe"));
        assert!(!is_excluded("eldenring.exe"));
    }
}
