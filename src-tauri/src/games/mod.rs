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

pub mod event;
pub mod lol;
pub mod lockfile;
pub mod net;
pub mod orchestrator;
pub mod recording;
pub mod rematch;
pub mod timeline;

use std::sync::Mutex;

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
}

#[allow(dead_code)] // `as_str`/`from_str` are part of the GameId API surface.
impl GameId {
    /// Stable lowercase id (DB column, settings key).
    pub fn as_str(self) -> &'static str {
        match self {
            GameId::Valorant => "valorant",
            GameId::Lol => "lol",
            GameId::Rematch => "rematch",
        }
    }

    /// Human label for UI copy ("Now Clipping <name>").
    pub fn display_name(self) -> &'static str {
        match self {
            GameId::Valorant => "Valorant",
            GameId::Lol => "League of Legends",
            GameId::Rematch => "Rematch",
        }
    }

    /// Parse a stored id back to a [`GameId`] (`None` for unknown ids).
    pub fn from_str(s: &str) -> Option<GameId> {
        match s.trim().to_ascii_lowercase().as_str() {
            "valorant" => Some(GameId::Valorant),
            "lol" | "leagueoflegends" | "league_of_legends" => Some(GameId::Lol),
            "rematch" => Some(GameId::Rematch),
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
pub fn registry() -> Vec<std::sync::Arc<dyn GameIntegration>> {
    vec![
        std::sync::Arc::new(valorant::Integration),
        std::sync::Arc::new(lol::Integration),
        std::sync::Arc::new(rematch::Integration),
    ]
}

/// The game whose window is currently present, preferring the registry order
/// (Valorant first). Used by `recorder_status` to label the detected game without
/// hard-coding Valorant.
pub fn detected_game() -> Option<GameId> {
    registry().iter().find_map(|g| g.find_window().map(|_| g.id()))
}
