//! Rematch process / window detection.
//!
//! Rematch (Sloclap, Unreal Engine 5) ships no stable window title we can match,
//! but its executable name is fixed, so we detect it by process — `RuntimeClient`
//! (the project is named "Runtime", hence the process and the `…\Runtime\Saved\…`
//! log path). Both the Steam (`-Win64-`) and Game Pass / GDK (`-WinGDK-`) builds
//! are covered.

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::core::capture;

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

/// Whether the Rematch process is running. Cheap process-name-only refresh
/// (matches `lol::detect::game_running`).
pub fn game_running() -> bool {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes().values().any(|p| {
        p.name()
            .to_str()
            .map(|n| GAME_PROCESSES.iter().any(|w| n.eq_ignore_ascii_case(w)))
            .unwrap_or(false)
    })
}
