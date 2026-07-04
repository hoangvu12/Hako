//! Dota 2 integration.
//!
//! Dota 2 exposes its live state through Valve's official **Game-State
//! Integration** (GSI), the same mechanism as CS2 — Hako hosts a localhost
//! server ([`crate::games::gsi`]) the game POSTs to, diffs successive payloads
//! into kill / multi-kill / death / assist events, and reconciles them to the
//! recorded session at match end. Only the cfg path, port, components, and
//! payload shape differ from CS2; the server harness is shared. No cloud
//! match-ingest (Medal's `MatchAPI.PostMatch` is skipped — Hako is local-only).

pub mod detect;
pub mod events;
pub mod gsi;
pub mod integration;
pub mod payload;

pub use integration::Integration;
