//! War Thunder process / window detection.
//!
//! War Thunder's client is a single fixed executable (`aces.exe`) regardless of
//! launcher (Steam / standalone / Gaijin), so we detect it by process like the
//! other integrations. The web-HUD server (`http://127.0.0.1:8111`) the
//! integration polls is only up while the game runs.

use crate::core::capture;
use crate::games::process_snapshot;

/// The War Thunder client process name (the "Aces" engine executable).
pub const GAME_PROCESSES: &[&str] = &["aces.exe"];

/// The War Thunder window's HWND if it's running and presenting (for auto-capture).
pub fn find_window() -> Option<i64> {
    capture::find_window_by_process(GAME_PROCESSES)
}

/// Whether the War Thunder process is running (shared, rate-limited snapshot).
pub fn game_running() -> bool {
    process_snapshot::any_running(GAME_PROCESSES, process_snapshot::DEFAULT_MAX_AGE)
}
