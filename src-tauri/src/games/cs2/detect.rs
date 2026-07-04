//! Counter-Strike 2 process / window detection.
//!
//! CS2 ships a single fixed executable name (`cs2.exe`), so we detect it by
//! process like Rematch. The window title ("Counter-Strike 2") is stable too,
//! but matching by owning process is simpler and consistent with the other
//! Steam integrations.

use crate::core::capture;
use crate::games::process_snapshot;

/// The CS2 process name (Source 2 shipping build).
pub const GAME_PROCESSES: &[&str] = &["cs2.exe"];

/// The CS2 window's HWND if it's running and presenting (for auto-capture).
pub fn find_window() -> Option<i64> {
    capture::find_window_by_process(GAME_PROCESSES)
}

/// Whether the CS2 process is running (shared, rate-limited process snapshot).
pub fn game_running() -> bool {
    process_snapshot::any_running(GAME_PROCESSES, process_snapshot::DEFAULT_MAX_AGE)
}
