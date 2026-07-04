//! The curated game list — the "known non-Steam games" source.
//!
//! Steam's on-disk catalog covers ~49k titles ([`super::steam`]), but games from
//! Epic / Riot / Battle.net / miHoYo / Rockstar / EA and other non-Steam launchers
//! won't have a `steamapps\common\` path to resolve. This is an `exe → display
//! name` table for the popular ones, seeded from public knowledge — the Phase-3
//! equivalent of Medal's downloadable signature rows, authored ourselves (we never
//! ship Medal's DB — plan §1.6). Only distinctive exe names are listed (never
//! generic engine exes like `javaw.exe` or `Client-Win64-Shipping.exe`, which
//! would false-match). Smart-game exes are intentionally absent — those own the
//! arbiter — and [`super::catalog::is_excluded`] is a second guard on top.
//!
//! The table has two layers:
//! - **Bundled** (`assets/games.json`, compiled in via `include_str!`) — always
//!   present, works offline on day one.
//! - **Remote** — the *same file* served from the repo via GitHub's raw endpoint,
//!   fetched best-effort on startup so the list can be updated between releases by
//!   just committing to `games.json` (no app update). It only ever *adds/overrides*
//!   entries on top of the bundled baseline (union merge) — a missing, private,
//!   offline, or malformed remote silently leaves the bundled list intact.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use serde::Deserialize;
use tauri::{AppHandle, Manager};

/// The bundled curated list, embedded at build time. Validated by the unit test
/// below so a malformed edit fails CI, not a user's runtime.
const GAMES_JSON: &str = include_str!("../../../assets/games.json");

/// The same `games.json`, served from the repo's default branch via GitHub raw —
/// fetched on startup so the list updates by a commit, no release required. If the
/// repo is private / offline / the file moves, the fetch just fails and the
/// bundled list stands (see [`spawn_refresh`]).
const REMOTE_URL: &str =
    "https://raw.githubusercontent.com/hoangvu12/hako/main/src-tauri/assets/games.json";
/// Cap on the fetched body so a bad/huge response can't blow up memory.
const REMOTE_MAX_BYTES: usize = 512 * 1024;
/// Last successfully-fetched list, cached in the app config dir (applied on the
/// next launch before/without the network).
const CACHE_FILE: &str = "games-remote.json";
/// The cached response's ETag, sent as `If-None-Match` so repeat fetches are a
/// cheap 304.
const ETAG_FILE: &str = "games-remote.etag";

#[derive(Deserialize)]
struct CuratedEntry {
    name: String,
    exes: Vec<String>,
}

/// Lowercase exe name → display name. A `RwLock` (not a bare `OnceLock` map) so the
/// remote refresh can extend it at runtime; reads (once per detection tick) take
/// the cheap read lock. Initialized lazily from the bundled list.
fn table() -> &'static RwLock<HashMap<String, String>> {
    static TABLE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(entries_to_map(parse_bundled())))
}

/// The curated display name for a **lowercase** exe name, or `None` if unlisted.
/// Callers pass the already-lowercased process name from the shared snapshot.
pub fn curated_name(process_name: &str) -> Option<String> {
    table().read().ok()?.get(process_name).cloned()
}

/// Union `entries` into the live table: remote may add or override by exe key but
/// never removes a bundled entry, so a bad/empty remote can only ever add.
fn merge_entries(entries: Vec<CuratedEntry>) {
    if entries.is_empty() {
        return;
    }
    if let Ok(mut map) = table().write() {
        for entry in entries {
            for exe in entry.exes {
                map.insert(exe.to_ascii_lowercase(), entry.name.clone());
            }
        }
    }
}

fn entries_to_map(entries: Vec<CuratedEntry>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for entry in entries {
        for exe in entry.exes {
            map.insert(exe.to_ascii_lowercase(), entry.name.clone());
        }
    }
    map
}

fn parse_bundled() -> Vec<CuratedEntry> {
    parse_entries(GAMES_JSON).expect("bundled games.json is valid JSON")
}

/// Parse a `games.json` body (bundled or remote). Lenient for the remote path:
/// `None` on any malformed input so the caller keeps the bundled list.
fn parse_entries(json: &str) -> Option<Vec<CuratedEntry>> {
    serde_json::from_str::<Vec<CuratedEntry>>(json).ok()
}

/// Best-effort remote refresh, spawned once at startup. Applies the last cached
/// list immediately (offline-friendly), then fetches a fresh copy conditionally on
/// the stored ETag. Every failure path is silent — the bundled list is the floor.
pub fn spawn_refresh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 1. Re-apply the last-known cached list right away (survives being offline).
        if let Some(dir) = cache_dir(&app) {
            if let Ok(text) = std::fs::read_to_string(dir.join(CACHE_FILE)) {
                if let Some(entries) = parse_entries(&text) {
                    merge_entries(entries);
                }
            }
        }
        // 2. Fetch fresh (conditional GET). A 304 / error just leaves the list as-is.
        if let Err(e) = fetch_and_apply(&app).await {
            tracing::debug!("curated: remote list refresh skipped: {e}");
        }
    });
}

async fn fetch_and_apply(app: &AppHandle) -> Result<(), String> {
    let dir = cache_dir(app).ok_or("no config dir")?;
    let etag = std::fs::read_to_string(dir.join(ETAG_FILE)).ok();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(REMOTE_URL);
    if let Some(tag) = etag.as_deref() {
        req = req.header(reqwest::header::IF_NONE_MATCH, tag.trim());
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;

    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(()); // unchanged — the cached list (applied above) is current
    }
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }
    let new_etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > REMOTE_MAX_BYTES {
        return Err(format!("remote list too large ({} bytes)", bytes.len()));
    }
    let text = String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?;
    let entries = parse_entries(&text).ok_or("malformed remote list")?;

    // Persist for the next launch, then merge into the live table.
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(CACHE_FILE), &text);
    if let Some(tag) = new_etag {
        let _ = std::fs::write(dir.join(ETAG_FILE), tag);
    }
    let n = entries.len();
    merge_entries(entries);
    tracing::info!("curated: applied {n} entries from the remote list");
    Ok(())
}

fn cache_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_list_parses_and_resolves() {
        // Parses + is non-trivial.
        assert!(table().read().unwrap().len() >= 20, "curated table looks too small");
        // Keyed by lowercase exe; the JSON stores mixed case.
        assert_eq!(
            curated_name("fortniteclient-win64-shipping.exe").as_deref(),
            Some("Fortnite")
        );
        assert_eq!(curated_name("gta5.exe").as_deref(), Some("Grand Theft Auto V"));
        // Non-listed / blacklisted exes don't resolve.
        assert_eq!(curated_name("chrome.exe"), None);
    }

    #[test]
    fn remote_entries_merge_without_dropping_bundled() {
        // A remote entry is added...
        merge_entries(vec![CuratedEntry {
            name: "Test Remote Game".into(),
            exes: vec!["testremotegame.exe".into()],
        }]);
        assert_eq!(
            curated_name("testremotegame.exe").as_deref(),
            Some("Test Remote Game")
        );
        // ...without removing bundled entries (union, never replace).
        assert!(curated_name("fortniteclient-win64-shipping.exe").is_some());
    }
}
