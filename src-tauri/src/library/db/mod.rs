//! SQLite clip library via rusqlite. Bundled SQLite (no system dep).
//!
//! One `clips` table: file path, title, the event tag that produced it
//! (Kill/Ace/Knife/… or "Manual" for hotkey saves), duration, dimensions, byte
//! size, optional thumbnail path, and a creation timestamp. CRUD only — file
//! deletion is the caller's job (the DB just tracks metadata).

#![allow(dead_code)]


use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// One event's position inside a clip — the label (EventKind label, e.g. "Kill")
/// and its offset in seconds from the clip's start. Drives the seek-bar markers
/// in the editor. Persisted as a JSON array in the `event_marks` column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMark {
    pub label: String,
    /// Seconds from the clip's start where the event happened.
    pub at: f64,
}

/// Shift a clip's marks for a sub-range `[start, end)` (a trim/export copy):
/// rebase each offset to the new clip's origin and keep only those that still
/// fall inside the kept window.
pub fn rebase_marks(marks: &[EventMark], start: f64, end: f64) -> Vec<EventMark> {
    marks
        .iter()
        .filter(|m| m.at >= start - 0.05 && m.at <= end + 0.05)
        .map(|m| EventMark {
            label: m.label.clone(),
            at: (m.at - start).max(0.0),
        })
        .collect()
}

/// Pull every mark earlier by `shift` seconds (clamped ≥ 0). A stream-copy trim
/// snaps the cut start *forward* to the nearest keyframe; `shift` is that snap,
/// so a marker measured from the requested start lands at `at − shift` in the
/// clip that was actually written. A no-op when `shift <= 0`.
pub fn shift_marks(marks: &[EventMark], shift: f64) -> Vec<EventMark> {
    if shift <= 0.0 {
        return marks.to_vec();
    }
    marks
        .iter()
        .map(|m| EventMark {
            label: m.label.clone(),
            at: (m.at - shift).max(0.0),
        })
        .collect()
}

/// A clip row as stored / returned. Mirrors `ClipRecord` in api.ts.
#[derive(Debug, Clone, Serialize)]
pub struct ClipRecord {
    pub id: i64,
    pub path: String,
    pub title: String,
    /// Primary event tag (EventKind label) or "Manual" — the dominant event when
    /// a clip's window merged several. Kept for back-compat + the headline badge.
    pub event: Option<String>,
    /// Every event captured in this clip's window, in time order (e.g. a window
    /// that merged a spike-defuse and a kill carries both). Falls back to the
    /// single `event` for rows written before multi-event tracking existed.
    pub events: Vec<String>,
    /// Per-event positions within the clip (label + offset seconds), for the
    /// editor's seek-bar markers. Empty for manual saves and for clips cut
    /// before event positions were persisted.
    pub event_marks: Vec<EventMark>,
    pub duration_secs: f64,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub thumb_path: Option<String>,
    /// Sprite-sheet filmstrip for the editor scrubber (one JPEG, N tiles).
    pub filmstrip_path: Option<String>,
    pub created_unix_ms: i64,

    // --- Valorant game context (all optional) -----------------------------
    // Populated for clips cut from a match: auto-clips fill everything from the
    // post-match summary; manual F9 saves fill agent/map/mode from the live
    // match (win + K/D/A are unknowable mid-match). All `None` for clips saved
    // outside a Valorant match and for clips predating this metadata.
    /// Agent display name (e.g. "Jett"), for filtering + artwork.
    pub agent: Option<String>,
    /// Agent UUID (`characterId`) — pairs with `agent` for icon lookup.
    pub agent_id: Option<String>,
    /// Map asset path (e.g. `/Game/Maps/Ascent/Ascent`); the UI prettifies it.
    pub map: Option<String>,
    /// Game-mode display name (e.g. "Competitive", "Standard", "Deathmatch").
    pub mode: Option<String>,
    /// Match result when known (auto-clips): `true` win, `false` loss.
    pub won: Option<bool>,
    /// Match K/D/A totals (auto-clips only).
    pub kills: Option<i64>,
    pub deaths: Option<i64>,
    pub assists: Option<i64>,
    /// Headshot % over recorded damage, 0–100 (auto-clips only).
    pub headshot_pct: Option<f64>,
    /// Which game this clip is from: `"valorant"` | `"lol"` (or `None` for clips
    /// predating multi-game support — treated as Valorant by the UI/backfill).
    pub game: Option<String>,

    /// True once cloud retention deleted the local files (the clip is now
    /// cloud-only and plays from its provider's presigned URL). `path` still
    /// holds the original local location for reference, but the file is gone.
    pub evicted: bool,
}

/// Fields supplied on insert (id + created timestamp are assigned by the DB).
#[derive(Debug, Clone, Default)]
pub struct NewClip {
    pub path: String,
    pub title: String,
    pub event: Option<String>,
    /// All events in the clip's window (empty ⇒ derive from `event`).
    pub events: Vec<String>,
    /// Per-event positions within the clip (empty ⇒ no markers).
    pub event_marks: Vec<EventMark>,
    pub duration_secs: f64,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub thumb_path: Option<String>,
    pub filmstrip_path: Option<String>,
    /// Valorant game context (see [`ClipRecord`]); all `None` ⇒ a clip with no
    /// match context (e.g. a manual save outside a game).
    pub agent: Option<String>,
    pub agent_id: Option<String>,
    pub map: Option<String>,
    pub mode: Option<String>,
    pub won: Option<bool>,
    pub kills: Option<i64>,
    pub deaths: Option<i64>,
    pub assists: Option<i64>,
    pub headshot_pct: Option<f64>,
    /// Source game (`"valorant"` | `"lol"`); `None` ⇒ no game context.
    pub game: Option<String>,
}

impl NewClip {
    /// A `NewClip` carrying *only* the Valorant game-context fields copied from an
    /// existing record (path/title/media fields stay `Default`). Use with struct-
    /// update syntax so a derived clip (a trim/export copy) keeps its match
    /// metadata: `NewClip { path, title, …, ..NewClip::context_from(&rec) }`.
    pub fn context_from(rec: &ClipRecord) -> NewClip {
        NewClip {
            agent: rec.agent.clone(),
            agent_id: rec.agent_id.clone(),
            map: rec.map.clone(),
            mode: rec.mode.clone(),
            won: rec.won,
            kills: rec.kills,
            deaths: rec.deaths,
            assists: rec.assists,
            headshot_pct: rec.headshot_pct,
            game: rec.game.clone(),
            ..Default::default()
        }
    }
}

/// Cloud-upload status values stored in `cloud_uploads.status`. String, not an
/// enum, to match the rest of the schema's free-text status columns and stay
mod clips;
mod cloud;
mod games;
mod retention;
mod schema;

pub use cloud::{cloud_status, CloudUpload};
pub use games::CustomGame;
pub use retention::EvictRow;

pub struct Library {
    conn: Connection,
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One-time relabel of the legacy "Standard" mode bucket → "Unrated".
///
/// Clips captured before auto-clips were labeled by live queue id stored every
/// bomb-based queue (Competitive / Unrated / Swiftplay / Premier) under the
/// generic "Standard" gameMode name. No queue id was persisted, so the buckets
/// can't be split — they're collapsed to "Unrated" (the common case).
///
/// Guarded by SQLite's `user_version` so it runs **exactly once** per database:
/// future custom-game clips, which legitimately carry "Standard" (no queue id),

#[cfg(test)]
mod tests;
