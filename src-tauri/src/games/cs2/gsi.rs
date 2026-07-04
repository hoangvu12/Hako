//! CS2-specific GSI wiring: resolve the cfg dir, write the config, and own the
//! inbound HTTP server for as long as the game is running.
//!
//! The Steam install dir is resolved from the running `cs2.exe`'s path, then the
//! cfg is written to `…\game\csgo\cfg\gamestate_integration_hako.cfg`. CS2 loads
//! GSI cfgs at process start, so a cfg written while the game is already up takes
//! effect on the *next* launch — the server still runs now and receives payloads
//! whenever a valid cfg is already present (from a prior session). We pick port
//! 31761 (Medal uses 12761) so a co-installed Medal coexists.

#![allow(dead_code)]

use std::sync::mpsc::Receiver;

use tauri::AppHandle;

use crate::games::cs2::detect;
use crate::games::gsi::{self, GsiServer};

/// Our GSI port (distinct from Medal's 12761 so both can run side by side).
pub const GSI_PORT: u16 = 31761;

/// State components we ask CS2 to include in each POST.
const COMPONENTS: &[&str] = &[
    "provider",
    "map",
    "round",
    "player_id",
    "player_state",
    "player_match_stats",
    "player_weapons",
];

/// A running CS2 GSI endpoint: the server (stopped on drop) plus the receiver
/// the integration drains each tick.
pub struct Cs2Gsi {
    _server: GsiServer,
    pub rx: Receiver<String>,
}

/// Resolve the CS2 install dir, write our GSI cfg (if changed), and start the
/// server. `None` if CS2 isn't a resolvable Steam install or the port is taken.
pub fn start(app: &AppHandle) -> Option<Cs2Gsi> {
    let base = gsi::steam_install_base(detect::GAME_PROCESSES)?;
    let cfg_path = base
        .join("game")
        .join("csgo")
        .join("cfg")
        .join("gamestate_integration_hako.cfg");
    let token = gsi::shared_token(app);
    let contents = gsi::config_file("hako", GSI_PORT, &token, COMPONENTS);
    match gsi::write_config_if_changed(&cfg_path, &contents) {
        Ok(true) => tracing::info!("cs2: wrote GSI cfg → {}", cfg_path.display()),
        Ok(false) => {}
        Err(e) => tracing::warn!("cs2: failed to write GSI cfg: {e}"),
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let server = match GsiServer::start(GSI_PORT, token, tx) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cs2: could not start GSI server on {GSI_PORT}: {e}");
            return None;
        }
    };
    Some(Cs2Gsi {
        _server: server,
        rx,
    })
}
