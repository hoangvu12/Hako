//! The `cloud_uploads` table: state for the cloud-upload engine
//! (`src-tauri/src/cloud`).
//!
//! Rows move QUEUED -> UPLOADING -> DONE/ERROR/CANCELED (see [`cloud_status`])
//! and cascade-delete with their clip. The library owns this table because
//! eviction (Medal's "free up space") joins it against `clips`; keeping both
//! behind the one `Mutex<Library>` avoids a second lock.

use rusqlite::params;
use serde::Serialize;

use super::{now_unix_ms, Library};

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

/// A user-added "record any game" entry (Medal's `CustomGameDatabase`). Once a
/// game is added — by pointing the picker at its window ("Request a Game") — it's
/// matched by `process_name` on every later launch and auto-recorded generically.
/// Matched case-insensitively on `process_name`; `window_class`/`caption` are kept
/// for future stricter matching but v1 matches on the exe name alone.

impl Library {
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
                params![
                    cloud_status::UPLOADING,
                    remote_path,
                    now_unix_ms(),
                    clip_id,
                    provider_id
                ],
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
                    .prepare(&format!(
                        "{sql} WHERE clip_id = ?1 ORDER BY updated_at DESC"
                    ))
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
