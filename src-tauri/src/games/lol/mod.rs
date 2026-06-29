//! League of Legends integration.
//!
//! Unlike Valorant's post-match reconcile, League exposes a **live event feed**
//! (the Live Client Data API on `https://127.0.0.1:2999`) that is only up while a
//! match is running. We detect the game by its window, auto-start capture, and
//! poll the feed each second; every new event is stamped with the capture clock
//! and reconciled to the recorded session at match end. No remote API, no log
//! parsing, no round reconciliation.

pub mod context;
pub mod detect;
pub mod events;
pub mod integration;
pub mod live_client;

pub use integration::Integration;
