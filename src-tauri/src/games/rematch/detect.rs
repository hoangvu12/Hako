//! Rematch process / window detection.
//!
//! Rematch (Sloclap, Unreal Engine 5) ships no stable window title we can match,
//! but its executable name is fixed, so we detect it by process — `RuntimeClient`
//! (the project is named "Runtime", hence the process and the `…\Runtime\Saved\…`
//! log path). Both the Steam (`-Win64-`) and Game Pass / GDK (`-WinGDK-`) builds
//! are covered.

use crate::core::capture;
use crate::games::process_snapshot;

/// The game process names (Steam + GDK shipping builds).
pub const GAME_PROCESSES: &[&str] = &[
    "RuntimeClient-Win64-Shipping.exe",
    "RuntimeClient-WinGDK-Shipping.exe",
];

/// The game window's HWND if Rematch is running and presenting (for auto-capture).
/// Matched by owning process rather than title (the title isn't dependable).
pub fn find_window() -> Option<i64> {
    capture::find_window_by_process(GAME_PROCESSES)
}

/// Whether the Rematch process is running. Reads the shared, rate-limited process
/// snapshot.
pub fn game_running() -> bool {
    process_snapshot::any_running(GAME_PROCESSES, process_snapshot::DEFAULT_MAX_AGE)
}
