//! Cloud retention -- Medal's "free up space".
//!
//! Reports what the library costs on disk and which clips are safe to evict.
//! Only clips already uploaded are evictable: eviction drops the local file but
//! keeps the row, so the clip can be rehydrated from the cloud later.

use rusqlite::params;

use super::Library;

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

impl Library {
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
            .execute("UPDATE clips SET evicted = 1 WHERE id = ?1", params![id])
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
