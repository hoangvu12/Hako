//! War Thunder integration.
//!
//! Unlike the Valve GSI games (CS2 / Dota 2), War Thunder exposes no push feed
//! and needs no config file. Instead its client hosts a small **web-HUD server**
//! on `http://127.0.0.1:8111` while a battle is running. Hako polls two endpoints
//! as an ordinary HTTP client ([`api`]): `/hudmsg` for the scrolling damage log
//! and `/indicators` for the local vehicle class. We classify each damage line
//! that mentions the player's nickname into a Kill / Death / Crash event
//! ([`events`]), stamp it with the capture-clock wall-clock at receipt, and
//! reconcile the collected events to the recorded session at battle end — the
//! same live-feed shape as League/CS2, just sourced from an outbound poll.
//!
//! No `.clog` decryption, memory reading, or cloud calls (Medal's nickname-from-
//! replay-log path is deliberately skipped — the nickname is a settings field).

pub mod api;
pub mod detect;
pub mod events;
pub mod integration;

pub use integration::Integration;
