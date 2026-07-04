//! War Thunder web-HUD client (`http://127.0.0.1:8111`).
//!
//! The running client serves two localhost JSON endpoints we consume:
//! - `GET /hudmsg?lastEvt=<n>&lastDmg=<id>` — the scrolling combat log. We only
//!   read the `damage` array (`{ id, msg }` rows); `id` increases monotonically
//!   *within a battle*. Passing the last `id` we've seen returns only newer rows.
//!   The engine restarts `id` numbering each new battle, so a returned `id` that
//!   is **below** our stored `last_dmg` is our battle-boundary signal.
//! - `GET /indicators` — `{ valid, army, … }`. We derive the local vehicle class
//!   (Air / Ground / Naval), needed only for the "a plane is *shot down*, never
//!   *destroyed*" disambiguation in [`super::events`].
//!
//! A hung socket is bounded by the shared short timeout ([`crate::games::net`]),
//! so a poll tick never stalls the integration loop.

#![allow(dead_code)]

use serde::Deserialize;

/// Base URL of the local web-HUD server (localhost only; opened by the client).
const BASE: &str = "http://127.0.0.1:8111";

/// The local player's vehicle class this battle, from `/indicators`. Used only to
/// disambiguate the ambiguous "destroyed" verb (see [`super::events::classify`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vehicle {
    /// Aircraft (`valid && army == "air"`). Planes are "shot down", not "destroyed".
    Air,
    /// Ground vehicle (`valid && army == "tank"`).
    Ground,
    /// Ship (`valid && army == "ship"`), or any other valid non-air/ground army.
    Naval,
    /// Not in a vehicle (`!valid`) — menus / spectator / between spawns.
    #[default]
    Unknown,
}

/// One `/hudmsg` `damage` row: a monotonically-`id`'d combat-log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DamageRow {
    pub id: i32,
    pub msg: String,
}

/// The result of one `/hudmsg` poll: the new damage rows, plus whether a battle
/// boundary was detected (the engine's `id` numbering reset below `last_dmg`).
#[derive(Debug, Clone, Default)]
pub struct HudPoll {
    pub rows: Vec<DamageRow>,
    /// A fresh battle began (id numbering restarted) — finalize the previous one.
    pub reset: bool,
}

/// A thin stateful client over the War Thunder web-HUD server. Tracks the last
/// damage `id` seen so each poll asks only for newer rows.
pub struct WarThunderApi {
    http: reqwest::Client,
    /// Highest damage `id` consumed so far this battle (0 before the first row).
    last_dmg: i32,
}

impl WarThunderApi {
    /// Build the client (plain localhost HTTP). Cheap; reused across polls.
    pub fn new() -> Result<WarThunderApi, String> {
        Ok(WarThunderApi {
            http: crate::games::net::plain_localhost_client()?,
            last_dmg: 0,
        })
    }

    /// Fetch damage rows newer than `last_dmg`, advancing our cursor. Detects a
    /// battle reset (id numbering restarted) and, when it fires, rewinds the
    /// cursor so the new battle's opening rows aren't skipped. `Err` when the
    /// server is unreachable (no battle) — the caller treats that as "no rows".
    pub async fn poll_damage(&mut self) -> Result<HudPoll, String> {
        let url = format!("{BASE}/hudmsg?lastEvt=0&lastDmg={}", self.last_dmg);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("hudmsg request: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("hudmsg status {}", resp.status()));
        }
        let body: HudMsg = resp
            .json()
            .await
            .map_err(|e| format!("hudmsg decode: {e}"))?;

        let rows: Vec<DamageRow> = body
            .damage
            .into_iter()
            .filter(|d| !d.msg.trim().is_empty())
            .map(|d| DamageRow { id: d.id, msg: d.msg })
            .collect();

        // Battle boundary: the engine restarts `id` at the start of each battle,
        // so any returned id below our cursor means the previous battle ended and
        // a new one began. Rewind to 0 so none of the new battle's rows are lost.
        let min_id = rows.iter().map(|r| r.id).min();
        let reset = matches!(min_id, Some(m) if m < self.last_dmg);
        if reset {
            self.last_dmg = 0;
        }
        if let Some(max_id) = rows.iter().map(|r| r.id).max() {
            self.last_dmg = self.last_dmg.max(max_id);
        }

        Ok(HudPoll { rows, reset })
    }

    /// Fetch the local vehicle class from `/indicators` ([`Vehicle::Unknown`] when
    /// the server is unreachable or reports an invalid/absent vehicle).
    pub async fn poll_vehicle(&self) -> Vehicle {
        let url = format!("{BASE}/indicators");
        let Ok(resp) = self.http.get(&url).send().await else {
            return Vehicle::Unknown;
        };
        if !resp.status().is_success() {
            return Vehicle::Unknown;
        }
        match resp.json::<Indicators>().await {
            Ok(ind) => ind.vehicle(),
            Err(_) => Vehicle::Unknown,
        }
    }
}

/// `GET /hudmsg` (the subset we consume — only the damage log).
#[derive(Debug, Clone, Default, Deserialize)]
struct HudMsg {
    #[serde(default)]
    damage: Vec<RawDamage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawDamage {
    #[serde(default)]
    id: i32,
    #[serde(default)]
    msg: String,
}

/// `GET /indicators` (only the fields that classify the vehicle).
#[derive(Debug, Clone, Default, Deserialize)]
struct Indicators {
    #[serde(default)]
    valid: bool,
    #[serde(default)]
    army: String,
}

impl Indicators {
    fn vehicle(&self) -> Vehicle {
        if !self.valid {
            return Vehicle::Unknown;
        }
        match self.army.to_ascii_lowercase().as_str() {
            "air" => Vehicle::Air,
            "tank" => Vehicle::Ground,
            _ => Vehicle::Naval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indicators_classifies_army() {
        let air = Indicators { valid: true, army: "air".into() };
        assert_eq!(air.vehicle(), Vehicle::Air);
        let tank = Indicators { valid: true, army: "tank".into() };
        assert_eq!(tank.vehicle(), Vehicle::Ground);
        let ship = Indicators { valid: true, army: "ship".into() };
        assert_eq!(ship.vehicle(), Vehicle::Naval);
        // Invalid ⇒ not in a vehicle, regardless of `army`.
        let none = Indicators { valid: false, army: "air".into() };
        assert_eq!(none.vehicle(), Vehicle::Unknown);
    }
}
