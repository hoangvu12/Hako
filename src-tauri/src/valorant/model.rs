//! serde structs for the Riot API surface. Field shapes follow valapidocs
//! (techchrism/valorant-api-docs). Only the fields Hako actually consumes are
//! modelled; everything else is ignored by serde. All deserialize-only.
//!
//! Two API tiers: the **local** client API (lockfile auth — entitlements,
//! chat session, presence) and the **remote** pvp.net API (current-game,
//! match-details). The derived [`EventKind`]/[`GameEvent`] types at the bottom
//! are Hako's own (the auto-clip event set).

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Local API: auth + identity
// ---------------------------------------------------------------------------

/// `GET /entitlements/v1/token` — access token + entitlements JWT.
#[derive(Debug, Clone, Deserialize)]
pub struct EntitlementsToken {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    /// The entitlements JWT (sent as `X-Riot-Entitlements-JWT`).
    pub token: String,
    #[serde(default)]
    pub subject: String,
}

/// `GET /chat/v1/session` — our puuid + affinity/region.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatSession {
    pub puuid: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub game_tag: String,
}

// ---------------------------------------------------------------------------
// Local API: presence — real-time, our clock
// ---------------------------------------------------------------------------

/// `GET /chat/v4/presences` → `{ presences: [...] }`.
#[derive(Debug, Clone, Deserialize)]
pub struct PresencesResponse {
    #[serde(default)]
    pub presences: Vec<Presence>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Presence {
    pub puuid: String,
    #[serde(default)]
    pub product: String,
    /// base64-encoded JSON ([`PrivatePresence`]) — present for the VALORANT product.
    #[serde(default)]
    pub private: String,
}

/// The Valorant state carried in the base64 `private` blob.
///
/// **Schema note (verified live, release-12.11, 2026-06):** newer clients moved
/// the live match fields (`sessionLoopState`, `matchMap`, `queueId`,
/// `provisioningFlow`) into a nested **`matchPresenceData`** object; older clients
/// carried them at the top level, and a `partyPresenceData.partyOwner*` mirror
/// also exists. Only the **score** (`partyOwnerMatchScore*`) stayed top-level —
/// which is why a stale top-level-only model still read the score but not the
/// state/map. Read via the accessor methods, which resolve
/// nested → legacy-top-level → party-owner so we work across client versions.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrivatePresence {
    /// Newer clients: live match state nested here.
    #[serde(rename = "matchPresenceData", default)]
    pub match_data: MatchPresenceData,
    /// Party-owner mirror of the live match state (secondary fallback).
    #[serde(rename = "partyPresenceData", default)]
    pub party_data: PartyPresenceData,

    /// Live score — top-level in every observed client version.
    #[serde(rename = "partyOwnerMatchScoreAllyTeam", default)]
    pub score_ally: i32,
    #[serde(rename = "partyOwnerMatchScoreEnemyTeam", default)]
    pub score_enemy: i32,

    // Legacy top-level fields (older clients) — primary fallback for the nested
    // object. Read through the accessors below, not directly.
    #[serde(rename = "sessionLoopState", default)]
    legacy_loop_state: String,
    #[serde(rename = "matchMap", default)]
    legacy_match_map: String,
    #[serde(rename = "queueId", default)]
    legacy_queue_id: String,
    #[serde(rename = "provisioningFlow", default)]
    legacy_provisioning_flow: String,
}

/// `matchPresenceData` — where current clients put the live match state.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchPresenceData {
    #[serde(rename = "sessionLoopState", default)]
    pub session_loop_state: String,
    #[serde(rename = "matchMap", default)]
    pub match_map: String,
    #[serde(rename = "queueId", default)]
    pub queue_id: String,
    #[serde(rename = "provisioningFlow", default)]
    pub provisioning_flow: String,
}

/// `partyPresenceData` — carries a `partyOwner*` mirror of the live state.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartyPresenceData {
    #[serde(rename = "partyOwnerSessionLoopState", default)]
    pub session_loop_state: String,
    #[serde(rename = "partyOwnerMatchMap", default)]
    pub match_map: String,
    #[serde(rename = "partyOwnerProvisioningFlow", default)]
    pub provisioning_flow: String,
}

impl PrivatePresence {
    /// `sessionLoopState`, resolving nested → legacy top-level → party-owner.
    pub fn session_loop_state(&self) -> &str {
        first_non_empty([
            &self.match_data.session_loop_state,
            &self.legacy_loop_state,
            &self.party_data.session_loop_state,
        ])
    }

