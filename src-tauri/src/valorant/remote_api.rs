//! Riot remote pvp.net endpoints.
//!
//! **Unofficial/undocumented** — read-only, rate-limited, and isolated here so
//! breakage degrades to manual/hotkey clips. Requires
//! the tokens from the local API (`local_api`): access token + entitlements JWT,
//! plus a client-version and client-platform header.
//!
//! - Live current game: `glz-{region}-1.{shard}.a.pvp.net/core-game/v1/players/{puuid}` → `MatchID`.
//! - Post-match details: `pd.{shard}.a.pvp.net/match-details/v1/matches/{matchID}`
//!   (404s mid-match — only available after the match ends, the reason Mode B
//!   records the whole match).

#![allow(dead_code)]

use std::sync::RwLock;

use base64::Engine;
use reqwest::header::AUTHORIZATION;

use crate::valorant::local_api::LocalClient;
use crate::valorant::model::{CoreGameMatch, CurrentGamePlayer, MatchDetails};

/// Fixed client-platform JSON, base64'd into `X-Riot-ClientPlatform`.
const CLIENT_PLATFORM_JSON: &str = r#"{"platformType":"PC","platformOS":"Windows","platformOSVersion":"10.0.19042.1.256.64bit","platformChipset":"Unknown"}"#;

pub fn client_platform_header() -> String {
    base64::engine::general_purpose::STANDARD.encode(CLIENT_PLATFORM_JSON.as_bytes())
}

/// Map a Riot region/affinity to its pvp.net shard. Most regions are 1:1; the
/// Americas sub-regions collapse onto `na`.
pub fn region_to_shard(region: &str) -> &'static str {
    match region.to_ascii_lowercase().as_str() {
        "na" | "latam" | "br" => "na",
        "eu" => "eu",
        "ap" => "ap",
        "kr" => "kr",
        _ => "na",
    }
}

/// Fetch the live Valorant client version (`X-Riot-ClientVersion`) from the
/// community mirror — simpler than reading it from the local product session.
pub async fn fetch_client_version(http: &reqwest::Client) -> Result<String, String> {
    #[derive(serde::Deserialize)]
    struct Resp {
        data: Data,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(rename = "riotClientVersion")]
        riot_client_version: String,
    }
    let r: Resp = http
        .get("https://valorant-api.com/v1/version")
        .send()
        .await
        .map_err(|e| format!("fetch version: {e}"))?
        .json()
        .await
        .map_err(|e| format!("decode version: {e}"))?;
    Ok(r.data.riot_client_version)
}

/// Resolve an agent's display name from its `characterId` via the community
/// mirror (Medal's `Utils.GetAgentDisplayName`). Public endpoint, no auth.
/// `None` on any failure (the caller falls back to "Unknown").
pub async fn fetch_agent_name(agent_id: &str) -> Option<String> {
    if agent_id.is_empty() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        data: Data,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(rename = "displayName", default)]
        display_name: String,
    }
    let url = format!("https://valorant-api.com/v1/agents/{agent_id}");
    let r: Resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    (!r.data.display_name.is_empty()).then_some(r.data.display_name)
}

/// The auth headers that go stale and must be refreshed before remote calls.
/// `client_version` is stable for the session; the access token + entitlements
/// JWT expire, so [`RemoteClient::refresh_tokens`] re-reads them from the local
/// API before each match-details call (mirroring Medal's `RefreshTokens`).
struct Auth {
    access_token: String,
    entitlements_jwt: String,
    client_version: String,
}

/// Authenticated remote client. Tokens live behind a lock and are applied
/// **per request** (not baked into the client) so they can be refreshed without
/// rebuilding the client — Riot's tokens expire and a match can run for
/// 30+ minutes before we fetch its details.
pub struct RemoteClient {
    http: reqwest::Client,
    region: String,
    shard: String,
    auth: RwLock<Auth>,
}

