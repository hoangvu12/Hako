//! "Download to edit" — the inverse of the upload engine. Cloud retention can
//! evict a clip's local file ("free up space"), leaving the row `evicted` and
//! playable only from its presigned `remote_url`. Editing (trim/remux) still
//! needs the real bytes on disk, so this re-fetches the object from the provider
//! back to the clip's original path, regenerates its thumbnail/filmstrip, and
//! clears the `evicted` flag — after which the normal editor path just works.
//!
//! This mirrors Medal's cloud-clip edit flow (download-with-progress → edit),
//! minus Medal's mandatory re-upload: our download is non-destructive (it only
//! restores local bytes), so there's no "you'll lose the original" confirm.
//!
//! Byte progress streams over [`events::CLOUD_DOWNLOAD_PROGRESS`] (throttled like
//! upload); the start/end transition over [`events::CLOUD_DOWNLOAD_STATUS`].

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

use super::{operator, upload::build_op, CloudState};
use crate::commands::{generate_filmstrip, generate_thumbnail, LibraryState};
use crate::events;
use crate::library::db::{cloud_status, ClipRecord};

/// Bytes pulled per ranged GET. Matches the upload part size.
const CHUNK: u64 = 8 * 1024 * 1024;
/// Min interval between progress events (the renderer doesn't need more).
const PROGRESS_THROTTLE: Duration = Duration::from_millis(250);

const STATUS_DOWNLOADING: &str = "downloading";
const STATUS_DONE: &str = "done";
const STATUS_ERROR: &str = "error";

#[derive(Clone, Serialize)]
struct DownloadProgress {
    clip_id: i64,
    received: u64,
    total: u64,
    bytes_per_sec: u64,
}

#[derive(Clone, Serialize)]
struct DownloadStatus {
    clip_id: i64,
    status: String,
    error: Option<String>,
}

fn emit_status(app: &AppHandle, clip_id: i64, status: &str, error: Option<&str>) {
    let _ = app.emit(
        events::CLOUD_DOWNLOAD_STATUS,
        DownloadStatus {
            clip_id,
            status: status.to_string(),
            error: error.map(str::to_string),
        },
    );
}

fn emit_progress(app: &AppHandle, clip_id: i64, received: u64, total: u64, started: Instant) {
    let secs = started.elapsed().as_secs_f64();
    let bytes_per_sec = if secs > 0.0 { (received as f64 / secs) as u64 } else { 0 };
    let _ = app.emit(
        events::CLOUD_DOWNLOAD_PROGRESS,
        DownloadProgress {
            clip_id,
            received,
            total,
            bytes_per_sec,
        },
    );
}

/// Re-download an evicted clip's file from the cloud so it can be edited locally.
/// No-op (returns the clip as-is) if it isn't evicted. Returns the refreshed
/// record (`evicted = false`, thumbnail/filmstrip restored).
#[tauri::command]
pub async fn cloud_download_clip(app: AppHandle, clip_id: i64) -> Result<ClipRecord, String> {
    // Resolve the clip; its `path` is where the file used to (and will again) live.
    let clip = {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        guard.get(clip_id)?.ok_or("clip not found")?
    };
    if !clip.evicted {
        return Ok(clip); // already local — nothing to download.
    }

    // Pick a completed cloud upload to pull from (provider + object key).
    let row = {
        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        guard.cloud_status(Some(clip_id))?
    }
    .into_iter()
    .find(|r| r.status == cloud_status::DONE && r.remote_path.is_some())
    .ok_or("this clip has no completed cloud copy to download")?;
    let provider_id = row.provider_id;
    let key = row.remote_path.expect("filtered to Some above");

    // Dedupe double-clicks; released on every exit path below.
    if !app.state::<CloudState>().begin_download(clip_id) {
        return Err("a download for this clip is already in progress".into());
    }
    emit_status(&app, clip_id, STATUS_DOWNLOADING, None);

    let outcome = download_and_finalize(&app, clip_id, &provider_id, &key, &clip).await;

    app.state::<CloudState>().end_download(clip_id);
    match &outcome {
        Ok(rec) => {
            emit_status(&app, clip_id, STATUS_DONE, None);
            // Tell the library/editor the clip is local again (same channel the
            // grid already uses for evicted rows), so the UI re-enables editing.
            let _ = app.emit(events::CLIP_CREATED, rec);
            tracing::info!("cloud download done: clip {clip_id} → {}", rec.path);
        }
        Err(e) => {
            emit_status(&app, clip_id, STATUS_ERROR, Some(e));
            tracing::warn!("cloud download failed: clip {clip_id}: {e}");
        }
    }
    outcome
}

