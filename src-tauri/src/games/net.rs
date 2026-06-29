//! Shared HTTP client builders for the local game APIs.
//!
//! Every Riot local endpoint we talk to — Valorant's local API, League's LCU, and
//! League's Live Client Data server on `:2999` — serves HTTPS with a self-signed
//! certificate and only accepts localhost connections. They are all reached with
//! `danger_accept_invalid_certs` (safe: the connection never leaves the machine).

#![allow(dead_code)]

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

/// Default per-request timeout for local game APIs. They answer in well under a
/// second when up; this just bounds a hung socket so a poll tick never stalls.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(5);

/// A bare client for a self-signed localhost endpoint with no auth (League's
/// Live Client Data API on `https://127.0.0.1:2999`).
pub fn insecure_localhost_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(LOCAL_TIMEOUT)
        .build()
        .map_err(|e| format!("build localhost http client: {e}"))
}

/// A self-signed localhost client preloaded with a Basic `Authorization` header
/// (Valorant local API / League LCU, authed from the lockfile password).
pub fn insecure_localhost_client_with_auth(auth_header: &str) -> Result<reqwest::Client, String> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(auth_header).map_err(|e| e.to_string())?,
    );
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(LOCAL_TIMEOUT)
        .default_headers(headers)
        .build()
        .map_err(|e| format!("build authed localhost http client: {e}"))
}
