//! Valorant integration — the "brain".
//!
//! Lockfile auth → local presence websocket → session loop state machine →
//! post-match match-details fetch → kill reconciliation → event derivation.
//! Read-only; isolated behind these modules so Riot API breakage degrades
//! gracefully to manual/hotkey clips.

#![allow(dead_code)]

pub mod lockfile; // parse lockfile, local auth
pub mod local_api; // presences, websocket, session loop state
pub mod log_watch; // ShooterGame.log tailer → round-start anchors
pub mod remote_api; // glz/pd: current game, match-details
pub mod model; // match/round/kill structs (serde)
pub mod reconcile; // kill time → buffer position; event derivation
pub mod summary; // post-match K/D/A, headshot %, agent, win/loss, title
pub mod service; // state machine + session bootstrap
pub mod cut; // post-match: derive → reconcile → cut clips → library
pub mod orchestrator; // live presence loop driving Mode-B recording
pub mod live; // shared live-match context for tagging manual clips
