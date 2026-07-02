//! League of Legends process / window detection.

use crate::games::process_snapshot;

/// The in-game window title (the Unreal-style game client, not the launcher).
pub const GAME_WINDOW_TITLE: &str = "League of Legends (TM) Client";
/// The in-game process (the actual match), distinct from the `LeagueClientUx`
/// pre-game client.
pub const GAME_PROCESS: &str = "League of Legends.exe";

/// Whether the in-game League process is running (the match client, not the
/// launcher). Reads the shared, rate-limited process snapshot.
pub fn game_running() -> bool {
    process_snapshot::any_running(&[GAME_PROCESS], process_snapshot::DEFAULT_MAX_AGE)
}
