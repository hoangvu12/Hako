//! Shared live-match context for tagging **manual** clips.
//!
//! The orchestrator's presence loop keeps this current while a Valorant match is
//! in progress: `map` + `mode` come straight from presence each tick, and
//! `agent` is resolved once per match by a best-effort core-game fetch. A manual
//! F9 save reads it (see [`commands::save_clip_full`]) so the clip carries the
//! same agent/map/mode an auto-clip would — win/loss + K/D/A stay unknown
//! mid-match. Reset to all-`None` when we leave the match.

use std::sync::Mutex;

use crate::library::db::NewClip;

/// The in-progress match's context, or all-default (`in_match = false`) when not
/// in a match.
#[derive(Debug, Clone, Default)]
pub struct LiveMatch {
    /// True while `sessionLoopState == INGAME`.
    pub in_match: bool,
    /// Map asset path (presence `matchMap`, e.g. `/Game/Maps/Ascent/Ascent`).
    pub map: Option<String>,
    /// Game-mode display name derived from the live queue id.
    pub mode: Option<String>,
    /// Agent display name, resolved best-effort once per match (`None` until the
    /// core-game fetch lands, or if it fails).
    pub agent: Option<String>,
    /// Agent UUID paired with `agent`.
    pub agent_id: Option<String>,
}

impl LiveMatch {
    /// The clip game-context for a manual save taken *right now*: agent/map/mode
    /// when in a match. Win/loss + K/D/A are unknowable mid-match, so they stay
    /// `None`. Returns a context-only [`NewClip`] for struct-update merge.
    pub fn clip_context(&self) -> NewClip {
        if !self.in_match {
            return NewClip::default();
        }
        NewClip {
            agent: self.agent.clone(),
            agent_id: self.agent_id.clone(),
            map: self.map.clone(),
            mode: self.mode.clone(),
            ..Default::default()
        }
    }
}

/// Tauri-managed handle to the live match context. Registered in `main`; written
/// by the orchestrator, read by `save_clip_full`.
#[derive(Default)]
pub struct LiveMatchState(pub Mutex<LiveMatch>);
