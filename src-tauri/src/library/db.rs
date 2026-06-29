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
/// forward-compatible if Phase 2/3 add states (e.g. `processing`).
pub mod cloud_status {
    /// Enqueued, not yet started (or reset for a retry).
    pub const QUEUED: &str = "queued";
    /// Bytes are streaming to the provider.
    pub const UPLOADING: &str = "uploading";
    /// Fully uploaded; `uploaded_at` is set → safe to evict locally.
    pub const DONE: &str = "done";
    /// Terminal failure after the RetryLayer's internal retries; `error` holds a
    /// friendly message. User can re-trigger (re-`cloud_enqueue`).
    pub const ERROR: &str = "error";
    /// User-canceled mid-flight.
    pub const CANCELED: &str = "canceled";
}

/// One `cloud_uploads` row as stored / returned. Mirrors `CloudUpload` in
/// api.ts; serde serializes with these exact field names. Keyed by
/// `(clip_id, provider_id)` — a clip may be backed up to several providers.
#[derive(Debug, Clone, Serialize)]
pub struct CloudUpload {
    pub clip_id: i64,
    /// The configured provider this row tracks (see `cloud::ProviderConfig::id`).
    pub provider_id: String,
    /// Key/path written in the bucket (set once the upload starts).
    pub remote_path: Option<String>,
    /// Presigned (or public) read URL, when the provider supports it.
    pub remote_url: Option<String>,
    /// One of [`cloud_status`].
    pub status: String,
    pub bytes_sent: i64,
    pub size_bytes: i64,
    /// Unix ms; set ONLY on success — the local-eviction gate.
    pub uploaded_at: Option<i64>,
    pub error: Option<String>,
    pub updated_at: i64,
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
             PRAGMA busy_timeout=5000;
             -- Enforce ON DELETE CASCADE (off by default per-connection in SQLite)
             -- so deleting a clip also clears its `cloud_uploads` rows.
             PRAGMA foreign_keys=ON;",
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
                headshot_pct    REAL,
                -- Source game (valorant | lol); NULL on pre-multi-game rows.
                game            TEXT,
                -- Local files deleted by cloud retention (free up space); the row
                -- stays as a cloud-only clip, played from its presigned URL.
                evicted         INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_unix_ms DESC);",
        )
        .map_err(|e| format!("init schema: {e}"))?;
        // Cloud-upload state, keyed by (clip_id, provider_id). Additive: a clip
        // can be uploaded to several configured providers, each tracked as its own
        // row. `uploaded_at IS NOT NULL` is the "safe to evict locally" gate the
        // retention worker keys off (Medal's `contentUploadedAt`). The `clips` row
        // owns the lifecycle — deleting a clip cascades its cloud rows away.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cloud_uploads (
                clip_id      INTEGER NOT NULL,
                provider_id  TEXT    NOT NULL,
                remote_path  TEXT,
                remote_url   TEXT,
                status       TEXT    NOT NULL,
                bytes_sent   INTEGER NOT NULL DEFAULT 0,
                size_bytes   INTEGER NOT NULL DEFAULT 0,
                uploaded_at  INTEGER,
                error        TEXT,
                updated_at   INTEGER NOT NULL,
                PRIMARY KEY (clip_id, provider_id),
                FOREIGN KEY (clip_id) REFERENCES clips(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_cloud_uploads_status ON cloud_uploads(status);
            CREATE INDEX IF NOT EXISTS idx_cloud_uploads_uploaded ON cloud_uploads(uploaded_at);",
        )
        .map_err(|e| format!("init cloud schema: {e}"))?;
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
        // Cloud-retention eviction flag (DBs created before "free up space").
        let _ = conn.execute(
            "ALTER TABLE clips ADD COLUMN evicted INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // Multi-game: source game column (DBs created before multi-game support).
        let _ = conn.execute("ALTER TABLE clips ADD COLUMN game TEXT", []);
        // One-time data migrations (best-effort; never block open).
        let _ = relabel_legacy_standard(&conn);
        let _ = backfill_game_valorant(&conn);
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
                    agent, agent_id, map, mode, won, kills, deaths, assists, headshot_pct, game)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
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
                    clip.game,
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
                        event_marks, evicted, game
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
                        event_marks, evicted, game
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

    /// Repoint a clip's on-disk paths after its files are relocated (e.g. the
    /// user changed the clip folder and existing clips were moved). Only rewrites
    /// the location columns; media metadata and title are untouched.
    pub fn update_paths(
        &self,
        id: i64,
        path: &str,
        thumb_path: Option<&str>,
        filmstrip_path: Option<&str>,
    ) -> Result<(), String> {
        let n = self
            .conn
            .execute(
                "UPDATE clips SET path = ?1, thumb_path = ?2, filmstrip_path = ?3 WHERE id = ?4",
                params![path, thumb_path, filmstrip_path, id],
            )
            .map_err(|e| format!("update_paths: {e}"))?;
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

    // --- cloud_uploads ----------------------------------------------------
    // State for the cloud-upload engine (src-tauri/src/cloud). The library owns
    // this table because eviction (Medal's "free up space") joins it against
    // `clips`; keeping both behind the one `Mutex<Library>` avoids a second lock.

    /// Insert or reset the row for `(clip_id, provider_id)` to `queued`, clearing
    /// any prior progress/error/`uploaded_at` so a re-upload after a failure (or
    /// a fresh enqueue) starts clean. `INSERT … ON CONFLICT … DO UPDATE` keeps the
    /// primary key stable so re-queuing overwrites in place.
    pub fn cloud_enqueue(
        &self,
        clip_id: i64,
        provider_id: &str,
        size_bytes: i64,
    ) -> Result<(), String> {
        let now = now_unix_ms();
        self.conn
            .execute(
                "INSERT INTO cloud_uploads
                   (clip_id, provider_id, status, bytes_sent, size_bytes, updated_at,
                    remote_path, remote_url, uploaded_at, error)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, NULL, NULL, NULL, NULL)
                 ON CONFLICT(clip_id, provider_id) DO UPDATE SET
                    status = excluded.status,
                    bytes_sent = 0,
                    size_bytes = excluded.size_bytes,
                    updated_at = excluded.updated_at,
                    remote_path = NULL, remote_url = NULL, uploaded_at = NULL, error = NULL",
                params![clip_id, provider_id, cloud_status::QUEUED, size_bytes, now],
            )
            .map_err(|e| format!("cloud_enqueue: {e}"))?;
        Ok(())
    }

    /// Flip a row to `uploading` and record the remote key it's streaming to.
    /// Called once, right before the byte stream starts.
    pub fn cloud_mark_uploading(
        &self,
        clip_id: i64,
        provider_id: &str,
        remote_path: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE cloud_uploads
                    SET status = ?1, remote_path = ?2, error = NULL, updated_at = ?3
                  WHERE clip_id = ?4 AND provider_id = ?5",
                params![cloud_status::UPLOADING, remote_path, now_unix_ms(), clip_id, provider_id],
            )
            .map_err(|e| format!("cloud_mark_uploading: {e}"))?;
        Ok(())
    }

    /// Update the streamed-byte counter for the live progress UI. Cheap and
    /// frequent — callers throttle it (see the upload engine).
    pub fn cloud_set_progress(
        &self,
        clip_id: i64,
        provider_id: &str,
        bytes_sent: i64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE cloud_uploads SET bytes_sent = ?1, updated_at = ?2
                  WHERE clip_id = ?3 AND provider_id = ?4",
                params![bytes_sent, now_unix_ms(), clip_id, provider_id],
            )
            .map_err(|e| format!("cloud_set_progress: {e}"))?;
        Ok(())
    }

    /// Mark a row `done`: set `uploaded_at` (the eviction gate), the optional
    /// presigned `remote_url`, and snap `bytes_sent` to `size_bytes`.
    pub fn cloud_mark_done(
        &self,
        clip_id: i64,
        provider_id: &str,
        remote_url: Option<&str>,
    ) -> Result<(), String> {
        let now = now_unix_ms();
        self.conn
            .execute(
                "UPDATE cloud_uploads
                    SET status = ?1, uploaded_at = ?2, remote_url = ?3, error = NULL,
                        bytes_sent = size_bytes, updated_at = ?2
                  WHERE clip_id = ?4 AND provider_id = ?5",
                params![cloud_status::DONE, now, remote_url, clip_id, provider_id],
            )
            .map_err(|e| format!("cloud_mark_done: {e}"))?;
        Ok(())
    }

    /// Record a terminal `error`/`canceled` outcome with a friendly message.
    pub fn cloud_mark_failed(
        &self,
        clip_id: i64,
        provider_id: &str,
        status: &str,
        error: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE cloud_uploads SET status = ?1, error = ?2, updated_at = ?3
                  WHERE clip_id = ?4 AND provider_id = ?5",
                params![status, error, now_unix_ms(), clip_id, provider_id],
            )
            .map_err(|e| format!("cloud_mark_failed: {e}"))?;
        Ok(())
    }

    /// Reconcile non-terminal upload rows on startup. The upload queue + worker
    /// live only in memory, so any row still `queued`/`uploading` from a previous
    /// run is a zombie: nothing is actually streaming it, yet it would show as an
    /// active upload forever (and couldn't be canceled, since the worker has no
    /// such job). Flip them to `error` so the UI clears and the user can retry.
    /// Returns how many rows were reset.
    pub fn cloud_reset_interrupted(&self) -> Result<usize, String> {
        self.conn
            .execute(
                "UPDATE cloud_uploads
                    SET status = ?1, error = ?2, updated_at = ?3
                  WHERE status IN (?4, ?5)",
                params![
                    cloud_status::ERROR,
                    "interrupted (app restarted)",
                    now_unix_ms(),
                    cloud_status::QUEUED,
                    cloud_status::UPLOADING,
                ],
            )
            .map_err(|e| format!("cloud_reset_interrupted: {e}"))
    }

    /// Cloud-upload rows for one clip, or all rows when `clip_id` is `None`
    /// (newest-touched first). Powers the per-clip badge and the upload toast.
    pub fn cloud_status(&self, clip_id: Option<i64>) -> Result<Vec<CloudUpload>, String> {
        let sql = "SELECT clip_id, provider_id, remote_path, remote_url, status,
                          bytes_sent, size_bytes, uploaded_at, error, updated_at
                     FROM cloud_uploads";
        let mut out = Vec::new();
        match clip_id {
            Some(id) => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("{sql} WHERE clip_id = ?1 ORDER BY updated_at DESC"))
                    .map_err(|e| format!("prepare cloud_status: {e}"))?;
                let rows = stmt
                    .query_map(params![id], row_to_cloud_upload)
                    .map_err(|e| format!("query cloud_status: {e}"))?;
                for r in rows {
                    out.push(r.map_err(|e| format!("read cloud row: {e}"))?);
                }
            }
            None => {
                let mut stmt = self
                    .conn
                    .prepare(&format!("{sql} ORDER BY updated_at DESC"))
                    .map_err(|e| format!("prepare cloud_status: {e}"))?;
                let rows = stmt
                    .query_map([], row_to_cloud_upload)
                    .map_err(|e| format!("query cloud_status: {e}"))?;
                for r in rows {
                    out.push(r.map_err(|e| format!("read cloud row: {e}"))?);
                }
            }
        }
        Ok(out)
    }

    // --- cloud retention ("free up space") ---------------------------------

    /// Total bytes + count of clips whose local files are still on disk (not yet
    /// evicted). Drives the retention gauge and the under-budget early-out.
    pub fn local_footprint(&self) -> Result<(i64, i64), String> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0), COUNT(*)
                   FROM clips WHERE evicted = 0",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("local_footprint: {e}"))
    }

    /// Eviction candidates, oldest first: clips with local files still present
    /// that are fully and safely uploaded to at least one provider. The newest
    /// stay on disk longest (we evict from the front until under budget).
    ///
    /// `provider_ids` lists the providers each clip has a *completed* upload to
    /// (comma-free ids, so a `GROUP_CONCAT` split is safe). Cloud retention uses
    /// it to skip clips whose only cloud copies can't presign — those can't
    /// stream-play once evicted, so they must keep their local file.
    pub fn evictable_clips(&self) -> Result<Vec<EvictRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.path, c.size_bytes,
                        GROUP_CONCAT(u.provider_id)
                   FROM clips c
                   JOIN cloud_uploads u ON u.clip_id = c.id
                  WHERE c.evicted = 0
                    AND u.status = 'done' AND u.uploaded_at IS NOT NULL
                  GROUP BY c.id
                  ORDER BY c.created_unix_ms ASC",
            )
            .map_err(|e| format!("prepare evictable: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                let provider_ids: Option<String> = r.get(3)?;
                let provider_ids = provider_ids
                    .unwrap_or_default()
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                Ok(EvictRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    size_bytes: r.get(2)?,
                    provider_ids,
                })
            })
            .map_err(|e| format!("query evictable: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("read evict row: {e}"))?);
        }
        Ok(out)
    }

    /// Flag a clip as evicted. The thumbnail/filmstrip paths are deliberately
    /// kept: retention deletes only the (large) video file, leaving the tiny
    /// poster/filmstrip on disk so a cloud-only clip still shows its real
    /// thumbnail in the library instead of a blank placeholder. `path` is kept as
    /// a record of where the video used to live (and where a re-download lands).
    pub fn mark_evicted(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE clips SET evicted = 1 WHERE id = ?1",
                params![id],
            )
            .map_err(|e| format!("mark_evicted: {e}"))?;
        Ok(())
    }

    /// Reverse of [`mark_evicted`]: the clip's local file has been re-downloaded
    /// from the cloud, so clear the `evicted` flag and restore the freshly
    /// regenerated thumbnail/filmstrip paths. `path` is unchanged (the download
    /// lands back at the row's recorded location).
    pub fn mark_rehydrated(
        &self,
        id: i64,
        thumb_path: Option<&str>,
        filmstrip_path: Option<&str>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE clips SET evicted = 0, thumb_path = ?2, filmstrip_path = ?3
                  WHERE id = ?1",
                params![id, thumb_path, filmstrip_path],
            )
            .map_err(|e| format!("mark_rehydrated: {e}"))?;
        Ok(())
    }
}

