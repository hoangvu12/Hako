//! League of Legends process / window detection.

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// The in-game window title (the Unreal-style game client, not the launcher).
pub const GAME_WINDOW_TITLE: &str = "League of Legends (TM) Client";
/// The in-game process (the actual match), distinct from the `LeagueClientUx`
/// pre-game client.
pub const GAME_PROCESS: &str = "League of Legends.exe";

/// Whether the in-game League process is running (the match client, not the
/// launcher). Cheap process-name-only refresh (matches `service::valorant_running`).
pub fn game_running() -> bool {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes().values().any(|p| {
        p.name()
            .to_str()
            .map(|n| n.eq_ignore_ascii_case(GAME_PROCESS))
            .unwrap_or(false)
    })
}
