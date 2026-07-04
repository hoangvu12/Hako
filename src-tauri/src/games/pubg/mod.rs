//! PLAYERUNKNOWN'S BATTLEGROUNDS integration.
//!
//! Unlike every other Hako game, PUBG exposes **no live feed at all** — instead
//! the client writes a *replay* to disk for each match under
//! `%LOCALAPPDATA%\TslGame\Saved\Demos\`. A finished replay directory carries a
//! `PUBG.replayinfo` header plus `events/` + `data/` JSON sidecars (no binary
//! parsing — some `data/` files are lightly obfuscated "UE4 strings"). We record
//! continuously while the game runs, and when a match's replay finalizes
//! ([`watch`] then [`parse`]) we derive its kill / knockdown / death /
//! chicken-dinner events, map their replay timestamps onto the recorded session,
//! and cut the highlights — the same post-match reconcile shape as Valorant,
//! sourced from replay sidecars instead of a remote match API. No demo (binary)
//! parsing, memory reading, or cloud calls (Medal's match-ingest path is
//! deliberately skipped — Hako is local-only).

pub mod detect;
pub mod events;
pub mod integration;
pub mod parse;
pub mod watch;

pub use integration::Integration;
