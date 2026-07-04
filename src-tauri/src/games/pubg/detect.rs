//! PUBG process / window detection.
//!
//! PUBG's client is a single fixed executable (`TslGame.exe` — the game's
//! internal "TslGame" project name), so we detect it by process like the other
//! integrations.

use crate::core::capture;
use crate::games::process_snapshot;

/// The PUBG client process name.
pub const GAME_PROCESSES: &[&str] = &["tslgame.exe"];

/// The PUBG window's HWND if it's running and presenting (for auto-capture).
pub fn find_window() -> Option<i64> {
    capture::find_window_by_process(GAME_PROCESSES)
}

/// Whether the PUBG process is running (shared, rate-limited process snapshot).
pub fn game_running() -> bool {
    process_snapshot::any_running(GAME_PROCESSES, process_snapshot::DEFAULT_MAX_AGE)
}
