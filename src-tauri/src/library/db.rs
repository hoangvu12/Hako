//! SQLite clip library via rusqlite. Bundled SQLite (no system dep).
//!
//! One `clips` table: file path, title, the event tag that produced it
//! (Kill/Ace/Knife/… or "Manual" for hotkey saves), duration, dimensions, byte
//! size, optional thumbnail path, and a creation timestamp. CRUD only — file
//! deletion is the caller's job (the DB just tracks metadata).

#![allow(dead_code)]

use std::path::Path;

use rusqlite::{params, Connection};
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
            ..Default::default()
        }
    }
}

pub struct Library {
    conn: Connection,
}

impl Library {
    /// Open (creating if needed) the clip DB at `path`.
    pub fn open(path: &Path) -> Result<Library, String> {
        let conn = Connection::open(path).map_err(|e| format!("open db: {e}"))?;
        Self::from_conn(conn)
    }

    /// In-memory DB (tests).
    pub fn open_in_memory() -> Result<Library, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("open db: {e}"))?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Library, String> {
        // Performance pragmas: WAL lets reads (the UI clip list) proceed without
        // blocking on a write, and synchronous=NORMAL drops the per-insert fsync
        // to once-per-checkpoint (far lower insert latency, negligible risk for a
        // clip index). Best-effort: ignored on an in-memory DB / older SQLite.
        let _ = conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA temp_store=MEMORY;
             PRAGMA busy_timeout=5000;",
        );
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS clips (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                path            TEXT NOT NULL,
                title           TEXT NOT NULL,
                event           TEXT,
                duration_secs   REAL NOT NULL DEFAULT 0,
                width           INTEGER NOT NULL DEFAULT 0,
                height          INTEGER NOT NULL DEFAULT 0,
                size_bytes      INTEGER NOT NULL DEFAULT 0,
                thumb_path      TEXT,
                filmstrip_path  TEXT,
                events          TEXT,
                event_marks     TEXT,
                created_unix_ms INTEGER NOT NULL,
                agent           TEXT,
                agent_id        TEXT,
                map             TEXT,
                mode            TEXT,
                won             INTEGER,
                kills           INTEGER,
                deaths          INTEGER,
                assists         INTEGER,
                headshot_pct    REAL
            );
            CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_unix_ms DESC);",
        )
        .map_err(|e| format!("init schema: {e}"))?;
        // Migrations for DBs created before a column existed. SQLite has no "ADD
        // COLUMN IF NOT EXISTS", so we ignore the duplicate-column error.
        let _ = conn.execute("ALTER TABLE clips ADD COLUMN filmstrip_path TEXT", []);
        let _ = conn.execute("ALTER TABLE clips ADD COLUMN events TEXT", []);
        let _ = conn.execute("ALTER TABLE clips ADD COLUMN event_marks TEXT", []);
        // Valorant game-context columns (added together; all nullable).
        for col in [
            "agent TEXT",
            "agent_id TEXT",
            "map TEXT",
            "mode TEXT",
            "won INTEGER",
            "kills INTEGER",
            "deaths INTEGER",
            "assists INTEGER",
            "headshot_pct REAL",
        ] {
            let _ = conn.execute(&format!("ALTER TABLE clips ADD COLUMN {col}"), []);
        }
        Ok(Library { conn })
    }

    /// Insert a clip; returns its new id.
    pub fn insert(&self, clip: &NewClip) -> Result<i64, String> {
        let created = now_unix_ms();
        // Store the event list as a JSON array; NULL when empty so old readers
        // (and the `event` fallback) keep working.
        let events_json = (!clip.events.is_empty())
            .then(|| serde_json::to_string(&clip.events).ok())
            .flatten();
        // Event positions as a JSON array; NULL when empty so old readers ignore it.
        let marks_json = (!clip.event_marks.is_empty())
            .then(|| serde_json::to_string(&clip.event_marks).ok())
            .flatten();
        self.conn
            .execute(
                "INSERT INTO clips
                   (path, title, event, duration_secs, width, height, size_bytes, thumb_path, filmstrip_path, events, event_marks, created_unix_ms,
                    agent, agent_id, map, mode, won, kills, deaths, assists, headshot_pct)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                params![
                    clip.path,
                    clip.title,
                    clip.event,
                    clip.duration_secs,
                    clip.width,
                    clip.height,
                    clip.size_bytes,
                    clip.thumb_path,
                    clip.filmstrip_path,
                    events_json,
                    marks_json,
                    created,
                    clip.agent,
                    clip.agent_id,
                    clip.map,
                    clip.mode,
                    clip.won,
                    clip.kills,
                    clip.deaths,
                    clip.assists,
                    clip.headshot_pct,
                ],
            )
            .map_err(|e| format!("insert clip: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// All clips, newest first.
    pub fn list(&self) -> Result<Vec<ClipRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, title, event, duration_secs, width, height, size_bytes,
                        thumb_path, filmstrip_path, created_unix_ms, events,
                        agent, agent_id, map, mode, won, kills, deaths, assists, headshot_pct,
                        event_marks
                 FROM clips ORDER BY created_unix_ms DESC",
            )
            .map_err(|e| format!("prepare list: {e}"))?;
        let rows = stmt
            .query_map([], row_to_record)
            .map_err(|e| format!("query list: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("read row: {e}"))?);
        }
        Ok(out)
    }

    pub fn get(&self, id: i64) -> Result<Option<ClipRecord>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, path, title, event, duration_secs, width, height, size_bytes,
                        thumb_path, filmstrip_path, created_unix_ms, events,
                        agent, agent_id, map, mode, won, kills, deaths, assists, headshot_pct,
                        event_marks
                 FROM clips WHERE id = ?1",
            )
            .map_err(|e| format!("prepare get: {e}"))?;
        let mut rows = stmt
            .query_map(params![id], row_to_record)
            .map_err(|e| format!("query get: {e}"))?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(|e| format!("read row: {e}"))?)),
            None => Ok(None),
        }
    }

    /// Refresh the media fields after an in-place edit (e.g. a trim that
    /// overwrote the file). Leaves title/event/path untouched.
    pub fn update_media(
        &self,
        id: i64,
        duration_secs: f64,
        width: i64,
        height: i64,
        size_bytes: i64,
        thumb_path: Option<&str>,
        filmstrip_path: Option<&str>,
    ) -> Result<(), String> {
        let n = self
            .conn
            .execute(
                "UPDATE clips
                   SET duration_secs = ?1, width = ?2, height = ?3,
                       size_bytes = ?4, thumb_path = ?5, filmstrip_path = ?6
                 WHERE id = ?7",
                params![duration_secs, width, height, size_bytes, thumb_path, filmstrip_path, id],
            )
            .map_err(|e| format!("update_media: {e}"))?;
        if n == 0 {
            return Err(format!("no clip with id {id}"));
        }
        Ok(())
    }

    /// Rewrite a clip's event markers after an in-place trim/export (the offsets
    /// must be rebased to the new file's origin). NULLs the column when empty.
    pub fn update_event_marks(&self, id: i64, marks: &[EventMark]) -> Result<(), String> {
        let json = (!marks.is_empty())
            .then(|| serde_json::to_string(marks).ok())
            .flatten();
        self.conn
            .execute(
                "UPDATE clips SET event_marks = ?1 WHERE id = ?2",
                params![json, id],
            )
            .map_err(|e| format!("update_event_marks: {e}"))?;
        Ok(())
    }

    pub fn rename(&self, id: i64, title: &str) -> Result<(), String> {
        let n = self
            .conn
            .execute("UPDATE clips SET title = ?1 WHERE id = ?2", params![title, id])
            .map_err(|e| format!("rename: {e}"))?;
        if n == 0 {
            return Err(format!("no clip with id {id}"));
        }
        Ok(())
    }

    /// Remove the row (does not touch the file on disk).
    pub fn delete(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM clips WHERE id = ?1", params![id])
            .map_err(|e| format!("delete: {e}"))?;
        Ok(())
    }

    pub fn count(&self) -> Result<i64, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM clips", [], |r| r.get(0))
            .map_err(|e| format!("count: {e}"))
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<ClipRecord> {
    let event: Option<String> = row.get(3)?;
    // `events` (col 11) is a JSON array; for rows predating it (NULL/garbage),
    // fall back to the single `event` so the UI always has something to show.
    let events_json: Option<String> = row.get(11)?;
    let events = events_json
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| event.clone().into_iter().collect());
    // `event_marks` (col 21) is a JSON array; absent/garbage ⇒ no markers.
    let marks_json: Option<String> = row.get(21)?;
    let event_marks = marks_json
        .and_then(|s| serde_json::from_str::<Vec<EventMark>>(&s).ok())
        .unwrap_or_default();
    Ok(ClipRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        event,
        events,
        event_marks,
        duration_secs: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        size_bytes: row.get(7)?,
        thumb_path: row.get(8)?,
        filmstrip_path: row.get(9)?,
        created_unix_ms: row.get(10)?,
        // events is col 11 (read above); game context follows at 12..=20.
        agent: row.get(12)?,
        agent_id: row.get(13)?,
        map: row.get(14)?,
        mode: row.get(15)?,
        won: row.get(16)?,
        kills: row.get(17)?,
        deaths: row.get(18)?,
        assists: row.get(19)?,
        headshot_pct: row.get(20)?,
    })
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(path: &str, title: &str, event: Option<&str>) -> NewClip {
        NewClip {
            path: path.into(),
            title: title.into(),
            event: event.map(|s| s.into()),
            events: event.into_iter().map(|s| s.to_string()).collect(),
            duration_secs: 12.0,
            width: 2560,
            height: 1440,
            size_bytes: 1234,
            thumb_path: None,
            filmstrip_path: None,
            ..Default::default()
        }
    }

    #[test]
    fn insert_list_rename_delete() {
        let lib = Library::open_in_memory().unwrap();
        let id1 = lib.insert(&sample("a.mp4", "First", Some("Ace"))).unwrap();
        let _id2 = lib.insert(&sample("b.mp4", "Second", None)).unwrap();
        assert_eq!(lib.count().unwrap(), 2);

        let all = lib.list().unwrap();
        assert_eq!(all.len(), 2);
        // Newest first; both inserted ~same ms, so just check membership.
        assert!(all.iter().any(|c| c.title == "First" && c.event.as_deref() == Some("Ace")));

        lib.rename(id1, "Renamed").unwrap();
        assert_eq!(lib.get(id1).unwrap().unwrap().title, "Renamed");

        lib.delete(id1).unwrap();
        assert!(lib.get(id1).unwrap().is_none());
        assert_eq!(lib.count().unwrap(), 1);
    }

    #[test]
    fn multi_event_round_trips_and_single_falls_back() {
        let lib = Library::open_in_memory().unwrap();
        // A merged window carrying several events.
        let mut multi = sample("m.mp4", "Spike Defused + Kill", Some("Spike Defused"));
        multi.events = vec!["Spike Defused".into(), "Kill".into()];
        let id_multi = lib.insert(&multi).unwrap();
        let rec = lib.get(id_multi).unwrap().unwrap();
        assert_eq!(rec.events, vec!["Spike Defused", "Kill"]);
        assert_eq!(rec.event.as_deref(), Some("Spike Defused"));

        // A single-event clip persisted with an empty events list still reports
        // its one event (the `event` fallback the UI relies on).
        let mut single = sample("s.mp4", "Ace", Some("Ace"));
        single.events = Vec::new();
        let id_single = lib.insert(&single).unwrap();
        assert_eq!(lib.get(id_single).unwrap().unwrap().events, vec!["Ace"]);
    }

    #[test]
    fn game_context_round_trips_and_defaults_to_null() {
        let lib = Library::open_in_memory().unwrap();

        // A fully-enriched auto-clip.
        let mut enriched = sample("g.mp4", "Ace — Jett", Some("Ace"));
        enriched.agent = Some("Jett".into());
        enriched.agent_id = Some("add6443a-41bd-e414-f6ad-e58d267f4e95".into());
        enriched.map = Some("/Game/Maps/Ascent/Ascent".into());
        enriched.mode = Some("Competitive".into());
        enriched.won = Some(true);
        enriched.kills = Some(21);
        enriched.deaths = Some(14);
        enriched.assists = Some(5);
        enriched.headshot_pct = Some(31.5);
        let id = lib.insert(&enriched).unwrap();
        let rec = lib.get(id).unwrap().unwrap();
        assert_eq!(rec.agent.as_deref(), Some("Jett"));
        assert_eq!(rec.map.as_deref(), Some("/Game/Maps/Ascent/Ascent"));
        assert_eq!(rec.mode.as_deref(), Some("Competitive"));
        assert_eq!(rec.won, Some(true));
        assert_eq!((rec.kills, rec.deaths, rec.assists), (Some(21), Some(14), Some(5)));
        assert_eq!(rec.headshot_pct, Some(31.5));

        // A bare clip (manual save with no match context) → all game fields null.
        let bare = sample("b.mp4", "Clip", None);
        let bid = lib.insert(&bare).unwrap();
        let brec = lib.get(bid).unwrap().unwrap();
        assert_eq!(brec.agent, None);
        assert_eq!(brec.map, None);
        assert_eq!(brec.won, None);
        assert_eq!(brec.kills, None);
    }

    #[test]
    fn event_marks_round_trip_and_rebase() {
        let lib = Library::open_in_memory().unwrap();
        let mut c = sample("e.mp4", "Double Kill", Some("Double Kill"));
        c.event_marks = vec![
            EventMark { label: "Kill".into(), at: 3.0 },
            EventMark { label: "Kill".into(), at: 9.5 },
        ];
        let id = lib.insert(&c).unwrap();
        let rec = lib.get(id).unwrap().unwrap();
        assert_eq!(rec.event_marks.len(), 2);
        assert_eq!(rec.event_marks[1].at, 9.5);

        // Rebasing onto [2, 8) drops the 9.5s mark and shifts 3.0 → 1.0.
        let rebased = rebase_marks(&rec.event_marks, 2.0, 8.0);
        assert_eq!(rebased.len(), 1);
        assert_eq!(rebased[0].at, 1.0);

        // A clip with no marks round-trips to an empty list (NULL column).
        let bare = sample("b.mp4", "Clip", None);
        let bid = lib.insert(&bare).unwrap();
        assert!(lib.get(bid).unwrap().unwrap().event_marks.is_empty());
    }

    #[test]
    fn rename_missing_errors() {
        let lib = Library::open_in_memory().unwrap();
        assert!(lib.rename(999, "x").is_err());
    }
}
