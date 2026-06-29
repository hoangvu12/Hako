//! League **Live Client Data API** client (`https://127.0.0.1:2999`).
//!
//! This local HTTPS server (self-signed cert, no auth, localhost-only) is opened
//! by the game process only while a match is actively running. `/allgamedata`
//! returns the live event feed plus our champion + scoreboard in one request, so
//! we poll that each tick. Each event carries an `EventTime` (seconds since
//! GameStart) and a monotonic `EventID` we dedup on.

#![allow(dead_code)]

use serde::Deserialize;

const BASE: &str = "https://127.0.0.1:2999";

/// A thin client over the Live Client Data server.
pub struct LiveClient {
    http: reqwest::Client,
}

impl LiveClient {
    /// Build the client (self-signed localhost). Cheap; reused across polls.
    pub fn new() -> Result<LiveClient, String> {
        Ok(LiveClient {
            http: crate::games::net::insecure_localhost_client()?,
        })
    }

    /// Fetch the full live snapshot. `Err` when no match is running (port closed)
    /// — the caller treats that as "not in game".
    pub async fn all_game_data(&self) -> Result<AllGameData, String> {
        let url = format!("{BASE}/liveclientdata/allgamedata");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("live-client request: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("live-client status {}", resp.status()));
        }
        resp.json::<AllGameData>()
            .await
            .map_err(|e| format!("live-client decode: {e}"))
    }
}

/// `GET /liveclientdata/allgamedata` (subset we consume).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AllGameData {
    #[serde(rename = "events", default)]
    pub events: EventList,
    #[serde(rename = "activePlayer", default)]
    pub active_player: ActivePlayer,
    #[serde(rename = "allPlayers", default)]
    pub all_players: Vec<PlayerEntry>,
    #[serde(rename = "gameData", default)]
    pub game_data: GameData,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EventList {
    #[serde(rename = "Events", default)]
    pub events: Vec<LiveEvent>,
}

/// One entry from the live event feed. Fields beyond the core three are optional
/// and present only for the relevant `EventName`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LiveEvent {
    #[serde(rename = "EventID", default)]
    pub event_id: i64,
    #[serde(rename = "EventName", default)]
    pub event_name: String,
    /// Seconds since GameStart.
    #[serde(rename = "EventTime", default)]
    pub event_time: f64,
    #[serde(rename = "KillerName", default)]
    pub killer_name: String,
    #[serde(rename = "VictimName", default)]
    pub victim_name: String,
    #[serde(rename = "Recipient", default)]
    pub recipient: String,
    #[serde(rename = "Assisters", default)]
    pub assisters: Vec<String>,
    #[serde(rename = "KillStreak", default)]
    pub kill_streak: i64,
    #[serde(rename = "Acer", default)]
    pub acer: String,
    #[serde(rename = "AcingTeam", default)]
    pub acing_team: String,
    #[serde(rename = "DragonType", default)]
    pub dragon_type: String,
    #[serde(rename = "Stolen", default)]
    pub stolen: String,
    #[serde(rename = "Result", default)]
    pub result: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ActivePlayer {
    /// Legacy name field; newer clients use `riotIdGameName`.
    #[serde(rename = "summonerName", default)]
    pub summoner_name: String,
    #[serde(rename = "riotIdGameName", default)]
    pub riot_id_game_name: String,
    /// Full Riot ID ("GameName#TAG").
    #[serde(rename = "riotId", default)]
    pub riot_id: String,
    #[serde(rename = "team", default)]
    pub team: String,
}

impl ActivePlayer {
    /// Our display name as it appears in the event feed (`summonerName` preferred,
    /// then `riotIdGameName`).
    pub fn name(&self) -> &str {
        if !self.summoner_name.is_empty() {
            &self.summoner_name
        } else {
            &self.riot_id_game_name
        }
    }

    /// Every name form the event feed might use for us (summoner name, Riot ID
    /// game name, full Riot ID) — non-empty ones only.
    pub fn name_forms(&self) -> Vec<String> {
        [&self.summoner_name, &self.riot_id_game_name, &self.riot_id]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayerEntry {
    #[serde(rename = "summonerName", default)]
    pub summoner_name: String,
    #[serde(rename = "riotIdGameName", default)]
    pub riot_id_game_name: String,
    /// Full Riot ID ("GameName#TAG").
    #[serde(rename = "riotId", default)]
    pub riot_id: String,
    #[serde(rename = "championName", default)]
    pub champion_name: String,
    #[serde(rename = "team", default)]
    pub team: String,
    #[serde(rename = "scores", default)]
    pub scores: Scores,
}

impl PlayerEntry {
    /// True if this entry is the given player (by any name form).
    pub fn is(&self, name: &str) -> bool {
        (!name.is_empty())
            && (self.summoner_name.eq_ignore_ascii_case(name)
                || self.riot_id_game_name.eq_ignore_ascii_case(name)
                || self.riot_id.eq_ignore_ascii_case(name))
    }

    /// Every name form the event feed might use for this player.
    pub fn name_forms(&self) -> Vec<String> {
        [&self.summoner_name, &self.riot_id_game_name, &self.riot_id]
            .into_iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Scores {
    #[serde(rename = "kills", default)]
    pub kills: i64,
    #[serde(rename = "deaths", default)]
    pub deaths: i64,
    #[serde(rename = "assists", default)]
    pub assists: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct GameData {
    #[serde(rename = "gameMode", default)]
    pub game_mode: String,
    #[serde(rename = "mapName", default)]
    pub map_name: String,
    #[serde(rename = "gameTime", default)]
    pub game_time: f64,
}
