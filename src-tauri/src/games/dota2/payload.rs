//! Dota 2 Game-State-Integration payload — serde structs + validation.
//!
//! Dota 2 POSTs a JSON snapshot on every state change (the `player`, `hero`,
//! `map`, `events`, `items` components we request). We deserialize the subset we
//! need and validate it the way Medal's `Dota2Parser` does: `map`, `player`, and
//! `hero.name` must be present. Unlike CS2 there's no spectator cross-check —
//! Dota's GSI stream is single-player, always the local client.
//!
//! [`parse_valid`] flattens a valid raw payload into an owned [`ValidPayload`]
//! so the diff in [`super::events`] stays serde-free and unit-testable. We
//! deliberately skip the `events`/`items` (Aegis) paths — low value, and Medal's
//! item-slot scan is large; per the plan it can be deferred.

#![allow(dead_code)]

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Payload {
    pub auth: Option<Auth>,
    pub player: Option<Player>,
    pub map: Option<Map>,
    pub hero: Option<Hero>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Player {
    #[serde(default)]
    pub kills: i32,
    #[serde(default)]
    pub deaths: i32,
    #[serde(default)]
    pub assists: i32,
    #[serde(default)]
    pub accountid: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Map {
    #[serde(default)]
    pub game_time: i32,
    #[serde(default)]
    pub matchid: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Hero {
    #[serde(default)]
    pub name: String,
}

/// A validated Dota 2 payload flattened to owned fields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValidPayload {
    pub game_time: i32,
    pub match_id: String,
    pub hero: String,
    pub player_name: String,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
}

/// Parse a raw GSI body into a [`ValidPayload`], or `None` if it doesn't
/// deserialize / is missing `map`, `player`, or `hero.name`.
pub fn parse_valid(body: &str) -> Option<ValidPayload> {
    let p: Payload = serde_json::from_str(body).ok()?;
    let map = p.map?;
    let player = p.player?;
    let hero = p.hero?;
    if hero.name.is_empty() {
        return None;
    }
    Some(ValidPayload {
        game_time: map.game_time,
        match_id: map.matchid,
        hero: hero.name,
        player_name: player.name,
        kills: player.kills,
        deaths: player.deaths,
        assists: player.assists,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_payload() {
        let body = r#"{
            "auth": {"token": "t"},
            "player": {"kills": 3, "deaths": 1, "assists": 4, "accountid": "42", "name": "me"},
            "map": {"game_time": 620, "matchid": "999"},
            "hero": {"name": "npc_dota_hero_juggernaut"}
        }"#;
        let v = parse_valid(body).unwrap();
        assert_eq!(v.game_time, 620);
        assert_eq!(v.kills, 3);
        assert_eq!(v.hero, "npc_dota_hero_juggernaut");
    }

    #[test]
    fn rejects_missing_hero_or_objects() {
        // No hero name.
        let no_hero = r#"{"auth":{"token":"t"},"player":{},"map":{},"hero":{}}"#;
        assert!(parse_valid(no_hero).is_none());
        assert!(parse_valid("{}").is_none());
        assert!(parse_valid("nope").is_none());
    }
}