/// Stream the object to a temp file, move it into place, regenerate derived
/// assets, and flip the row back to local. Any error leaves the row evicted.
async fn download_and_finalize(
    app: &AppHandle,
    clip_id: i64,
    provider_id: &str,
    key: &str,
    clip: &ClipRecord,
) -> Result<ClipRecord, String> {
    let op = build_op(app, provider_id)?;

    let dest = PathBuf::from(&clip.path);
    // Land in a sibling temp file first so a failed/partial download never
    // masquerades as a real clip at `dest`.
    let part = dest.with_extension("part");

    if let Err(e) = stream_to_file(app, &op, key, clip_id, &part).await {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }

    // Move into place + regenerate thumb/filmstrip (FFmpeg, blocking) + commit.
    let app = app.clone();
    let duration = clip.duration_secs;
    tauri::async_runtime::spawn_blocking(move || -> Result<ClipRecord, String> {
        // Replace any stale file already at the destination, then move ours in.
        let _ = std::fs::remove_file(&dest);
        std::fs::rename(&part, &dest).map_err(|e| format!("move downloaded file into place: {e}"))?;

        let thumb = generate_thumbnail(&app, &dest);
        let filmstrip = generate_filmstrip(&app, &dest, duration);

        let lib = app.state::<LibraryState>();
        let guard = lib.0.lock().map_err(|_| "library poisoned")?;
        guard.mark_rehydrated(clip_id, thumb.as_deref(), filmstrip.as_deref())?;
        guard.get(clip_id)?.ok_or_else(|| "clip vanished after download".to_string())
    })
    .await
    .map_err(|e| format!("download finalize task failed: {e}"))?
}

/// Ranged GET loop: stat for the total size, then pull `CHUNK`-sized ranges into
/// `part`, emitting throttled progress. Sequential (one clip's download at a
/// time, user-initiated) — no need for the upload engine's parallel parts.
async fn stream_to_file(
    app: &AppHandle,
    op: &opendal::Operator,
    key: &str,
    clip_id: i64,
    part: &Path,
) -> Result<(), String> {
    let total = op
        .stat(key)
        .await
        .map_err(|e| operator::friendly_error(&e))?
        .content_length();

    if let Some(parent) = part.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut file = tokio::fs::File::create(part)
        .await
        .map_err(|e| format!("create temp file: {e}"))?;

    let started = Instant::now();
    let mut last_emit = Instant::now();
    let mut received: u64 = 0;
    while received < total {
        let end = (received + CHUNK).min(total);
        let buf = op
            .read_with(key)
            .range(received..end)
            .await
            .map_err(|e| operator::friendly_error(&e))?;
        file.write_all(&buf.to_vec())
            .await
            .map_err(|e| format!("write temp file: {e}"))?;
        received = end;

        if last_emit.elapsed() >= PROGRESS_THROTTLE {
            emit_progress(app, clip_id, received, total, started);
            last_emit = Instant::now();
        }
    }

    file.flush().await.map_err(|e| format!("flush temp file: {e}"))?;
    // Final 100% tick so the UI lands exactly on total.
    emit_progress(app, clip_id, received, total, started);
    Ok(())
}
