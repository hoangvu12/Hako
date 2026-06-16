//! SQLite clip library via rusqlite. Bundled SQLite (no system dep).
//!
//! One `clips` table: file path, title, the event tag that produced it
//! (Kill/Ace/Knife/… or "Manual" for hotkey saves), duration, dimensions, byte
//! size, optional thumbnail path, and a creation timestamp. CRUD only — file
//! deletion is the caller's job (the DB just tracks metadata).

#![allow(dead_code)]

use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;

/// A clip row as stored / returned. Mirrors `ClipRecord` in api.ts.
#[derive(Debug, Clone, Serialize)]
pub struct ClipRecord {
    pub id: i64,
    pub path: String,
    pub title: String,
    /// Event tag (EventKind name) or "Manual".
    pub event: Option<String>,
    pub duration_secs: f64,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub thumb_path: Option<String>,
    pub created_unix_ms: i64,
}

/// Fields supplied on insert (id + created timestamp are assigned by the DB).
#[derive(Debug, Clone, Default)]
pub struct NewClip {
    pub path: String,
    pub title: String,
    pub event: Option<String>,
    pub duration_secs: f64,
    pub width: i64,
    pub height: i64,
    pub size_bytes: i64,
    pub thumb_path: Option<String>,
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
                created_unix_ms INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_clips_created ON clips(created_unix_ms DESC);",
        )
        .map_err(|e| format!("init schema: {e}"))?;
        Ok(Library { conn })
    }

    /// Insert a clip; returns its new id.
    pub fn insert(&self, clip: &NewClip) -> Result<i64, String> {
        let created = now_unix_ms();
        self.conn
            .execute(
                "INSERT INTO clips
                   (path, title, event, duration_secs, width, height, size_bytes, thumb_path, created_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    clip.path,
                    clip.title,
                    clip.event,
                    clip.duration_secs,
                    clip.width,
                    clip.height,
                    clip.size_bytes,
                    clip.thumb_path,
                    created,
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
                        thumb_path, created_unix_ms
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
                        thumb_path, created_unix_ms
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
    Ok(ClipRecord {
        id: row.get(0)?,
        path: row.get(1)?,
        title: row.get(2)?,
        event: row.get(3)?,
        duration_secs: row.get(4)?,
        width: row.get(5)?,
        height: row.get(6)?,
        size_bytes: row.get(7)?,
        thumb_path: row.get(8)?,
        created_unix_ms: row.get(9)?,
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
            duration_secs: 12.0,
            width: 2560,
            height: 1440,
            size_bytes: 1234,
            thumb_path: None,
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
    fn rename_missing_errors() {
        let lib = Library::open_in_memory().unwrap();
        assert!(lib.rename(999, "x").is_err());
    }
}
