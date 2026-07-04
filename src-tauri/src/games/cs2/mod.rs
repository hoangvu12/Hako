//! Counter-Strike 2 integration.
//!
//! CS2 exposes its live state through Valve's official **Game-State Integration**
//! (GSI): drop a `gamestate_integration_hako.cfg` into the game's `cfg` dir and
//! CS2 POSTs a JSON snapshot to a localhost port we host on every state change.
//! Hako hosts that server ([`crate::games::gsi`]), diffs successive payloads into
//! kill / headshot / multi-kill / death / assist events, and reconciles them to
//! the recorded session at match end — League's live-feed shape, sourced from an
//! inbound HTTP feed. No demo parsing, memory reading, or cloud calls (Medal's
//! match-ingest / demo-upload paths are deliberately skipped — Hako is
//! local-only).

pub mod detect;
pub mod events;
pub mod gsi;
pub mod integration;
pub mod payload;

pub use integration::Integration;
