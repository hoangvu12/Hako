//! Schema creation and forward migrations.
//!
//! `from_conn` is the single place the DDL lives: it creates the tables, applies
//! the pragmas, best-effort adds the columns that older databases predate, then
//! runs the one-off data migrations at the bottom of this file. Nothing here
//! changes when a feature is added -- only when the shape of the database does.

use std::path::Path;

use rusqlite::Connection;

use super::Library;

impl Library {
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
        // User-added "record any game" list (Medal's `CustomGameDatabase`). Plain-
        // text (no need to encrypt like Medal). Match key is `process_name`,
        // deduped by the UNIQUE constraint (stored lowercase, matched case-insens).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS custom_games (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                process_name TEXT NOT NULL,
                display_name TEXT NOT NULL,
                window_class TEXT,
                caption      TEXT,
                enabled      INTEGER NOT NULL DEFAULT 1,
                added_at     INTEGER NOT NULL,
                icon         TEXT,
                UNIQUE(process_name)
            );",
        )
        .map_err(|e| format!("init custom_games schema: {e}"))?;
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
        // Custom-game exe icon (DBs created before "record any game" icons).
        let _ = conn.execute("ALTER TABLE custom_games ADD COLUMN icon TEXT", []);
        // One-time data migrations (best-effort; never block open).
        let _ = relabel_legacy_standard(&conn);
        let _ = backfill_game_valorant(&conn);
        Ok(Library { conn })
    }
}

pub(super) fn relabel_legacy_standard(conn: &Connection) -> Result<(), String> {
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