    /// The map asset path, same resolution order.
    pub fn match_map(&self) -> &str {
        first_non_empty([
            &self.match_data.match_map,
            &self.legacy_match_map,
            &self.party_data.match_map,
        ])
    }

    /// The queue id (e.g. `competitive`, `swiftplay`), nested → legacy.
    pub fn queue_id(&self) -> &str {
        first_non_empty([&self.match_data.queue_id, &self.legacy_queue_id])
    }

    /// `provisioningFlow` (e.g. `Matchmaking`), nested → legacy → party-owner.
    pub fn provisioning_flow(&self) -> &str {
        first_non_empty([
            &self.match_data.provisioning_flow,
            &self.legacy_provisioning_flow,
            &self.party_data.provisioning_flow,
        ])
    }

    /// The live `sessionLoopState` as a typed [`LoopState`].
    pub fn loop_state(&self) -> LoopState {
        LoopState::parse(self.session_loop_state())
    }
}

/// First non-empty string in `opts`, or `""` if all are empty.
fn first_non_empty<'a, const N: usize>(opts: [&'a String; N]) -> &'a str {
    opts.into_iter()
        .map(String::as_str)
        .find(|s| !s.is_empty())
        .unwrap_or("")
}

/// `sessionLoopState` as a typed value. The state machine in `service.rs`
/// transitions on changes of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LoopState {
    Menus,
    Pregame,
    InGame,
    Unknown,
}

