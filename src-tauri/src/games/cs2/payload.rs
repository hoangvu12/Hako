//! CS2 Game-State-Integration payload — serde structs + validation.
//!
//! CS2 POSTs a JSON snapshot on every state change (the components we request in
//! [`super::gsi`]). We deserialize the subset we care about, then validate it the
//! way Medal's `CounterStrike2Parser` does: the top-level objects must be present
//! and — critically — `player.steamid == provider.steamid`, so we only ever
//! process **our own** stats and never clip a *spectated* player's kills.
//!
//! [`parse_valid`] flattens a valid raw payload into an owned [`ValidPayload`] so
//! the diff logic in [`super::events`] stays serde-free and unit-testable.

#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Payload {
    pub auth: Option<Auth>,
    pub map: Option<Map>,
    pub player: Option<Player>,
    pub provider: Option<Provider>,
    pub round: Option<Round>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Map {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub round: i32,
    #[serde(default)]
    pub phase: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Player {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub steamid: String,
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub match_stats: MatchStats,
    #[serde(default)]
    pub state: State,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchStats {
    #[serde(default)]
    pub kills: i32,
    #[serde(default)]
    pub deaths: i32,
    #[serde(default)]
    pub assists: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct State {
    /// Headshot kills this round (resets to 0 at each round start).
    #[serde(default)]
    pub round_killhs: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Provider {
    #[serde(default)]
    pub steamid: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Round {
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub bomb: Option<String>,
}

/// A validated CS2 payload flattened to owned fields, guaranteed to concern the
/// local player (steamid cross-checked). Everything the diff needs, no serde.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidPayload {
    pub map_name: String,
    pub map_mode: String,
    pub map_round: i32,
    pub map_phase: String,
    pub player_name: String,
    pub team: String,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub round_killhs: i32,
    pub round_phase: String,
    pub bomb: String,
}

impl ValidPayload {
    /// Whether the match is over (final scoreboard) — a finalize signal.
    pub fn is_gameover(&self) -> bool {
        self.map_phase.eq_ignore_ascii_case("gameover")
    }
}

/// Parse a raw GSI body into a [`ValidPayload`], or `None` if it doesn't
/// deserialize / is missing a required object / isn't the local player.
///
/// (The `auth.token` is already validated by [`crate::games::gsi::GsiServer`]
/// before the body reaches us, so we don't re-check it here — but the fields are
/// present should a caller want to.)
pub fn parse_valid(body: &str) -> Option<ValidPayload> {
    let p: Payload = serde_json::from_str(body).ok()?;
    let map = p.map?;
    let player = p.player?;
    let provider = p.provider?;
    let round = p.round?;
    // Spectator filter: only our own stats (mirrors Medal's steamid cross-check).
    if player.steamid.is_empty() || player.steamid != provider.steamid {
        return None;
    }
    Some(ValidPayload {
        map_name: map.name,
        map_mode: map.mode,
        map_round: map.round,
        map_phase: map.phase,
        player_name: player.name,
        team: player.team,
        kills: player.match_stats.kills,
        deaths: player.match_stats.deaths,
        assists: player.match_stats.assists,
        round_killhs: player.state.round_killhs,
        round_phase: round.phase,
        bomb: round.bomb.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = r#"{
        "auth": {"token": "t"},
        "map": {"name": "de_dust2", "mode": "competitive", "round": 3, "phase": "live"},
        "player": {"name": "me", "steamid": "111", "team": "CT",
                   "match_stats": {"kills": 5, "deaths": 2, "assists": 1},
                   "state": {"round_killhs": 1}},
        "provider": {"steamid": "111"},
        "round": {"phase": "live", "bomb": "planted"}
    }"#;

    #[test]
    fn parses_own_payload() {
        let v = parse_valid(OURS).unwrap();
        assert_eq!(v.map_name, "de_dust2");
        assert_eq!(v.kills, 5);
        assert_eq!(v.round_killhs, 1);
        assert_eq!(v.bomb, "planted");
    }

    #[test]
    fn rejects_spectated_player() {
        // Player steamid ≠ provider steamid ⇒ we're spectating someone else.
        let spec = r#"{
            "auth": {"token": "t"},
            "map": {"name": "de_dust2", "mode": "competitive", "round": 3, "phase": "live"},
            "player": {"name": "them", "steamid": "999", "team": "T",
                       "match_stats": {"kills": 9, "deaths": 0, "assists": 0},
                       "state": {"round_killhs": 2}},
            "provider": {"steamid": "111"},
            "round": {"phase": "live"}
        }"#;
        assert!(parse_valid(spec).is_none());
    }

    #[test]
    fn rejects_missing_objects_and_garbage() {
        assert!(parse_valid("{}").is_none());
        assert!(parse_valid("not json").is_none());
    }
}
