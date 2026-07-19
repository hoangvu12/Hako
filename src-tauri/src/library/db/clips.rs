//! CRUD over the `clips` table -- the library's primary record.

use rusqlite::params;

use super::{now_unix_ms, ClipRecord, EventMark, Library, NewClip};

impl Library {
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
                params![
                    duration_secs,
                    width,
                    height,
                    size_bytes,
                    thumb_path,
                    filmstrip_path,
                    id
                ],
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
            .execute(
                "UPDATE clips SET title = ?1 WHERE id = ?2",
                params![title, id],
            )
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
