//! Generic "record any game" integration — Medal-parity detect-and-record for
//! games with no dedicated integration.
//!
//! Hako's three smart integrations (Valorant / League / Rematch) each know how to
//! follow a match and cut per-event highlights. Everything *else* records
//! generically: buffer + manual clips, plus whole-session/full-match capture, with
//! **no** automatic event clips (there's no event feed) — exactly how Medal treats
//! a game with no `GameCustomization`. This module is that one generic pipeline,
//! registered as a fourth [`GameIntegration`] under the [`GameId::Other`] bucket.
//!
//! Detection is catalog-driven, never "hook any fullscreen app" (§ the plan):
//! - [`catalog`] — the "is this process a game, and what's it called?" resolver.
//!   Phase 1 matches the user-added custom list only (Steam scan + curated list
//!   are Phases 2–3); it excludes the three smart games + a non-game blacklist.
//! - [`detect`] — a short-TTL cache over one catalog scan per tick, shared by the
//!   integration's `find_window` and the status labeler so the real detected name
//!   surfaces without re-scanning.
//! - [`integration`] — the loop: reuse [`recording::GameCtx::auto_manage_capture`]
//!   to start/stop capture on the detected window, record whole sessions in
//!   Session / Full-match modes, and publish the real game name to status + clips.

pub mod catalog;
pub mod curated;
pub mod detect;
pub mod integration;
pub mod steam;

pub use integration::Integration;

/// A generic game currently detected as running with a visible main window. The
/// `name` is the *real* title (from the custom list / curated / Steam `.acf`) —
/// used for the "Now Clipping <name>" pill and the clip's `game` column, even
/// though the arbiter/settings key is the single [`super::GameId::Other`] bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedGame {
    /// The game window's HWND (visible, non-minimized main window).
    pub hwnd: i64,
    /// Real game title for the status pill + `clip.game`.
    pub name: String,
    /// Owning exe file name, lowercase (the custom-list match key).
    pub process_name: String,
    /// Which catalog source resolved it.
    pub source: GameSource,
}

/// Where a [`DetectedGame`]'s identity came from, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameSource {
    /// A user-added entry in the custom-games table (Medal's `CustomGameDatabase`).
    Custom,
    /// The bundled curated list (`assets/games.json`).
    Curated,
    /// A Steam install resolved from `steamapps\...\appmanifest_*.acf`.
    Steam,
}
