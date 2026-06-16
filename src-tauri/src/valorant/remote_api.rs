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

use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

use crate::valorant::model::{CurrentGamePlayer, MatchDetails};

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

/// Authenticated remote client. Build once per session from local-API tokens.
pub struct RemoteClient {
    http: reqwest::Client,
    region: String,
    shard: String,
}

impl RemoteClient {
    /// `region` from the chat session; tokens from the entitlements endpoint.
    pub fn new(
        region: &str,
        access_token: &str,
        entitlements_jwt: &str,
        client_version: &str,
    ) -> Result<RemoteClient, String> {
        let mut headers = HeaderMap::new();
        let mut set = |k: reqwest::header::HeaderName, v: &str| -> Result<(), String> {
            headers.insert(k, HeaderValue::from_str(v).map_err(|e| e.to_string())?);
            Ok(())
        };
        set(AUTHORIZATION, &format!("Bearer {access_token}"))?;
        set(
            reqwest::header::HeaderName::from_static("x-riot-entitlements-jwt"),
            entitlements_jwt,
        )?;
        set(
            reqwest::header::HeaderName::from_static("x-riot-clientversion"),
            client_version,
        )?;
        set(
            reqwest::header::HeaderName::from_static("x-riot-clientplatform"),
            &client_platform_header(),
        )?;
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| format!("build remote client: {e}"))?;
        Ok(RemoteClient {
            http,
            region: region.to_string(),
            shard: region_to_shard(region).to_string(),
        })
    }

    /// Live match id, or `None` if we're not currently in a game (404).
    pub async fn current_match_id(&self, puuid: &str) -> Result<Option<String>, String> {
        let url = format!(
            "https://glz-{}-1.{}.a.pvp.net/core-game/v1/players/{}",
            self.region, self.shard, puuid
        );
        let resp = self
            .http
            .get(&url)
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

    /// Post-match details. 404 means the match isn't finalized yet.
    pub async fn match_details(&self, match_id: &str) -> Result<MatchDetails, String> {
        let url = format!(
            "https://pd.{}.a.pvp.net/match-details/v1/matches/{}",
            self.shard, match_id
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("match-details: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("match-details → HTTP {}", resp.status()));
        }
        resp.json().await.map_err(|e| format!("decode match-details: {e}"))
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
