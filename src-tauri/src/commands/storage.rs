//! Storage locations and library relocation.
//!
//! Resolves every path the app writes to -- the clip directory (user-configured
//! or `<Videos>/Hako`), the ring buffer's scratch dir, and the settings file --
//! and implements moving an existing library to a new drive when the user
//! changes `storage_dir`.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

use crate::settings::Settings;

use super::{LibraryState, SettingsState};

/// The directory clips are written to (`<Videos>/Hako`), for the overlay's
/// disk-space monitor. `None` if the Videos dir can't be resolved.
pub fn storage_root(app: &AppHandle) -> Option<PathBuf> {
    clip_dir(app).ok()
}

/// Clip + thumbnail storage root, created if needed. Uses the user's configured
/// `storage_dir` setting when set; otherwise falls back to `<Videos>/Hako`.
pub(super) fn clip_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let configured = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.storage_dir.clone()))
        .flatten();
    resolve_clip_dir(app, configured.as_deref())
}

/// Grant the asset-protocol scope read access to the configured clip folder, so
/// `convertFileSrc` (clip-card video, posters, filmstrips) can load clips written
/// outside the static `$VIDEO/Hako` scope declared in `tauri.conf.json`. No-op
/// when `storage_dir` is unset (the default folder is already in the static
/// scope). Best-effort: a failure just means those clips fall back to 403, which
/// is logged. Recursive so the `thumbs/` subdir is covered too.
pub fn allow_storage_asset_scope(app: &AppHandle) {
    let dir = app
        .try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.storage_dir.clone()))
        .flatten();
    let Some(dir) = dir else { return };
    let dir = dir.trim();
    if dir.is_empty() {
        return;
    }
    if let Err(e) = app.asset_protocol_scope().allow_directory(dir, true) {
        tracing::warn!("could not grant asset scope for clip folder {dir}: {e}");
    }
}

/// The default storage root, `<Videos>/Hako`.
fn default_clip_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .video_dir()
        .map_err(|e| format!("resolve Videos dir: {e}"))?
        .join("Hako"))
}

/// Resolve a storage root from a raw `storage_dir` setting value, creating it on
/// disk. An empty/unset value uses the default `<Videos>/Hako`. A configured path
/// that can't be created (bad path, unplugged drive, missing permissions) falls
/// back to the default so saving a clip never hard-fails on a stale setting.
fn resolve_clip_dir(app: &AppHandle, storage_dir: Option<&str>) -> Result<PathBuf, String> {
    let configured = storage_dir
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    if let Some(custom) = configured {
        match std::fs::create_dir_all(&custom) {
            Ok(()) => return Ok(custom),
            Err(e) => tracing::warn!(
                "clip folder {} is unusable ({e}); falling back to the default folder",
                custom.display()
            ),
        }
    }
    let dir = default_clip_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create clip dir: {e}"))?;
    Ok(dir)
}

/// Move `src` to `dst`, falling back to copy+remove when the two live on
/// different volumes (`rename` is `EXDEV` across drives). No-op if `src` is
/// missing. The destination's parent is created as needed.
fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    if !src.exists() || src == dst {
        return Ok(());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // Cross-volume (or replacing an existing file): copy then drop the original.
    std::fs::copy(src, dst)
        .map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    std::fs::remove_file(src).map_err(|e| format!("remove {}: {e}", src.display()))?;
    Ok(())
}

/// If `stored` lives under `old_root`, return its path rebased onto `new_root`
/// (preserving the sub-path, e.g. `thumbs/<stem>.jpg`). Returns `None` for files
/// that aren't under the old root, so clips already elsewhere are left alone.
fn relocate(stored: &str, old_root: &Path, new_root: &Path) -> Option<PathBuf> {
    Path::new(stored)
        .strip_prefix(old_root)
        .ok()
        .map(|rel| new_root.join(rel))
}

/// How many library clips currently live under `root`.
fn count_clips_under(app: &AppHandle, root: &Path) -> u32 {
    let Some(library) = app.try_state::<LibraryState>() else {
        return 0;
    };
    let clips = match library.0.lock() {
        Ok(lib) => lib.list().unwrap_or_default(),
        Err(_) => return 0,
    };
    clips
        .iter()
        .filter(|c| Path::new(&c.path).starts_with(root))
        .count() as u32
}

