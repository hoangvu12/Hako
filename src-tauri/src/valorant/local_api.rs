//! Riot local client API.
//!
//! Talks to `https://127.0.0.1:{port}` (lockfile auth, self-signed cert accepted
//! for localhost only). Polls `/chat/v4/presences` every ~2 s for the Valorant
//! `sessionLoopState` + live score — each score increment is a wall-clock-stamped
//! round boundary (our reconciliation anchor).
//!
//! The local WAMP websocket (`OnJsonApiEvent`) is preferred over polling;
//! polling is the documented fallback and is what we ship first (simpler, no TLS
//! websocket to the self-signed localhost endpoint). Websocket is a later
//! refinement — the round-boundary anchor is identical either way.

#![allow(dead_code)]

use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::de::DeserializeOwned;

use crate::valorant::lockfile::{self, Lockfile};
use crate::valorant::model::{
    ChatSession, EntitlementsToken, ExternalSessions, Presence, PresencesResponse, PrivatePresence,
};

/// A connected local-API client (lives as long as the Riot client's lockfile).
pub struct LocalClient {
    http: reqwest::Client,
    base: String,
}

impl LocalClient {
    /// Read the lockfile and build an authenticated client. Errors if Riot
    /// isn't running (no lockfile) — the caller degrades to manual clips.
    pub fn connect() -> Result<LocalClient, String> {
        let lf = lockfile::read()?;
        Self::from_lockfile(&lf)
    }

    pub fn from_lockfile(lf: &Lockfile) -> Result<LocalClient, String> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&lf.basic_auth_header()).map_err(|e| e.to_string())?,
        );
        let http = reqwest::Client::builder()
            .danger_accept_invalid_certs(true) // self-signed localhost cert
            .default_headers(headers)
            .build()
            .map_err(|e| format!("build local http client: {e}"))?;
        Ok(LocalClient {
            http,
            base: lf.base_url(),
        })
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("GET {path}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("GET {path} → HTTP {}", resp.status()));
        }
        resp.json::<T>()
            .await
            .map_err(|e| format!("decode {path}: {e}"))
    }

    /// `GET /entitlements/v1/token` — tokens for the remote API.
    pub async fn entitlements(&self) -> Result<EntitlementsToken, String> {
        self.get_json("/entitlements/v1/token").await
    }

    /// `GET /chat/v1/session` — our puuid + region.
    pub async fn chat_session(&self) -> Result<ChatSession, String> {
        self.get_json("/chat/v1/session").await
    }

    /// `GET /product-session/v1/external-sessions` — running Riot product
    /// sessions, keyed by id. We parse the Valorant entry's launch arguments for
    /// `-ares-deployment=<shard>` (our region/shard).
    pub async fn sessions(&self) -> Result<ExternalSessions, String> {
        self.get_json("/product-session/v1/external-sessions").await
    }

    /// Find the `valorant` product session and parse its `-ares-deployment=`
    /// launch argument — the shard/region Medal anchors all pvp.net calls on.
    /// `None` if Valorant isn't in the session list (e.g. only the launcher is up).
    pub async fn valorant_deployment(&self) -> Result<Option<String>, String> {
        Ok(parse_valorant_deployment(&self.sessions().await?))
    }

    /// `GET /chat/v4/presences` — all presences (ours carries the Valorant blob).
    pub async fn presences(&self) -> Result<Vec<Presence>, String> {
        let r: PresencesResponse = self.get_json("/chat/v4/presences").await?;
        Ok(r.presences)
    }

    /// Our decoded Valorant presence (`sessionLoopState` + live score), or `None`
    /// if we have no VALORANT presence yet.
    pub async fn our_presence(&self, puuid: &str) -> Result<Option<PrivatePresence>, String> {
        for p in self.presences().await? {
            if p.puuid == puuid && !p.private.is_empty() {
                return Ok(Some(decode_private(&p.private)?));
            }
        }
        Ok(None)
    }
}

/// Find the `valorant` product in an external-sessions map and extract its
/// `-ares-deployment=<shard>` launch argument. Mirrors Medal's
/// `RetrieveAndParseSessionInfo`: first valorant product, first arg with the
/// prefix, value after `=`. `None` if absent/empty.
pub fn parse_valorant_deployment(sessions: &ExternalSessions) -> Option<String> {
    const PREFIX: &str = "-ares-deployment=";
    for session in sessions.values() {
        if session.product_id != "valorant" {
            continue;
        }
        for arg in &session.launch_configuration.arguments {
            if let Some(rest) = arg.strip_prefix(PREFIX) {
                let v = rest.trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
        break; // matched the valorant product; Medal stops after it
    }
    None
}

/// Decode the base64-JSON `private` presence blob.
pub fn decode_private(private_b64: &str) -> Result<PrivatePresence, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(private_b64.as_bytes())
        .map_err(|e| format!("base64 private: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("decode private json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valorant::model::LoopState;

    #[test]
    fn parses_ares_deployment_from_valorant_session() {
        let json = r#"{
            "abc": { "productId": "league_of_legends", "launchConfiguration": { "arguments": ["-foo"] } },
            "def": { "productId": "valorant", "launchConfiguration": {
                "arguments": ["--launch-product=valorant", "-ares-deployment=eu", "--other"] } }
        }"#;
        let sessions: crate::valorant::model::ExternalSessions = serde_json::from_str(json).unwrap();
        assert_eq!(parse_valorant_deployment(&sessions).as_deref(), Some("eu"));
    }

    #[test]
    fn deployment_none_when_no_valorant_or_no_flag() {
        let json = r#"{ "x": { "productId": "valorant", "launchConfiguration": { "arguments": ["--no-flag"] } } }"#;
        let s: crate::valorant::model::ExternalSessions = serde_json::from_str(json).unwrap();
        assert_eq!(parse_valorant_deployment(&s), None);
        let empty: crate::valorant::model::ExternalSessions = Default::default();
        assert_eq!(parse_valorant_deployment(&empty), None);
    }

    #[test]
    fn decodes_a_base64_private_blob() {
        let json = r#"{"sessionLoopState":"INGAME","partyOwnerMatchScoreAllyTeam":3,"partyOwnerMatchScoreEnemyTeam":2}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        let pp = decode_private(&b64).unwrap();
        assert_eq!(pp.loop_state(), LoopState::InGame);
        assert_eq!(pp.score_ally + pp.score_enemy, 5);
    }
}