impl RemoteClient {
    /// Build with an explicit region **and** shard. Medal sets both to the
    /// `-ares-deployment=` value (so `glz-{region}-1.{shard}` and `pd.{shard}`
    /// match the live client); the bootstrap passes that through here.
    pub fn with_region_shard(
        region: &str,
        shard: &str,
        access_token: &str,
        entitlements_jwt: &str,
        client_version: &str,
    ) -> Result<RemoteClient, String> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("build remote client: {e}"))?;
        Ok(RemoteClient {
            http,
            region: region.to_string(),
            shard: shard.to_string(),
            auth: RwLock::new(Auth {
                access_token: access_token.to_string(),
                entitlements_jwt: entitlements_jwt.to_string(),
                client_version: client_version.to_string(),
            }),
        })
    }

    /// Apply the four required Riot headers (current token snapshot) to a request.
    /// The read guard is released before the caller awaits `send()`.
    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let a = self.auth.read().expect("auth lock poisoned");
        rb.header(AUTHORIZATION, format!("Bearer {}", a.access_token))
            .header("x-riot-entitlements-jwt", a.entitlements_jwt.clone())
            .header("x-riot-clientversion", a.client_version.clone())
            .header("x-riot-clientplatform", client_platform_header())
    }

    /// Re-read the entitlements token + JWT from the local API and swap them in.
    /// Medal does this before **every** remote call; we do it before each
    /// match-details attempt, since the match-start token is long stale by then.
    pub async fn refresh_tokens(&self, local: &LocalClient) -> Result<(), String> {
        let ent = local.entitlements().await?;
        let mut a = self.auth.write().expect("auth lock poisoned");
        a.access_token = ent.access_token;
        a.entitlements_jwt = ent.token;
        Ok(())
    }

    /// `region` from the chat session; the shard is derived via the
    /// [`region_to_shard`] heuristic. Prefer [`with_region_shard`](Self::with_region_shard)
    /// with the parsed `-ares-deployment=` value when available.
    pub fn new(
        region: &str,
        access_token: &str,
        entitlements_jwt: &str,
        client_version: &str,
    ) -> Result<RemoteClient, String> {
        Self::with_region_shard(
            region,
            region_to_shard(region),
            access_token,
            entitlements_jwt,
            client_version,
        )
    }

    /// The glz affinity/region this client targets.
    pub fn region(&self) -> &str {
        &self.region
    }

    /// The pvp.net shard this client targets.
    pub fn shard(&self) -> &str {
        &self.shard
    }

    /// Live match id, or `None` if we're not currently in a game (404).
    pub async fn current_match_id(&self, puuid: &str) -> Result<Option<String>, String> {
        let url = format!(
            "https://glz-{}-1.{}.a.pvp.net/core-game/v1/players/{}",
            self.region, self.shard, puuid
        );
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| format!("current-game: {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(format!("current-game → HTTP {}", resp.status()));
        }
        let p: CurrentGamePlayer = resp.json().await.map_err(|e| format!("decode: {e}"))?;
        Ok(if p.match_id.is_empty() {
            None
        } else {
            Some(p.match_id)
        })
    }

    /// The **live** (in-progress) match: players (with their agents), map, mode.
    /// Available mid-match (unlike `match-details`); 404s once the match ends.
    /// Used to resolve our agent for tagging manual clips.
    pub async fn core_game_match(&self, match_id: &str) -> Result<CoreGameMatch, String> {
        let url = format!(
            "https://glz-{}-1.{}.a.pvp.net/core-game/v1/matches/{}",
            self.region, self.shard, match_id
        );
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| format!("core-game match: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("core-game match → HTTP {}", resp.status()));
        }
        resp.json()
            .await
            .map_err(|e| format!("decode core-game match: {e}"))
    }

    /// Post-match details. 404 means the match isn't finalized yet.
    pub async fn match_details(&self, match_id: &str) -> Result<MatchDetails, String> {
        let url = format!(
            "https://pd.{}.a.pvp.net/match-details/v1/matches/{}",
            self.shard, match_id
        );
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| format!("match-details: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("match-details → HTTP {}", resp.status()));
        }
        // Decode from text (not resp.json()) so a schema mismatch yields the exact
        // serde path/line — reqwest's own decode error is opaque ("error decoding
        // response body"), which hides which field broke.
        let body = resp
            .text()
            .await
            .map_err(|e| format!("match-details read body: {e}"))?;
        serde_json::from_str(&body).map_err(|e| format!("decode match-details: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_platform_is_stable_base64() {
        // Must decode back to the exact fixed JSON.
        let b64 = client_platform_header();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .unwrap();
        assert_eq!(String::from_utf8(bytes).unwrap(), CLIENT_PLATFORM_JSON);
    }

    #[test]
    fn region_shard_mapping() {
        assert_eq!(region_to_shard("na"), "na");
        assert_eq!(region_to_shard("LATAM"), "na");
        assert_eq!(region_to_shard("eu"), "eu");
        assert_eq!(region_to_shard("kr"), "kr");
        assert_eq!(region_to_shard("weird"), "na");
    }
}
