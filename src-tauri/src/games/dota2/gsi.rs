//! Dota 2-specific GSI wiring: resolve the cfg dir, write the config, and own
//! the inbound HTTP server for as long as the game is running.
//!
//! Same harness as CS2 ([`crate::games::gsi`]); only the cfg subpath, port, and
//! components differ. Dota's GSI cfg lives one dir deeper than CS2's, under a
//! `gamestate_integration\` folder. We use port 31760 (Medal uses 12760) so both
//! coexist.

#![allow(dead_code)]

use std::sync::mpsc::Receiver;

use tauri::AppHandle;

use crate::games::dota2::detect;
use crate::games::gsi::{self, GsiServer};

/// Our GSI port (distinct from Medal's 12760).
pub const GSI_PORT: u16 = 31760;

/// State components we ask Dota 2 to include in each POST. (We request `events`
/// / `items` too for parity, though the Aegis paths are currently unused.)
const COMPONENTS: &[&str] = &["provider", "player", "hero", "map", "events", "items"];

/// A running Dota 2 GSI endpoint (server + drain receiver).
pub struct Dota2Gsi {
    _server: GsiServer,
    pub rx: Receiver<String>,
}

/// Resolve the Dota 2 install dir, write our GSI cfg (if changed), and start the
/// server. `None` if Dota 2 isn't a resolvable Steam install or the port is taken.
pub fn start(app: &AppHandle) -> Option<Dota2Gsi> {
    let base = gsi::steam_install_base(detect::GAME_PROCESSES)?;
    let cfg_path = base
        .join("game")
        .join("dota")
        .join("cfg")
        .join("gamestate_integration")
        .join("gamestate_integration_hako.cfg");
    let token = gsi::shared_token(app);
    let contents = gsi::config_file("hako", GSI_PORT, &token, COMPONENTS);
    match gsi::write_config_if_changed(&cfg_path, &contents) {
        Ok(true) => tracing::info!("dota2: wrote GSI cfg → {}", cfg_path.display()),
        Ok(false) => {}
        Err(e) => tracing::warn!("dota2: failed to write GSI cfg: {e}"),
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let server = match GsiServer::start(GSI_PORT, token, tx) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("dota2: could not start GSI server on {GSI_PORT}: {e}");
            return None;
        }
    };
    Some(Dota2Gsi {
        _server: server,
        rx,
    })
}
