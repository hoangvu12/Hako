//! Valorant orchestration.
//!
//! Two pieces:
//! - [`StateMachine`] — a pure transition machine over `sessionLoopState` +
//!   round count. On entering INGAME it says "start the session recording", on
//!   each score increment "log a round boundary" (the reconciliation anchor),
//!   and on leaving INGAME "the match ended → fetch + reconcile + cut".
//!   Unit-tested without a live game.
//! - [`probe_status`] — a best-effort snapshot for the `/valorant` UI panel:
//!   is Valorant running, can we reach the local API, and the current state/score.
//!   Degrades gracefully — any failure becomes a populated `error`.
//!
//! The live loop (poll presence every ~2 s, drive the machine, kick the
//! record/reconcile pipeline) is wired in `commands.rs`/`main.rs`; this module
//! provides the testable logic + probe it builds on.

#![allow(dead_code)]

use serde::Serialize;
use sysinfo::System;

use crate::valorant::local_api::LocalClient;
use crate::valorant::model::LoopState;

/// The Valorant game process (not the launcher `RiotClientServices.exe`).
const VALORANT_PROCESS: &str = "VALORANT-Win64-Shipping.exe";

/// What the state machine asks the orchestrator to do on a state update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Entered INGAME — begin Mode B full-match recording.
    MatchStarted,
    /// Score changed during INGAME — log a round-boundary wall-clock.
    RoundBoundary { rounds_played: i32 },
    /// Left INGAME — fetch match-details, reconcile, cut clips, drop session file.
    MatchEnded,
}

/// Pure transition machine over `(sessionLoopState, rounds_played)`.
pub struct StateMachine {
    state: LoopState,
    rounds_played: i32,
    in_match: bool,
}

impl Default for StateMachine {
    fn default() -> Self {
        StateMachine {
            state: LoopState::Unknown,
            rounds_played: 0,
            in_match: false,
        }
    }
}

impl StateMachine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state(&self) -> LoopState {
        self.state
    }

    /// Feed the latest presence (`loop_state` + total rounds played =
    /// ally+enemy score). Returns the actions triggered by this update.
    pub fn update(&mut self, loop_state: LoopState, rounds_played: i32) -> Vec<Action> {
        let mut actions = Vec::new();

        let entering = loop_state == LoopState::InGame && !self.in_match;
        let leaving = loop_state != LoopState::InGame && self.in_match;

        if entering {
            self.in_match = true;
            self.rounds_played = rounds_played;
            actions.push(Action::MatchStarted);
        } else if self.in_match
            && loop_state == LoopState::InGame
            && rounds_played != self.rounds_played
        {
            self.rounds_played = rounds_played;
            actions.push(Action::RoundBoundary { rounds_played });
        }

        if leaving {
            self.in_match = false;
            actions.push(Action::MatchEnded);
        }

        self.state = loop_state;
        actions
    }
}

/// Is the Valorant game process running? (Cheap-ish full process scan.)
pub fn valorant_running() -> bool {
    let sys = System::new_all();
    sys.processes().values().any(|p| {
        p.name()
            .to_str()
            .map(|n| n.eq_ignore_ascii_case(VALORANT_PROCESS))
            .unwrap_or(false)
    })
}

/// Snapshot for the `/valorant` panel. Mirrors `ValorantStatus` in api.ts.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ValorantStatus {
    /// Game process detected.
    pub running: bool,
    /// Local API reachable (lockfile present + responded).
    pub connected: bool,
    /// `MENUS` / `PREGAME` / `INGAME` / null.
    pub loop_state: Option<String>,
    pub score_ally: i32,
    pub score_enemy: i32,
    pub map: String,
    /// Populated on any degraded path (Riot down, not logged in, etc.).
    pub error: Option<String>,
}

/// Best-effort live status: process → local API → our presence.
pub async fn probe_status() -> ValorantStatus {
    let mut status = ValorantStatus {
        running: valorant_running(),
        ..Default::default()
    };

    let client = match LocalClient::connect() {
        Ok(c) => c,
        Err(e) => {
            status.error = Some(e);
            return status;
        }
    };
    status.connected = true;

    let puuid = match client.chat_session().await {
        Ok(s) => s.puuid,
        Err(e) => {
            status.error = Some(e);
            return status;
        }
    };

    match client.our_presence(&puuid).await {
        Ok(Some(p)) => {
            let ls = p.session_loop_state();
            status.loop_state = (!ls.is_empty()).then(|| ls.to_string());
            status.score_ally = p.score_ally;
            status.score_enemy = p.score_enemy;
            status.map = p.match_map().to_string();
        }
        Ok(None) => status.loop_state = None,
        Err(e) => status.error = Some(e),
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_match_lifecycle() {
        let mut sm = StateMachine::new();
        // Menus → no actions.
        assert!(sm.update(LoopState::Menus, 0).is_empty());
        // Pregame → no actions.
        assert!(sm.update(LoopState::Pregame, 0).is_empty());
        // Enter INGAME → MatchStarted.
        assert_eq!(sm.update(LoopState::InGame, 0), vec![Action::MatchStarted]);
        // Same state, no score change → nothing.
        assert!(sm.update(LoopState::InGame, 0).is_empty());
        // Score ticks to 1 round played → RoundBoundary.
        assert_eq!(
            sm.update(LoopState::InGame, 1),
            vec![Action::RoundBoundary { rounds_played: 1 }]
        );
        // Back to MENUS → MatchEnded.
        assert_eq!(sm.update(LoopState::Menus, 13), vec![Action::MatchEnded]);
        // Idle in menus → nothing.
        assert!(sm.update(LoopState::Menus, 13).is_empty());
    }

    #[test]
    fn re_entering_ingame_starts_a_new_match() {
        let mut sm = StateMachine::new();
        sm.update(LoopState::InGame, 0);
        sm.update(LoopState::Menus, 24); // match ended
        // A second match.
        assert_eq!(sm.update(LoopState::InGame, 0), vec![Action::MatchStarted]);
    }

    #[test]
    fn process_scan_runs() {
        // Can't assert the result (depends on whether Valorant is open), but it
        // must not panic and must return a bool.
        let _ = valorant_running();
    }
}
