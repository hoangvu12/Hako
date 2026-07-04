//! Dota 2 process / window detection.
//!
//! Dota 2 (Source 2) ships a single fixed executable, `dota2.exe`, so we detect
//! it by process like CS2.

use crate::core::capture;
use crate::games::process_snapshot;

/// The Dota 2 process name.
pub const GAME_PROCESSES: &[&str] = &["dota2.exe"];

/// The Dota 2 window's HWND if it's running and presenting (for auto-capture).
pub fn find_window() -> Option<i64> {
    capture::find_window_by_process(GAME_PROCESSES)
}

/// Whether the Dota 2 process is running (shared, rate-limited process snapshot).
pub fn game_running() -> bool {
    process_snapshot::any_running(GAME_PROCESSES, process_snapshot::DEFAULT_MAX_AGE)
}