/// A clip eligible for local eviction (see [`Library::evictable_clips`]).
#[derive(Debug, Clone)]
pub struct EvictRow {
    pub id: i64,
    pub path: String,
    pub size_bytes: i64,
    /// Providers this clip has a completed upload to. Retention only evicts a
    /// clip when at least one is presign-capable (see `cloud::retention`).
    pub provider_ids: Vec<String>,
}

fn row_to_cloud_upload(row: &rusqlite::Row) -> rusqlite::Result<CloudUpload> {
    Ok(CloudUpload {
        clip_id: row.get(0)?,
        provider_id: row.get(1)?,
        remote_path: row.get(2)?,
        remote_url: row.get(3)?,
        status: row.get(4)?,
        bytes_sent: row.get(5)?,
        size_bytes: row.get(6)?,
        uploaded_at: row.get(7)?,
        error: row.get(8)?,
        updated_at: row.get(9)?,
    })
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
    // `evicted` (col 22) — stored as 0/1; absent on very old rows ⇒ false.
    let evicted: i64 = row.get(22).unwrap_or(0);
    // `game` (col 23) — NULL on pre-multi-game rows (the backfill labels existing
    // rows "valorant"; a NULL here just means "unknown", handled by the UI).
    let game: Option<String> = row.get(23).unwrap_or(None);
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
        game,
        evicted: evicted != 0,
    })
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
/// are left untouched. Idempotent — a second call is a no-op.
fn relabel_legacy_standard(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if version >= 1 {
        return Ok(());
    }
    conn.execute(
        "UPDATE clips SET mode = 'Unrated' WHERE mode = 'Standard'",
        [],
    )
    .map_err(|e| format!("relabel legacy standard: {e}"))?;
    conn.execute_batch("PRAGMA user_version = 1;")
        .map_err(|e| format!("bump user_version: {e}"))
}

