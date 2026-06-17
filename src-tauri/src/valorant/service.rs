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

use std::time::Duration;

use serde::Serialize;
use sysinfo::System;

use crate::valorant::local_api::LocalClient;
use crate::valorant::log_watch;
use crate::valorant::model::LoopState;
use crate::valorant::remote_api::{self, RemoteClient};

/// The Valorant game process (not the launcher `RiotClientServices.exe`).
const VALORANT_PROCESS: &str = "VALORANT-Win64-Shipping.exe";

// Medal's retry budgets (`ValorantSessionService`): chat connect 6 × 20 s,
// client version 12 × 10 s.
const CHAT_CONNECT_ATTEMPTS: u32 = 6;
const CHAT_CONNECT_DELAY: Duration = Duration::from_secs(20);
const CLIENT_VERSION_ATTEMPTS: u32 = 12;
const CLIENT_VERSION_DELAY: Duration = Duration::from_secs(10);

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

/// Everything a live match needs after bootstrap: the authenticated remote
/// client plus our identity and the resolved region/shard. Port of Medal's
/// `ValorantSessionData` paired with the built `ValorantRemoteApiClient`.
pub struct SessionData {
    /// Authenticated pvp.net client (current-game + match-details).
    pub remote: RemoteClient,
    pub puuid: String,
    pub player_name: String,
    pub tagline: String,
    /// glz affinity/region and pvp.net shard (Medal sets both from
    /// `-ares-deployment=`).
    pub region: String,
    pub shard: String,
    pub client_version: String,
}

/// Bootstrap a Valorant session, faithfully porting Medal's
/// `ValorantSessionService.InitializeSessionAsync`:
/// 1. chat-session retry until `state == "connected"` (6 × 20 s) → identity,
/// 2. shard/region from `/product-session/v1/external-sessions`
///    (`-ares-deployment=`), falling back to the chat region + `region_to_shard`,
/// 3. entitlements token (access token + entitlements JWT),
/// 4. client version — `CI server version:` from the log first, else
///    valorant-api.com — retried (12 × 10 s),
/// 5. assemble the authenticated [`RemoteClient`].
///
/// Errors (rather than Medal's "return null") so the caller can log + degrade to
/// manual clips. Long-running: bounded by the retry budgets above.
pub async fn start_session(client: &LocalClient) -> Result<SessionData, String> {
    let chat = attempt_chat_connection(client).await?;
    let (region, shard) = resolve_region_shard(client, &chat.region).await?;

    let ent = client
        .entitlements()
        .await
        .map_err(|e| format!("entitlements token: {e}"))?;

    let client_version = attempt_client_version().await?;

    let remote = RemoteClient::with_region_shard(
        &region,
        &shard,
        &ent.access_token,
        &ent.token,
        &client_version,
    )?;

    tracing::info!(
        region = %region,
        shard = %shard,
        version = %client_version,
        "valorant session bootstrap complete"
    );
    Ok(SessionData {
        remote,
        puuid: chat.puuid,
        player_name: chat.game_name,
        tagline: chat.game_tag,
        region,
        shard,
        client_version,
    })
}

/// Poll `/chat/v1/session` until it reports `connected` (Medal: 6 × 20 s).
async fn attempt_chat_connection(
    client: &LocalClient,
) -> Result<crate::valorant::model::ChatSession, String> {
    let mut last_err = String::from("chat session never connected");
    for attempt in 0..CHAT_CONNECT_ATTEMPTS {
        match client.chat_session().await {
            Ok(s) if s.is_connected() => return Ok(s),
            Ok(s) => last_err = format!("chat session state = {:?}", s.state),
            Err(e) => last_err = format!("chat session: {e}"),
        }
        if attempt + 1 < CHAT_CONNECT_ATTEMPTS {
            tracing::debug!("waiting for chat connection ({last_err})");
            tokio::time::sleep(CHAT_CONNECT_DELAY).await;
        }
    }
    Err(last_err)
}

/// Resolve (region, shard): prefer the live `-ares-deployment=` value (Medal
/// sets both region and shard to it); else fall back to the chat-session region
/// and the [`region_to_shard`](remote_api::region_to_shard) heuristic.
async fn resolve_region_shard(
    client: &LocalClient,
    chat_region: &str,
) -> Result<(String, String), String> {
    let deployment = client.valorant_deployment().await.unwrap_or(None);
    resolve_region_shard_values(deployment.as_deref(), chat_region)
        .ok_or_else(|| "could not determine region/shard (no -ares-deployment and no chat region)".into())
}

/// Pure region/shard decision (testable without a live client). Medal uses the
/// deployment string for **both** region and shard; otherwise we map the chat
/// region through the shard heuristic.
fn resolve_region_shard_values(
    deployment: Option<&str>,
    chat_region: &str,
) -> Option<(String, String)> {
    if let Some(dep) = deployment.map(str::trim).filter(|d| !d.is_empty()) {
        return Some((dep.to_string(), dep.to_string()));
    }
    let region = chat_region.trim();
    if region.is_empty() {
        return None;
    }
    Some((region.to_string(), remote_api::region_to_shard(region).to_string()))
}

/// Determine the client release version: the log's `CI server version:` line
/// first (Medal's primary), else valorant-api.com — retried (12 × 10 s).
async fn attempt_client_version() -> Result<String, String> {
    let http = reqwest::Client::new();
    for attempt in 0..CLIENT_VERSION_ATTEMPTS {
        if let Some(v) = log_watch::client_version_from_log() {
            return Ok(v);
        }
        match remote_api::fetch_client_version(&http).await {
            Ok(v) if !v.is_empty() => return Ok(v),
            Ok(_) => {}
            Err(e) => tracing::debug!("client version from API failed: {e}"),
        }
        if attempt + 1 < CLIENT_VERSION_ATTEMPTS {
            tokio::time::sleep(CLIENT_VERSION_DELAY).await;
        }
    }
    Err("could not determine client version (log + API both failed)".into())
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
    fn region_shard_prefers_deployment_for_both() {
        // Medal sets region == shard == the -ares-deployment value.
        assert_eq!(
            resolve_region_shard_values(Some("eu"), "ignored"),
            Some(("eu".into(), "eu".into()))
        );
        // latam deployment is used verbatim (matches the live client's URLs).
        assert_eq!(
            resolve_region_shard_values(Some(" latam "), ""),
            Some(("latam".into(), "latam".into()))
        );
    }

    #[test]
    fn region_shard_falls_back_to_chat_region_heuristic() {
        // No deployment → chat region + region_to_shard (latam → na shard).
        assert_eq!(
            resolve_region_shard_values(None, "latam"),
            Some(("latam".into(), "na".into()))
        );
        assert_eq!(
            resolve_region_shard_values(Some("  "), "eu"),
            Some(("eu".into(), "eu".into()))
        );
        // Nothing to go on → None (bootstrap errors out).
        assert_eq!(resolve_region_shard_values(None, ""), None);
    }

    #[test]
    fn chat_session_connected_flag() {
        use crate::valorant::model::ChatSession;
        let connected: ChatSession =
            serde_json::from_str(r#"{"puuid":"p","state":"connected"}"#).unwrap();
        assert!(connected.is_connected());
        let connecting: ChatSession =
            serde_json::from_str(r#"{"puuid":"p","state":"connecting"}"#).unwrap();
        assert!(!connecting.is_connected());
    }

    #[test]
    fn process_scan_runs() {
        // Can't assert the result (depends on whether Valorant is open), but it
        // must not panic and must return a bool.
        let _ = valorant_running();
    }
}
