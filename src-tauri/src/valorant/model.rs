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
    /// Chat connection state; Medal waits until this is `"connected"` before
    /// trusting the puuid/identity (see `service::start_session`).
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub game_name: String,
    #[serde(default)]
    pub game_tag: String,
}

impl ChatSession {
    /// Riot reports `"connected"` once the chat session is fully established.
    pub fn is_connected(&self) -> bool {
        self.state == "connected"
    }
}

/// `GET /product-session/v1/external-sessions` — a map of running Riot product
/// sessions keyed by an opaque id. Used to find the Valorant launch arguments
/// (the `-ares-deployment=<shard>` flag carries our region/shard).
pub type ExternalSessions = std::collections::HashMap<String, ExternalSession>;

#[derive(Debug, Clone, Deserialize)]
pub struct ExternalSession {
    #[serde(rename = "productId", default)]
    pub product_id: String,
    #[serde(rename = "launchConfiguration", default)]
    pub launch_configuration: LaunchConfiguration,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LaunchConfiguration {
    #[serde(default)]
    pub arguments: Vec<String>,
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
    #[serde(default, deserialize_with = "null_default")]
    pub puuid: String,
    #[serde(default, deserialize_with = "null_default")]
    pub product: String,
    /// base64-encoded JSON ([`PrivatePresence`]) — present for the VALORANT product.
    /// Other Riot products a friend is running (e.g. League) can send
    /// `private: null`; `null_default` collapses that to `""`. Plain
    /// `#[serde(default)]` only covers a *missing* key — a present-but-`null`
    /// value hard-fails, and since the whole `/chat/v4/presences` array decodes
    /// at once, one such entry would silently kill match detection for everyone.
    #[serde(default, deserialize_with = "null_default")]
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

/// Deserialize helper: treat JSON `null` (and, with `#[serde(default)]`, a missing
/// key) as `T::default()`. Riot's match-details returns `null` for several fields
/// (the whole `players[].stats` object, `gameLengthMillis`, various arrays), and
/// plain `#[serde(default)]` only covers a *missing* key — a present-but-`null`
/// value still hard-fails the decode. Pair both: `#[serde(default, deserialize_with = "null_default")]`.
fn null_default<'de, D, T>(de: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(de)?.unwrap_or_default())
}

/// `GET https://pd.{shard}.a.pvp.net/match-details/v1/matches/{matchID}`.
#[derive(Debug, Clone, Deserialize)]
pub struct MatchDetails {
    #[serde(rename = "matchInfo", default)]
    pub match_info: MatchInfo,
    #[serde(default)]
    pub players: Vec<Player>,
    /// Teams (with win flag) — used for the post-match win/loss result.
    #[serde(default)]
    pub teams: Vec<Team>,
    #[serde(rename = "roundResults", default)]
    pub round_results: Vec<RoundResult>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MatchInfo {
    #[serde(rename = "matchId", default)]
    pub match_id: String,
    #[serde(rename = "mapId", default)]
    pub map_id: String,
    #[serde(rename = "gameLengthMillis", default, deserialize_with = "null_default")]
    pub game_length_millis: i64,
    #[serde(rename = "gameStartMillis", default, deserialize_with = "null_default")]
    pub game_start_millis: i64,
    #[serde(rename = "queueID", alias = "queueId", default)]
    pub queue_id: String,
    /// Game-mode asset path (e.g. `/Game/GameModes/Bomb/BombGameMode.BombGameMode_C`).
    /// Mapped to a display name via [`game_mode_name`]; drives the Skirmish offset.
    #[serde(rename = "gameMode", default)]
    pub game_mode: String,
}

/// Map a Valorant `gameMode` asset path to its display name. Port of Medal's
/// `GameModeUtility`. Empty string for unknown/unmapped modes.
pub fn game_mode_name(asset: &str) -> &'static str {
    match asset {
        "/Game/GameModes/Bomb/BombGameMode.BombGameMode_C" => "Standard",
        "/Game/GameModes/Deathmatch/DeathmatchGameMode.DeathmatchGameMode_C" => "Deathmatch",
        "/Game/GameModes/GunGame/GunGameTeamsGameMode.GunGameTeamsGameMode_C" => "Escalation",
        "/Game/GameModes/NewPlayerExperience/NPEGameMode.NPEGameMode_C" => "Onboarding",
        "/Game/GameModes/OneForAll/OneForAll_GameMode.OneForAll_GameMode_C" => "Replication",
        "/Game/GameModes/QuickBomb/QuickBombGameMode.QuickBombGameMode_C" => "Spike Rush",
        "/Game/GameModes/ShootingRange/ShootingRangeGameMode.ShootingRangeGameMode_C" => "PRACTICE",
        "/Game/GameModes/SnowballFight/SnowballFightGameMode.SnowballFightGameMode_C" => {
            "Snowball Fight"
        }
        "/Game/GameModes/_Development/Swiftplay_EndOfRoundCredits/Swiftplay_EoRCredits_GameMode.Swiftplay_EoRCredits_GameMode_C" => "Swiftplay",
        "/Game/GameModes/Skirmish/SkirmishGameMode.SkirmishGameMode_C" => "Skirmish",
        _ => "",
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Player {
    /// Player UUID. Riot's match-details calls this `subject` (NOT `puuid`,
    /// despite what some docs show) — verified live against `pd.ap` 2026-06.
    #[serde(rename = "subject", alias = "puuid", default)]
    pub puuid: String,
    #[serde(rename = "gameName", default, deserialize_with = "null_default")]
    pub game_name: String,
    #[serde(rename = "tagLine", default, deserialize_with = "null_default")]
    pub tag_line: String,
    #[serde(rename = "teamId", default, deserialize_with = "null_default")]
    pub team_id: String,
    /// Agent UUID (`characterId`); resolved to a display name via valorant-api.com.
    #[serde(rename = "characterId", default, deserialize_with = "null_default")]
    pub character_id: String,
    /// Match totals (kills/deaths/assists). The whole object is `null` for some
    /// players/modes, so `null_default` collapses that to a zeroed `PlayerStats`.
    #[serde(default, deserialize_with = "null_default")]
    pub stats: PlayerStats,
}

/// `players[].stats` — the player's match totals.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerStats {
    #[serde(default)]
    pub kills: i32,
    #[serde(default)]
    pub deaths: i32,
    #[serde(default)]
    pub assists: i32,
}

/// `teams[]` — carries the win flag per team id.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Team {
    #[serde(rename = "teamId", default)]
    pub team_id: String,
    #[serde(default)]
    pub won: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoundResult {
    #[serde(rename = "roundNum", default, deserialize_with = "null_default")]
    pub round_num: i32,
    #[serde(rename = "playerStats", default, deserialize_with = "null_default")]
    pub player_stats: Vec<PlayerRoundStats>,
    /// How the round ended: `"Elimination"` | `"Bomb detonated"` |
    /// `"Bomb defused"` | `"Round timer expired"` | `"Surrendered"`. Riot's
    /// machine-readable companion is `roundResultCode` ([`Self::result_code`]).
    #[serde(rename = "roundResult", default, deserialize_with = "null_default")]
    pub round_result: String,
    /// Stable code for the round outcome: `"Elimination"` | `"Detonate"` |
    /// `"Defuse"` | `"Surrendered"`.
    #[serde(rename = "roundResultCode", default, deserialize_with = "null_default")]
    pub round_result_code: String,
    /// Team id that won the round (`"Blue"` / `"Red"`).
    #[serde(rename = "winningTeam", default, deserialize_with = "null_default")]
    pub winning_team: String,
    /// puuid of whoever planted the spike this round (empty if no plant).
    #[serde(rename = "bombPlanter", default, deserialize_with = "null_default")]
    pub bomb_planter: String,
    /// puuid of whoever defused the spike this round (empty if no defuse).
    #[serde(rename = "bombDefuser", default, deserialize_with = "null_default")]
    pub bomb_defuser: String,
    /// ms since round start at which the spike was planted (0 if no plant).
    #[serde(rename = "plantRoundTime", default, deserialize_with = "null_default")]
    pub plant_round_time: i64,
    /// ms since round start at which the spike was defused (0 if no defuse).
    #[serde(rename = "defuseRoundTime", default, deserialize_with = "null_default")]
    pub defuse_round_time: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlayerRoundStats {
    #[serde(rename = "subject", alias = "puuid", default)]
    pub puuid: String,
    #[serde(default, deserialize_with = "null_default")]
    pub kills: Vec<Kill>,
    /// Per-weapon damage tally this round — used for headshot %.
    #[serde(default, deserialize_with = "null_default")]
    pub damage: Vec<DamageEvent>,
}

/// `playerStats[].damage[]` — shot-location counts, for headshot %.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DamageEvent {
    #[serde(default)]
    pub legshots: i32,
    #[serde(default)]
    pub bodyshots: i32,
    #[serde(default)]
    pub headshots: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Kill {
    /// ms since game start. Riot's key is `gameTime` (NOT `timeSinceGameStartMillis`)
    /// — verified live 2026-06; the old name decoded to 0 and wrecked clip timing.
    #[serde(rename = "gameTime", alias = "timeSinceGameStartMillis", default, deserialize_with = "null_default")]
    pub time_since_game_start_millis: i64,
    /// ms since round start. Riot's key is `roundTime`.
    #[serde(rename = "roundTime", alias = "timeSinceRoundStartMillis", default, deserialize_with = "null_default")]
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

/// The highlight kinds Hako can auto-clip. Mirrors Medal's / Outplayed's event
/// set. Serialized as the variant name for the UI / event toggles.
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
    /// We won the match (anchored at the final round's last action).
    Victory,
    /// We won a round as the last player alive on our team (a 1vX clutch).
    Clutch,
    /// A spike we planted detonated (round won by detonation).
    SpikeDetonated,
    /// We defused the spike.
    SpikeDefused,
}

impl EventKind {
    /// Human label for clip titles / library tags (e.g. "Triple Kill", "Ace").
    pub fn label(self) -> &'static str {
        match self {
            EventKind::Kill => "Kill",
            EventKind::DoubleKill => "Double Kill",
            EventKind::TripleKill => "Triple Kill",
            EventKind::QuadraKill => "Quadra Kill",
            EventKind::Ace => "Ace",
            EventKind::Knife => "Knife",
            EventKind::Death => "Death",
            EventKind::Assist => "Assist",
            EventKind::Victory => "Victory",
            EventKind::Clutch => "Clutch",
            EventKind::SpikeDetonated => "Spike Detonated",
            EventKind::SpikeDefused => "Spike Defused",
        }
    }

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
    fn decodes_real_match_details_shape() {
        // Minimal slice of the REAL pd.{shard} match-details body (verified live
        // 2026-06): player id is `subject`, kills use `gameTime`/`roundTime`,
        // matchInfo uses `queueID`, and `stats` can be null.
        let json = r#"{
            "matchInfo": { "matchId": "abc", "mapId": "/Game/Maps/Ascent/Ascent",
                "gameLengthMillis": null, "gameStartMillis": 1781697024000,
                "queueID": "swiftplay",
                "gameMode": "/Game/GameModes/Bomb/BombGameMode.BombGameMode_C" },
            "players": [
                { "subject": "p1", "gameName": "", "tagLine": null, "teamId": "Blue",
                  "characterId": "agent-1",
                  "stats": { "kills": 7, "deaths": 4, "assists": 2 } },
                { "subject": "p2", "teamId": "Red", "stats": null }
            ],
            "teams": [ { "teamId": "Blue", "won": true } ],
            "roundResults": [
                { "roundNum": 0, "playerStats": [
                    { "subject": "p1",
                      "kills": [ { "gameTime": 109409, "roundTime": 54331,
                          "killer": "p1", "victim": "p2",
                          "finishingDamage": { "damageType": "Weapon", "damageItem": "x" } } ],
                      "damage": [ { "legshots": 0, "bodyshots": 1, "headshots": 2 } ] }
                ] }
            ]
        }"#;
        let md: MatchDetails = serde_json::from_str(json).expect("real shape must decode");
        assert_eq!(md.match_info.queue_id, "swiftplay");
        assert_eq!(md.match_info.game_length_millis, 0); // null → default
        assert_eq!(md.players[0].puuid, "p1"); // from `subject`
        assert_eq!(md.players[0].stats.kills, 7);
        assert_eq!(md.players[1].puuid, "p2");
        assert_eq!(md.players[1].stats.kills, 0); // stats: null → zeroed
        assert!(md.teams[0].won);
        let kill = &md.round_results[0].player_stats[0].kills[0];
        assert_eq!(md.round_results[0].player_stats[0].puuid, "p1");
        assert_eq!(kill.time_since_game_start_millis, 109409); // from `gameTime`
        assert_eq!(kill.time_since_round_start_millis, 54331); // from `roundTime`
        assert_eq!(md.round_results[0].player_stats[0].damage[0].headshots, 2);
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
