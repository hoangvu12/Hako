//! Cloud retention — "free up space". Evicts the local files of clips that are
//! already safely in the cloud (`status = done` AND `uploaded_at IS NOT NULL`),
//! oldest first, until the local library is back under the configured budget.
//! A direct port of Medal's eviction worker.
//!
//! The clip row stays: we flag it `evicted` and clear its thumb/filmstrip paths
//! (see [`crate::library::db::Library::mark_evicted`]), so the clip still shows
//! in the library and plays from its provider's presigned URL. Deletes go to the
//! OS Recycle Bin unless the user turned that off (`cloud_delete_to_recycle_bin`),
//! so an unwanted eviction is recoverable.
//!
//! Trigger points: the manual `cloud_free_up_space` command (a "Free up space
//! now" button) and, when `cloud_free_up_space_enabled` is on, automatically
//! after each successful upload.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{LibraryState, SettingsState};
use crate::library::db::EvictRow;

const BYTES_PER_GIB: i64 = 1024 * 1024 * 1024;

/// Outcome of a retention pass (or a stats-only probe). Mirrors `EvictStats` in
/// api.ts.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvictStats {
    /// Local bytes still on disk after this pass (non-evicted clips).
    pub local_bytes: i64,
    /// Clips with local files still on disk after this pass.
    pub local_count: i64,
    /// Configured budget in bytes (`cloud_retention_gb` × 1 GiB).
    pub budget_bytes: i64,
    /// Bytes reclaimed by this pass (0 for a stats-only probe).
    pub freed_bytes: i64,
    /// Clips evicted by this pass (0 for a stats-only probe).
    pub evicted_count: i64,
}

/// Current retention gauge without changing anything: how much local space the
/// library uses vs. the budget.
#[tauri::command]
pub fn cloud_retention_stats(app: AppHandle) -> Result<EvictStats, String> {
    let budget_bytes = budget(&app);
    let (local_bytes, local_count) = {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        guard.local_footprint()?
    };
    Ok(EvictStats {
        local_bytes,
        local_count,
        budget_bytes,
        ..Default::default()
    })
}

/// Manually run a retention pass (the "Free up space now" button). Evicts oldest
/// uploaded clips until under budget regardless of the auto toggle.
#[tauri::command]
pub fn cloud_free_up_space(app: AppHandle) -> Result<EvictStats, String> {
    run(&app)
}

/// Auto-trigger after a successful upload, gated on `cloud_free_up_space_enabled`.
/// Best-effort and non-fatal — a retention failure must never fail the upload.
pub fn maybe_free_up_space(app: &AppHandle) {
    let enabled = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.cloud_free_up_space_enabled))
        .unwrap_or(false);
    if !enabled {
        return;
    }
    if crate::commands::pause_background_work(app) {
        tracing::info!("cloud retention: deferred while gaming");
        return;
    }

    match run(app) {
        Ok(stats) if stats.evicted_count > 0 => tracing::info!(
            "cloud retention: evicted {} clip(s), freed {} bytes",
            stats.evicted_count,
            stats.freed_bytes
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!("cloud retention pass failed: {e}"),
    }
}

/// The ids of configured providers that can presign a read URL — the set a
/// clip must intersect to be eligible for local eviction (see [`run`]). Loaded
/// fresh each pass (cheap: a small JSON file). If the config can't be resolved,
/// the set is empty, so nothing is evicted — fail safe.
fn presign_capable_provider_ids(app: &AppHandle) -> std::collections::HashSet<String> {
    super::config_dir(app)
        .map(|dir| super::providers::load_providers(&dir))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.kind.supports_presign())
        .map(|p| p.id)
        .collect()
}

/// The configured local-cache budget in bytes.
fn budget(app: &AppHandle) -> i64 {
    let gib = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.cloud_retention_gb))
        .unwrap_or(5);
    (gib as i64).saturating_mul(BYTES_PER_GIB)
}

/// Core eviction pass. Reads the candidate list + footprint under the library
/// lock, releases it to do the (potentially slow) file deletes lock-free, then
/// re-acquires it to flag the evicted rows and emit the updated clips.
fn run(app: &AppHandle) -> Result<EvictStats, String> {
    let budget_bytes = budget(app);
    let recycle = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.cloud_delete_to_recycle_bin))
        .unwrap_or(true);

    // Snapshot footprint + candidates, then drop the lock before touching disk.
    let (mut local_bytes, local_count, candidates) = {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        let (bytes, count) = guard.local_footprint()?;
        let candidates = guard.evictable_clips()?;
        (bytes, count, candidates)
    };

    // Presign gate (handoff §0): only evict a clip when at least one of its
    // completed uploads is to a presign-capable provider, so the cloud-only copy
    // can still stream-play from a presigned `remote_url`. Drive/Dropbox/OneDrive
    // can't presign, so a clip backed *only* by those keeps its local file —
    // they give backup, not local-space reclamation.
    let presign_ids = presign_capable_provider_ids(app);
    let candidates: Vec<_> = candidates
        .into_iter()
        .filter(|c| c.provider_ids.iter().any(|id| presign_ids.contains(id)))
        .collect();

    let mut stats = EvictStats {
        local_bytes,
        local_count,
        budget_bytes,
        ..Default::default()
    };
    if local_bytes <= budget_bytes {
        return Ok(stats); // already under budget — nothing to do.
    }

    // Evict oldest-first until under budget (the newest clips stay on disk).
    let mut evicted_ids: Vec<i64> = Vec::new();
    for c in candidates {
        if local_bytes <= budget_bytes {
            break;
        }
        evict_files(&c, recycle);
        evicted_ids.push(c.id);
        local_bytes -= c.size_bytes;
        stats.freed_bytes += c.size_bytes;
        stats.evicted_count += 1;
    }

    // Flag the rows + emit the updated records so the library UI drops to
    // cloud-only playback without a manual refetch.
    if !evicted_ids.is_empty() {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        for id in &evicted_ids {
            let _ = guard.mark_evicted(*id);
            if let Ok(Some(rec)) = guard.get(*id) {
                let _ = app.emit(crate::events::CLIP_CREATED, &rec);
            }
        }
    }

    stats.local_bytes = local_bytes;
    stats.local_count = local_count - stats.evicted_count;
    Ok(stats)
}

/// Delete a clip's local *video* file, to the Recycle Bin when `recycle` is set,
/// else a hard delete. The thumbnail and filmstrip are intentionally left on
/// disk — they're a few KB next to a multi-MB video, and keeping them lets a
/// cloud-only clip still render its real poster (and editor filmstrip) instead
/// of a blank placeholder. Best-effort: a missing/already-deleted file is fine.
fn evict_files(row: &EvictRow, recycle: bool) {
    remove_path(&row.path, recycle);
}

fn remove_path(path: &str, recycle: bool) {
    let p = Path::new(path);
    if !p.exists() {
        return;
    }
    let result = if recycle {
        trash::delete(p).map_err(|e| e.to_string())
    } else {
        std::fs::remove_file(p).map_err(|e| e.to_string())
    };
    if let Err(e) = result {
        tracing::warn!("retention: failed to delete {path}: {e}");
    }
}