/// One-time backfill of the `game` column → `"valorant"` for every existing clip
/// (the only game before multi-game support). Guarded by `user_version >= 2` so
/// it runs exactly once; later clips carry their own `game` and are untouched.
fn backfill_game_valorant(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if version >= 2 {
        return Ok(());
    }
    conn.execute("UPDATE clips SET game = 'valorant' WHERE game IS NULL", [])
        .map_err(|e| format!("backfill game: {e}"))?;
    conn.execute_batch("PRAGMA user_version = 2;")
        .map_err(|e| format!("bump user_version: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relabels_legacy_standard_once() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE clips (id INTEGER PRIMARY KEY, mode TEXT);
             INSERT INTO clips (mode) VALUES ('Standard'), ('Competitive'), ('Standard'), (NULL);",
        )
        .unwrap();

        // First pass (user_version 0): the two legacy "Standard" rows → "Unrated".
        relabel_legacy_standard(&conn).unwrap();
        let count = |m: &str| -> i64 {
            conn.query_row("SELECT COUNT(*) FROM clips WHERE mode = ?1", [m], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count("Unrated"), 2);
        assert_eq!(count("Standard"), 0);
        assert_eq!(count("Competitive"), 1); // other modes untouched

        // A later custom-game "Standard" clip is preserved — the guard makes the
        // second pass a no-op (it has no queue id and is legitimately "Standard").
        conn.execute("INSERT INTO clips (mode) VALUES ('Standard')", [])
            .unwrap();
        relabel_legacy_standard(&conn).unwrap();
        assert_eq!(count("Standard"), 1);
        assert_eq!(count("Unrated"), 2);
    }

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

    #[test]
    fn cloud_upload_lifecycle_and_cascade() {
        let lib = Library::open_in_memory().unwrap();
        let id = lib.insert(&sample("c.mp4", "Clip", None)).unwrap();

        // Enqueue → uploading → progress → done.
        lib.cloud_enqueue(id, "r2-main", 1000).unwrap();
        let q = &lib.cloud_status(Some(id)).unwrap()[0];
        assert_eq!(q.status, cloud_status::QUEUED);
        assert_eq!((q.size_bytes, q.bytes_sent, q.uploaded_at), (1000, 0, None));

        lib.cloud_mark_uploading(id, "r2-main", "hako/2026/06/c.mp4").unwrap();
        lib.cloud_set_progress(id, "r2-main", 512).unwrap();
        lib.cloud_mark_done(id, "r2-main", Some("https://signed/url")).unwrap();
        let d = &lib.cloud_status(Some(id)).unwrap()[0];
        assert_eq!(d.status, cloud_status::DONE);
        assert_eq!(d.bytes_sent, d.size_bytes); // snapped to total on success
        assert!(d.uploaded_at.is_some()); // eviction gate set
        assert_eq!(d.remote_url.as_deref(), Some("https://signed/url"));
        assert_eq!(d.remote_path.as_deref(), Some("hako/2026/06/c.mp4"));

        // Re-enqueue resets progress/error/uploaded_at in place (a retry).
        lib.cloud_mark_failed(id, "r2-main", cloud_status::ERROR, "boom").unwrap();
        lib.cloud_enqueue(id, "r2-main", 1000).unwrap();
        let r = &lib.cloud_status(Some(id)).unwrap()[0];
        assert_eq!(r.status, cloud_status::QUEUED);
        assert_eq!(r.error, None);
        assert_eq!(r.uploaded_at, None);
        assert_eq!(lib.cloud_status(Some(id)).unwrap().len(), 1); // overwrote, no dup

        // Deleting the clip cascades the cloud row away (PRAGMA foreign_keys=ON).
        lib.delete(id).unwrap();
        assert!(lib.cloud_status(Some(id)).unwrap().is_empty());
    }

    #[test]
    fn retention_only_evicts_uploaded_clips() {
        let lib = Library::open_in_memory().unwrap();
        // Two clips, 1000 bytes each (see `sample`'s size_bytes = 1234 actually).
        let mut uploaded_clip = sample("done.mp4", "Uploaded", None);
        uploaded_clip.thumb_path = Some("done.jpg".into());
        uploaded_clip.filmstrip_path = Some("done_strip.jpg".into());
        let uploaded = lib.insert(&uploaded_clip).unwrap();
        let local_only = lib.insert(&sample("local.mp4", "Local", None)).unwrap();

        // Only the first is safely in the cloud → only it is an eviction candidate.
        lib.cloud_enqueue(uploaded, "r2-main", 1234).unwrap();
        lib.cloud_mark_done(uploaded, "r2-main", Some("https://signed/url"))
            .unwrap();

        let (bytes_before, count_before) = lib.local_footprint().unwrap();
        assert_eq!(count_before, 2);
        assert_eq!(bytes_before, 1234 * 2);

        let candidates = lib.evictable_clips().unwrap();
        assert_eq!(candidates.len(), 1, "only the uploaded clip is evictable");
        assert_eq!(candidates[0].id, uploaded);
        // The completed-upload provider is surfaced for the presign gate.
        assert_eq!(candidates[0].provider_ids, vec!["r2-main".to_string()]);

        // Evicting flips the flag and drops it from the footprint, but KEEPS the
        // thumbnail/filmstrip (only the video is deleted) so the cloud-only clip
        // still shows a real poster. The row and its `path` survive.
        lib.mark_evicted(uploaded).unwrap();
        let rec = lib.get(uploaded).unwrap().unwrap();
        assert!(rec.evicted);
        assert_eq!(rec.thumb_path.as_deref(), Some("done.jpg"));
        assert_eq!(rec.filmstrip_path.as_deref(), Some("done_strip.jpg"));
        assert_eq!(rec.path, "done.mp4");

        let (bytes_after, count_after) = lib.local_footprint().unwrap();
        assert_eq!((bytes_after, count_after), (1234, 1));
        // Already-evicted clips are no longer candidates.
        assert!(lib.evictable_clips().unwrap().is_empty());
        // The non-uploaded clip is untouched.
        assert!(!lib.get(local_only).unwrap().unwrap().evicted);
    }
}