/// Move every clip (and its thumbnail/filmstrip) currently under `old_root` to
/// `new_root` and repoint the library rows. Returns the number of clips moved.
/// Best-effort per file: a clip that fails to move keeps its old path, and a
/// missing thumbnail just regenerates lazily. Synchronous — callers run it off
/// the UI thread (the command below wraps it in `spawn_blocking`).
fn migrate_clips(app: &AppHandle, old_root: &Path, new_root: &Path) -> u32 {
    let library = app.state::<LibraryState>();
    // Snapshot under the lock, then release it for the slow file IO.
    let clips = match library.0.lock() {
        Ok(lib) => lib.list().unwrap_or_default(),
        Err(_) => return 0,
    };

    let mut moved = 0u32;
    for clip in clips {
        let Some(new_path) = relocate(&clip.path, old_root, new_root) else {
            continue; // not under the old folder — leave it where it is
        };
        let new_thumb = clip
            .thumb_path
            .as_deref()
            .and_then(|p| relocate(p, old_root, new_root));
        let new_film = clip
            .filmstrip_path
            .as_deref()
            .and_then(|p| relocate(p, old_root, new_root));

        // Move the video first; only repoint the row once it has landed so a
        // failure leaves the clip fully intact at its old location.
        if let Err(e) = move_file(Path::new(&clip.path), &new_path) {
            tracing::warn!("migrate clip {}: {e}", clip.path);
            continue;
        }
        if let (Some(src), Some(dst)) = (clip.thumb_path.as_deref(), new_thumb.as_ref()) {
            let _ = move_file(Path::new(src), dst);
        }
        if let (Some(src), Some(dst)) = (clip.filmstrip_path.as_deref(), new_film.as_ref()) {
            let _ = move_file(Path::new(src), dst);
        }

        let new_path = new_path.to_string_lossy().to_string();
        let new_thumb = new_thumb.map(|p| p.to_string_lossy().to_string());
        let new_film = new_film.map(|p| p.to_string_lossy().to_string());
        if let Ok(lib) = library.0.lock() {
            if let Err(e) = lib.update_paths(
                clip.id,
                &new_path,
                new_thumb.as_deref(),
                new_film.as_deref(),
            ) {
                tracing::warn!("migrate db update for clip {}: {e}", clip.id);
            }
        }
        moved += 1;
    }

    if moved > 0 {
        tracing::info!("migrated {moved} clip(s) to {}", new_root.display());
    }
    moved
}

/// Count existing library clips stored under the folder `dir` resolves to (null →
/// default `<Videos>/Hako`). Drives the "move N clips?" prompt the UI shows when
/// the user changes the clip folder; returns 0 when nothing would move.
#[tauri::command]
pub fn count_clips_in(app: AppHandle, dir: Option<String>) -> Result<u32, String> {
    let root = resolve_clip_dir(&app, dir.as_deref())?;
    Ok(count_clips_under(&app, &root))
}

/// Move existing clips from the `from` folder to the `to` folder and repoint the
/// library (both null-default-resolved). Opt-in: the UI calls this only after the
/// user confirms the "move existing clips?" prompt. Returns the count moved; a
/// no-op (0) when the two resolve to the same folder.
#[tauri::command]
pub async fn migrate_clips_to(
    app: AppHandle,
    from: Option<String>,
    to: Option<String>,
) -> Result<u32, String> {
    let old_root = resolve_clip_dir(&app, from.as_deref())?;
    let new_root = resolve_clip_dir(&app, to.as_deref())?;
    if old_root == new_root {
        return Ok(0);
    }
    // Off the UI thread — moving many large files (or copying across drives) can
    // take a while.
    tauri::async_runtime::spawn_blocking(move || migrate_clips(&app, &old_root, &new_root))
        .await
        .map_err(|e| format!("migration task failed: {e}"))
}

/// `<Videos>/Hako/.hako-buffer` — the private spool dir for the disk recording
/// buffer (created/emptied by [`crate::core::disk_buffer::DiskPacketRing`]). Kept
/// on the same drive as clips so it inherits the user's storage choice.
pub(super) fn buffer_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(clip_dir(app)?.join(".hako-buffer"))
}

/// `<Videos>/Hako/hako_clip_<unix_ms>.mp4` (ms so rapid presses don't collide).
pub(super) fn clip_output_path(app: &AppHandle) -> Result<PathBuf, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    Ok(clip_dir(app)?.join(format!("hako_clip_{ts}.mp4")))
}

/// `settings.json` in the app config dir.
pub(super) fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("resolve config dir: {e}"))?;
    Ok(Settings::file_in(&dir))
}
