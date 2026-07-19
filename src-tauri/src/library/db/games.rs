//! The `custom_games` table -- "record any game".
//!
//! The user-added generic-capture list (Medal's `CustomGameDatabase`). The
//! generic integration reads `enabled_custom_games` each detection tick; the
//! custom-game Tauri commands own the CRUD.

use rusqlite::params;

use serde::Serialize;

use super::{now_unix_ms, Library};

/// A user-added "record any game" entry (Medal's `CustomGameDatabase`). Once a
/// game is added — by pointing the picker at its window ("Request a Game") — it's
/// matched by `process_name` on every later launch and auto-recorded generically.
/// Matched case-insensitively on `process_name`; `window_class`/`caption` are kept
/// for future stricter matching but v1 matches on the exe name alone.
#[derive(Debug, Clone, Serialize)]
pub struct CustomGame {
    pub id: i64,
    /// Exe file name, lowercase, no path (e.g. `"gta5.exe"`).
    pub process_name: String,
    /// Shown in the UI + stored on clips as the real game title.
    pub display_name: String,
    pub window_class: Option<String>,
    pub caption: Option<String>,
    pub enabled: bool,
    /// Unix millis when it was added.
    pub added_at: i64,
    /// The exe's icon as a PNG `data:` URL, captured when the game was added, so
    /// the list shows its real icon even while it isn't running. `None` if
    /// extraction failed or the DB predates the column.
    pub icon: Option<String>,
}

impl Library {
    /// All custom games, newest first (for the settings list).
    pub fn list_custom_games(&self) -> Result<Vec<CustomGame>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, process_name, display_name, window_class, caption, enabled, added_at, icon
                   FROM custom_games ORDER BY added_at DESC",
            )
            .map_err(|e| format!("prepare list_custom_games: {e}"))?;
        let rows = stmt
            .query_map([], row_to_custom_game)
            .map_err(|e| format!("query custom_games: {e}"))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| format!("read custom_game row: {e}"))?);
        }
        Ok(out)
    }

    /// Only the enabled custom games — the generic detector's match list.
    pub fn enabled_custom_games(&self) -> Result<Vec<CustomGame>, String> {
        Ok(self
            .list_custom_games()?
            .into_iter()
            .filter(|g| g.enabled)
            .collect())
    }

    /// Insert (or, on a `process_name` collision, refresh the display name of) a
    /// custom game. Returns the resulting row. `process_name` is stored lowercase;
    /// re-adding the same exe updates its name + re-enables it rather than erroring.
    pub fn add_custom_game(
        &self,
        process_name: &str,
        display_name: &str,
        window_class: Option<&str>,
        caption: Option<&str>,
        icon: Option<&str>,
    ) -> Result<CustomGame, String> {
        let process_name = process_name.trim().to_ascii_lowercase();
        if process_name.is_empty() {
            return Err("custom game has no process name".into());
        }
        let display_name = display_name.trim();
        let display_name = if display_name.is_empty() {
            &process_name
        } else {
            display_name
        };
        self.conn
            .execute(
                "INSERT INTO custom_games
                    (process_name, display_name, window_class, caption, icon, enabled, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
                 ON CONFLICT(process_name) DO UPDATE SET
                    display_name = excluded.display_name,
                    window_class = excluded.window_class,
                    caption = excluded.caption,
                    -- keep the existing icon if this re-add couldn't extract one.
                    icon = COALESCE(excluded.icon, custom_games.icon),
                    enabled = 1",
                params![
                    process_name,
                    display_name,
                    window_class,
                    caption,
                    icon,
                    now_unix_ms()
                ],
            )
            .map_err(|e| format!("add_custom_game: {e}"))?;
        self.custom_game_by_process(&process_name)?
            .ok_or_else(|| "inserted custom game vanished".into())
    }

    /// Remove a custom game by id.
    pub fn remove_custom_game(&self, id: i64) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM custom_games WHERE id = ?1", params![id])
            .map_err(|e| format!("remove_custom_game: {e}"))?;
        Ok(())
    }

    /// Enable/disable a custom game (a disabled entry is skipped by detection but
    /// kept in the list).
    pub fn set_custom_game_enabled(&self, id: i64, enabled: bool) -> Result<(), String> {
        let n = self
            .conn
            .execute(
                "UPDATE custom_games SET enabled = ?1 WHERE id = ?2",
                params![enabled as i64, id],
            )
            .map_err(|e| format!("set_custom_game_enabled: {e}"))?;
        if n == 0 {
            return Err(format!("no custom game with id {id}"));
        }
        Ok(())
    }

    /// Look a custom game up by its (lowercase) process name.
    fn custom_game_by_process(&self, process_name: &str) -> Result<Option<CustomGame>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, process_name, display_name, window_class, caption, enabled, added_at, icon
                   FROM custom_games WHERE process_name = ?1",
            )
            .map_err(|e| format!("prepare custom_game_by_process: {e}"))?;
        let mut rows = stmt
            .query_map(params![process_name], row_to_custom_game)
            .map_err(|e| format!("query custom_game_by_process: {e}"))?;
        match rows.next() {
            Some(r) => Ok(Some(r.map_err(|e| format!("read custom_game row: {e}"))?)),
            None => Ok(None),
        }
    }
}

fn row_to_custom_game(row: &rusqlite::Row) -> rusqlite::Result<CustomGame> {
    let enabled: i64 = row.get(5)?;
    Ok(CustomGame {
        id: row.get(0)?,
        process_name: row.get(1)?,
        display_name: row.get(2)?,
        window_class: row.get(3)?,
        caption: row.get(4)?,
        enabled: enabled != 0,
        added_at: row.get(6)?,
        icon: row.get(7)?,
    })
}
