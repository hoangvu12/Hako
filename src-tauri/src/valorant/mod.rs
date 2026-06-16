//! Valorant integration — the "brain".
//!
//! Lockfile auth → local presence websocket → session loop state machine →
//! post-match match-details fetch → kill reconciliation → event derivation.
//! Read-only; isolated behind these modules so Riot API breakage degrades
//! gracefully to manual/hotkey clips.

#![allow(dead_code)]

pub mod lockfile; // parse lockfile, local auth
pub mod local_api; // presences, websocket, session loop state
pub mod remote_api; // glz/pd: current game, match-details
pub mod model; // match/round/kill structs (serde)
pub mod reconcile; // kill time → buffer position; event derivation
pub mod service; // state machine; orchestrates A/B record modes
