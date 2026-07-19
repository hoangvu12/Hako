//! Rematch integration.
//!
//! Rematch (Sloclap, UE5) exposes no local API and no goal feed — Medal resolves
//! goals server-side, Overwolf via a native memory-reading plugin. But the game's
//! Unreal log (`%LOCALAPPDATA%\Runtime\Saved\Logs\Runtime.log`) carries the goal-
//! sound cue and match lifecycle in plain text, so we follow it the way Valorant
//! follows `ShooterGame.log`: detect the game by process, auto-start capture, and
//! tail the log — stamping each goal with the capture clock and reconciling to the
//! recorded session at match end (League's live-feed shape). Highlights are
//! **Goal** (any goal, matching Medal's "Goal Scored") plus **My Goal** /
//! **My Assist**, attributed to the local player via the achievement-stat
//! increments the game logs right after their own goals (see `log_watch`).

pub mod context;
pub mod detect;
pub mod events;
pub mod integration;
pub mod log_watch;

pub use integration::Integration;
