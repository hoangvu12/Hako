//! Durable "pending match" store — the retry-later half of the post-match cut.
//!
//! When a match ends but `match-details` can't be fetched (the player was kicked
//! / disconnected and their RSO token is dead, the match isn't finalized yet, or
//! Riot is briefly down), the live cut pipeline can't derive highlights. Rather
//! than discard the footage we already recorded, we persist the finished session
//! MP4 plus everything the cut needs ([`PendingMatch`]) into the app-data dir and
//! retry from [`crate::valorant::cut::reconcile_pending`] whenever the local Riot
//! API is reachable again (the match-details endpoint is the durable match-history
//! endpoint, so it stays fetchable for a long time once finalized).
//!
//! Pending entries are **Highlights-mode only** — FullMatch / Session save the
//! whole recording immediately (they don't need match-details), so they never
//! pend. Each entry is `{stem}.json` (this struct) beside `{stem}.mp4` (the
//! session footage), so an app restart picks up where it left off.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::valorant::reconcile::{RoundAnchor, TimelineIndex};

/// Subdirectory of the app-data dir holding pending matches.
const DIR: &str = "pending_matches";

/// Everything the post-match cut needs to run later, minus the remote client +
/// match-details (rebuilt from a fresh local-API connection at reconcile time).
/// Serialized as the `{stem}.json` sidecar next to the session `{stem}.mp4`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMatch {
    /// Session MP4 file name within the pending dir (footage to cut from).
    pub session_file: String,
    /// Wall-clock ↔ session-PTS map built while recording.
    pub timeline: TimelineIndex,
    /// Session-PTS spans recorded while capture was frozen (skip dead clips).
    pub frozen_spans: Vec<(i64, i64)>,
    /// Round-start anchors from the log tail.
    pub anchors: Vec<RoundAnchor>,
    pub fps: u32,
    /// Fallback anchor (match-found wall-clock) when no round anchor matches.
    pub game_start_ticks: i64,
    /// Our identity (for event derivation + summary).
    pub puuid: String,
    /// The match to fetch details for.
    pub match_id: String,
    /// glz affinity/region + pvp.net shard + client version, so a fresh
    /// [`crate::valorant::remote_api::RemoteClient`] can be rebuilt with new
    /// tokens at reconcile time.
    pub region: String,
    pub shard: String,
    pub client_version: String,
    /// Unix-ms when this entry was first written (drives the age cap).
    pub created_unix_ms: u128,
    /// How many reconcile attempts have failed so far (logging / cap).
    pub attempts: u32,
}

/// The pending dir (created if missing). `None` if the app-data dir can't be
/// resolved or created — the caller then degrades to the whole-session fallback.
pub fn dir(app: &AppHandle) -> Option<PathBuf> {
    let base = app.path().app_data_dir().ok()?.join(DIR);
    std::fs::create_dir_all(&base).ok()?;
    Some(base)
}

/// Absolute path of an entry's session MP4.
pub fn session_path(app: &AppHandle, entry: &PendingMatch) -> Option<PathBuf> {
    Some(dir(app)?.join(&entry.session_file))
}

/// Persist a pending match: copy `session_src` into the pending dir and write the
/// JSON sidecar. The caller's temp `session_src` is left for its own cleanup.
/// `stem` must be unique (match id + timestamp) so two recordings of the same
/// match (e.g. a mid-match crash + reconnect) don't collide. Returns the written
/// sidecar path.
pub fn save(
    app: &AppHandle,
    stem: &str,
    session_src: &Path,
    mut entry: PendingMatch,
) -> Result<PathBuf, String> {
    let dir = dir(app).ok_or("pending: app-data dir unavailable")?;
    let session_file = format!("{stem}.mp4");
    let session_dst = dir.join(&session_file);
    copy_or_move(session_src, &session_dst)
        .map_err(|e| format!("pending: copy session footage: {e}"))?;
    entry.session_file = session_file;

    let json = dir.join(format!("{stem}.json"));
    let body = serde_json::to_vec_pretty(&entry).map_err(|e| format!("pending: encode: {e}"))?;
    if let Err(e) = std::fs::write(&json, body) {
        // Don't leave an orphan MP4 with no sidecar to drive its cut.
        let _ = std::fs::remove_file(&session_dst);
        return Err(format!("pending: write sidecar: {e}"));
    }
    Ok(json)
}

/// Every pending entry on disk: `(sidecar_path, decoded)`. Skips unreadable /
/// malformed sidecars (logged) so one bad file can't stall the queue.
pub fn list(app: &AppHandle) -> Vec<(PathBuf, PendingMatch)> {
    let Some(dir) = dir(app) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ent in rd.flatten() {
        let path = ent.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| {
            serde_json::from_slice::<PendingMatch>(&b).map_err(|e| e.to_string())
        }) {
            Ok(m) => out.push((path, m)),
            Err(e) => tracing::warn!("pending: skipping unreadable {}: {e}", path.display()),
        }
    }
    out
}

/// True if there's at least one pending entry — a cheap gate so the orchestrator
/// only spawns a reconcile task when there's work.
pub fn any(app: &AppHandle) -> bool {
    dir(app)
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            rd.flatten()
                .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        })
        .unwrap_or(false)
}

/// Delete an entry (sidecar + its session MP4). Best-effort.
pub fn remove(app: &AppHandle, sidecar: &Path, entry: &PendingMatch) {
    if let Some(mp4) = session_path(app, entry) {
        if let Err(e) = std::fs::remove_file(&mp4) {
            tracing::debug!("pending: remove footage {}: {e}", mp4.display());
        }
    }
    if let Err(e) = std::fs::remove_file(sidecar) {
        tracing::debug!("pending: remove sidecar {}: {e}", sidecar.display());
    }
}

/// Rewrite a sidecar in place after a failed attempt (to bump `attempts`).
pub fn update(sidecar: &Path, entry: &PendingMatch) {
    match serde_json::to_vec_pretty(entry) {
        Ok(b) => {
            if let Err(e) = std::fs::write(sidecar, b) {
                tracing::debug!("pending: update sidecar {}: {e}", sidecar.display());
            }
        }
        Err(e) => tracing::debug!("pending: encode update: {e}"),
    }
}

/// Current Unix time in milliseconds (entry timestamps + the age cap).
pub fn now_unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Move `src` to `dst`, falling back to copy when `rename` fails (the temp dir
/// and app-data dir can live on different volumes, where `rename` is `EXDEV`).
fn copy_or_move(src: &Path, dst: &Path) -> std::io::Result<()> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    std::fs::copy(src, dst)?;
    Ok(())
}
