//! Multi-game integration layer.
//!
//! Hako's capture/encode core ([`crate::core`]) is game-agnostic. Everything that
//! knows *about a specific game* — how to detect its window, follow a match, and
//! turn a finished match into highlight clips — lives behind this module as a
//! [`GameIntegration`]. Each supported title is a self-contained submodule;
//! adding a new game is implementing the trait and registering it in
//! [`registry`].
//!
//! Two integration *shapes* coexist behind the one trait:
//! - **post-match reconcile** (Valorant, [`crate::valorant`]): poll presence →
//!   state machine → at match end fetch remote match-details, derive events,
//!   reconcile their times to the recorded session PTS via round anchors + a
//!   [`timeline::TimelineIndex`].
//! - **live event feed** (League, [`lol`]): poll the local Live Client Data API
//!   during the game; each event is stamped with the wall-clock at receipt and
//!   placed directly on the session timeline. No remote fetch or reconciliation.
//!
//! The shared recording machinery ([`recording`]) and the supervisor
//! ([`orchestrator`]) are the same for both: both shapes converge on producing
//! placed clip windows handed to one cut routine.

pub mod cs2;
pub mod dota2;
pub mod engine;
pub mod event;
pub mod event_config;
pub mod generic;
pub mod gsi;
pub mod lockfile;
pub mod lol;
pub mod net;
pub mod orchestrator;
pub mod process_snapshot;
pub mod pubg;
pub mod recording;
pub mod rematch;
pub mod timeline;
pub mod warthunder;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::valorant;
use recording::GameCtx;

/// A game Hako can integrate with. Serialized as a stable lowercase id, stored in
/// the clip DB's `game` column and used as the per-game settings key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameId {
    Valorant,
    Lol,
    Rematch,
    Cs2,
    Dota2,
    WarThunder,
    Pubg,
    /// The generic "any other game" bucket — a single arbiter/settings key shared
    /// by every non-integrated game we detect + record generically (Steam /
    /// curated / user-added). Individual clips are still tagged with the *real*
    /// detected game name, not this bucket label (see [`generic`]).
    Other,
}

#[allow(dead_code)] // `as_str`/`from_str` are part of the GameId API surface.
impl GameId {
    /// Stable lowercase id (DB column, settings key).
    pub fn as_str(self) -> &'static str {
        match self {
            GameId::Valorant => "valorant",
            GameId::Lol => "lol",
            GameId::Rematch => "rematch",
            GameId::Cs2 => "cs2",
            GameId::Dota2 => "dota2",
            GameId::WarThunder => "warthunder",
            GameId::Pubg => "pubg",
            GameId::Other => "other",
        }
    }

    /// Human label for UI copy ("Now Clipping <name>").
    pub fn display_name(self) -> &'static str {
        match self {
            GameId::Valorant => "Valorant",
            GameId::Lol => "League of Legends",
            GameId::Rematch => "Rematch",
            GameId::Cs2 => "Counter-Strike 2",
            GameId::Dota2 => "Dota 2",
            GameId::WarThunder => "War Thunder",
            GameId::Pubg => "PUBG",
            // The generic *bucket* label. A live generic clip/status uses the real
            // detected title instead (see `commands::recorder_status_snapshot`).
            GameId::Other => "Other Games",
        }
    }

    /// Parse a stored id back to a [`GameId`] (`None` for unknown ids).
    pub fn from_str(s: &str) -> Option<GameId> {
        match s.trim().to_ascii_lowercase().as_str() {
            "valorant" => Some(GameId::Valorant),
            "lol" | "leagueoflegends" | "league_of_legends" => Some(GameId::Lol),
            "rematch" => Some(GameId::Rematch),
            "cs2" | "counterstrike2" | "counter_strike_2" | "csgo" => Some(GameId::Cs2),
            "dota2" | "dota" | "dota_2" => Some(GameId::Dota2),
            "warthunder" | "war_thunder" | "aces" => Some(GameId::WarThunder),
            "pubg" | "tslgame" | "playerunknowns_battlegrounds" => Some(GameId::Pubg),
            "other" => Some(GameId::Other),
            _ => None,
        }
    }
}

/// Which game currently owns the single global capture, if any.
///
/// There is one [`crate::commands::CaptureState`], so only one game integration
/// may auto-manage it at a time. A game claims ownership when its window appears
/// while capture is idle; another game finding its window then leaves it alone
/// (you can't be in two matches at once). Ownership is only ever set for a capture
/// *we* auto-started, so a user's manual capture is never auto-stopped. Managed in
/// `main` and read by [`recording::GameCtx::auto_manage_capture`].
#[derive(Default)]
pub struct CaptureOwner(pub Mutex<Option<GameId>>);

/// A game integration: detect the game, follow a match, and produce highlight
/// clips. Implementors own their own polling cadence inside [`run`](Self::run),
/// calling the shared [`GameCtx`] helpers for everything capture/recording.
#[async_trait]
pub trait GameIntegration: Send + Sync + 'static {
    /// Which game this is.
    fn id(&self) -> GameId;

    /// The game window's HWND if it's running and visible (for auto-capture).
    fn find_window(&self) -> Option<i64>;

    /// Whether the game process is running (used to finalize a recording when the
    /// game vanishes mid-match even though presence/feed went quiet first).
    fn detect_running(&self) -> bool;

    /// Run the integration's live loop forever (per-app, spawned by the
    /// supervisor). Drives detection → recording → cut for this game.
    async fn run(self: std::sync::Arc<Self>, ctx: GameCtx);
}

/// All registered game integrations. Adding a game = add it here.
///
/// Built once and cached — the returned slice is borrowed for the process
/// lifetime, so hot callers (the per-game loops, [`detected_game`]) don't
/// reallocate three `Arc`s on every call.
pub fn registry() -> &'static [Arc<dyn GameIntegration>] {
    static REGISTRY: OnceLock<Vec<Arc<dyn GameIntegration>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        vec![
            Arc::new(valorant::Integration) as Arc<dyn GameIntegration>,
            Arc::new(lol::Integration),
            Arc::new(rematch::Integration),
            Arc::new(cs2::Integration),
            Arc::new(dota2::Integration),
            Arc::new(warthunder::Integration),
            Arc::new(pubg::Integration),
            // Registered LAST so the smart integrations win the single-capture
            // arbiter: if a smart game is up, the generic bucket stands down.
            Arc::new(generic::Integration),
        ]
    })
}

/// The game whose window is currently present, preferring the registry order
/// (Valorant first). Used by `recorder_status` to label the detected game without
/// hard-coding Valorant.
///
/// This is polled from the status snapshot on every tick of all three game loops
/// (~2–5 Hz combined), and each miss costs up to three `EnumWindows` sweeps + a
/// full process-table refresh (Rematch's `find_window_by_process`). A status
/// label doesn't need sub-second freshness, so the result is cached with a short
/// TTL. (Cleaner long-term design: each game loop publishes its own detection
/// result to a shared atomic every tick and this just reads it — deferred; the TTL
/// cache captures most of the win with far less plumbing.)
pub fn detected_game() -> Option<GameId> {
    const TTL: Duration = Duration::from_secs(2);
    static DETECTED: Mutex<Option<(Instant, Option<GameId>)>> = Mutex::new(None);

    let scan = || {
        registry()
            .iter()
            .find_map(|g| g.find_window().map(|_| g.id()))
    };
    let Ok(mut cache) = DETECTED.lock() else {
        return scan();
    };
    if let Some((at, val)) = *cache {
        if at.elapsed() < TTL {
            return val;
        }
    }
    let val = scan();
    *cache = Some((Instant::now(), val));
    val
}