impl LoopState {
    pub fn parse(s: &str) -> Self {
        match s {
            "MENUS" => LoopState::Menus,
            "PREGAME" => LoopState::Pregame,
            "INGAME" => LoopState::InGame,
            _ => LoopState::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Remote API: match-details — post-match
// ---------------------------------------------------------------------------

/// `GET https://pd.{shard}.a.pvp.net/match-details/v1/matches/{matchID}`.
#[derive(Debug, Clone, Deserialize)]
pub struct MatchDetails {
    #[serde(rename = "matchInfo", default)]
    pub match_info: MatchInfo,
    #[serde(default)]
    pub players: Vec<Player>,
    #[serde(rename = "roundResults", default)]
    pub round_results: Vec<RoundResult>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchInfo {
    #[serde(rename = "matchId", default)]
    pub match_id: String,
    #[serde(rename = "mapId", default)]
    pub map_id: String,
    #[serde(rename = "gameLengthMillis", default)]
    pub game_length_millis: i64,
    #[serde(rename = "queueId", default)]
    pub queue_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Player {
    pub puuid: String,
    #[serde(rename = "gameName", default)]
    pub game_name: String,
    #[serde(rename = "tagLine", default)]
    pub tag_line: String,
    #[serde(rename = "teamId", default)]
    pub team_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RoundResult {
    #[serde(rename = "roundNum", default)]
    pub round_num: i32,
    #[serde(rename = "playerStats", default)]
    pub player_stats: Vec<PlayerRoundStats>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerRoundStats {
    pub puuid: String,
    #[serde(default)]
    pub kills: Vec<Kill>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Kill {
    #[serde(rename = "timeSinceGameStartMillis", default)]
    pub time_since_game_start_millis: i64,
    #[serde(rename = "timeSinceRoundStartMillis", default)]
    pub time_since_round_start_millis: i64,
    #[serde(default)]
    pub killer: String,
    #[serde(default)]
    pub victim: String,
    #[serde(rename = "finishingDamage", default)]
    pub finishing_damage: FinishingDamage,
    #[serde(default)]
    pub assistants: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FinishingDamage {
    /// `"Weapon" | "Melee" | "Bomb" | "Ability"`.
    #[serde(rename = "damageType", default)]
    pub damage_type: String,
    /// Weapon GUID, or `"Melee"` / `"Ultimate"` / ability slot.
    #[serde(rename = "damageItem", default)]
    pub damage_item: String,
    #[serde(rename = "isSecondaryFireMode", default)]
    pub is_secondary_fire_mode: bool,
}

impl Kill {
    /// A knife/melee kill ("Knife Kill").
    pub fn is_knife(&self) -> bool {
        self.finishing_damage.damage_type.eq_ignore_ascii_case("Melee")
            || self.finishing_damage.damage_item.eq_ignore_ascii_case("Melee")
    }
}

// ---------------------------------------------------------------------------
// Remote API: live current-game
// ---------------------------------------------------------------------------

/// `GET https://glz-{region}-1.{shard}.a.pvp.net/core-game/v1/players/{puuid}`.
#[derive(Debug, Clone, Deserialize)]
pub struct CurrentGamePlayer {
    #[serde(rename = "MatchID", default)]
    pub match_id: String,
}

// ---------------------------------------------------------------------------
// Derived events (Hako's own — the auto-clip event set)
// ---------------------------------------------------------------------------

/// The highlight kinds Hako can auto-clip. Mirrors Medal's event set
/// (`events.json`). Serialized as the variant name for the UI / event toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    Kill,
    DoubleKill,
    TripleKill,
    QuadraKill,
    Ace,
    Knife,
    Death,
    Assist,
}

impl EventKind {
    /// The multi-kill tier for `n` kills in a single round (n≥1; 5+ ⇒ Ace).
    pub fn for_multikill(n: usize) -> EventKind {
        match n {
            2 => EventKind::DoubleKill,
            3 => EventKind::TripleKill,
            4 => EventKind::QuadraKill,
            n if n >= 5 => EventKind::Ace,
            _ => EventKind::Kill, // 0 or 1
        }
    }
}

/// A derived in-match highlight, positioned in match-relative time. The
/// reconciler (`reconcile.rs`) later maps these to session-file PTS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameEvent {
    pub kind: EventKind,
    /// Round index this event belongs to (`roundNum`).
    pub round: i32,
    /// Anchor time for reconciliation: ms since the game started.
    pub time_since_game_start_millis: i64,
    /// ms since this round started (finer anchor when round starts are known).
    pub time_since_round_start_millis: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loop_state() {
        assert_eq!(LoopState::parse("INGAME"), LoopState::InGame);
        assert_eq!(LoopState::parse("MENUS"), LoopState::Menus);
        assert_eq!(LoopState::parse("PREGAME"), LoopState::Pregame);
        assert_eq!(LoopState::parse("WHATEVER"), LoopState::Unknown);
    }

    #[test]
    fn decodes_nested_private_presence_blob() {
        // Real shape (release-12.11, verified live 2026-06): live state nested in
        // `matchPresenceData`, score top-level.
        let json = r#"{
            "isValid": true,
            "matchPresenceData": {
                "matchMap": "/Game/Maps/Canyon/Canyon",
                "provisioningFlow": "Matchmaking",
                "queueId": "swiftplay",
                "sessionLoopState": "INGAME"
            },
            "partyOwnerMatchScoreAllyTeam": 0,
            "partyOwnerMatchScoreEnemyTeam": 3,
            "partyPresenceData": {
                "partyOwnerMatchMap": "/Game/Maps/Canyon/Canyon",
                "partyOwnerSessionLoopState": "INGAME"
            }
        }"#;
        let p: PrivatePresence = serde_json::from_str(json).unwrap();
        assert_eq!(p.loop_state(), LoopState::InGame);
        assert_eq!(p.match_map(), "/Game/Maps/Canyon/Canyon");
        assert_eq!(p.queue_id(), "swiftplay");
        assert_eq!(p.score_ally, 0);
        assert_eq!(p.score_enemy, 3);
    }

    #[test]
    fn decodes_legacy_top_level_private_blob() {
        // Older clients carried the state at the top level — fallback path.
        let json = r#"{"sessionLoopState":"INGAME","partyOwnerMatchScoreAllyTeam":7,
            "partyOwnerMatchScoreEnemyTeam":5,"matchMap":"/Game/Maps/Ascent/Ascent",
            "queueId":"competitive","provisioningFlow":"Matchmaking"}"#;
        let p: PrivatePresence = serde_json::from_str(json).unwrap();
        assert_eq!(p.loop_state(), LoopState::InGame);
        assert_eq!(p.match_map(), "/Game/Maps/Ascent/Ascent");
        assert_eq!(p.score_ally, 7);
        assert_eq!(p.score_enemy, 5);
        assert_eq!(p.queue_id(), "competitive");
    }

    #[test]
    fn knife_detection() {
        let mut k = Kill::default();
        assert!(!k.is_knife());
        k.finishing_damage.damage_type = "Melee".into();
        assert!(k.is_knife());
    }

    #[test]
    fn multikill_tiers() {
        assert_eq!(EventKind::for_multikill(1), EventKind::Kill);
        assert_eq!(EventKind::for_multikill(2), EventKind::DoubleKill);
        assert_eq!(EventKind::for_multikill(3), EventKind::TripleKill);
        assert_eq!(EventKind::for_multikill(4), EventKind::QuadraKill);
        assert_eq!(EventKind::for_multikill(5), EventKind::Ace);
        assert_eq!(EventKind::for_multikill(6), EventKind::Ace);
    }
}
