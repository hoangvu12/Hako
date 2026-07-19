//! Clip editor commands: lossless trim, per-track remux, and the range reads
//! the waveform/preview UI issues.
//!
//! Both mutating commands follow the same shape -- an `async` command that hands
//! the FFmpeg work to a blocking `*_blocking` twin on a worker thread, holding
//! the library lock only to read the source row and to commit the result, never
//! across the remux itself.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager, State};

use crate::library::db::{rebase_marks, shift_marks, ClipRecord, NewClip};

use super::{clip_output_path, generate_filmstrip, generate_thumbnail, LibraryState};

/// Where a trim writes its result: replace the original file, or save a copy.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrimMode {
    Overwrite,
    Copy,
}

/// Loss-lessly trim a clip to `[start, end)` seconds (stream copy, optional audio
/// drop). `Copy` writes a new library clip; `Overwrite` replaces the original
/// file in place and refreshes its row. Returns the resulting record.
///
/// The FFmpeg remux is slow IO, so the library lock is only held to read the
/// source row and to commit the result — never across the trim itself.
#[tauri::command]
pub async fn trim_clip(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    drop_audio: bool,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    // Off the main thread (see `remux_with_tracks`): the FFmpeg stream-copy, the
    // thumbnail/filmstrip render, and the overwrite retry-sleep would otherwise
    // freeze the UI for the duration of the save.
    tauri::async_runtime::spawn_blocking(move || {
        trim_clip_blocking(app, id, start, end, drop_audio, mode)
    })
    .await
    .map_err(|e| format!("trim task failed: {e}"))?
}

fn trim_clip_blocking(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    drop_audio: bool,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let library = app.state::<LibraryState>();
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let input = PathBuf::from(&rec.path);

    match mode {
        TrimMode::Copy => {
            let out = clip_output_path(&app)?;
            let res = crate::library::trim::trim_clip(&input, &out, start, end, drop_audio)?;
            let thumb = generate_thumbnail(&app, &out);
            let filmstrip = generate_filmstrip(&app, &out, res.duration_secs);
            let size_bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            let new = NewClip {
                path: out.to_string_lossy().to_string(),
                title: format!("{} (trim)", rec.title),
                event: rec.event.clone(),
                events: rec.events.clone(),
                event_marks: shift_marks(
                    &rebase_marks(&rec.event_marks, start, end),
                    res.start_shift_secs,
                ),
                duration_secs: res.duration_secs,
                width: res.width,
                height: res.height,
                size_bytes,
                thumb_path: thumb,
                filmstrip_path: filmstrip,
                ..NewClip::context_from(&rec)
            };
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                let id = lib.insert(&new)?;
                lib.get(id)?.ok_or("inserted clip vanished")?
            };
            let _ = app.emit(crate::events::CLIP_CREATED, &record);
            tracing::info!("trimmed clip {id} → copy {}", record.path);
            Ok(record)
        }
        TrimMode::Overwrite => {
            // Trim to a fresh temp file, then atomically swap it over the
            // original (which the webview may still hold open — hence retries).
            let tmp = clip_output_path(&app)?;
            let res = crate::library::trim::trim_clip(&input, &tmp, start, end, drop_audio)?;
            replace_file_retrying(&tmp, &input)?;

            let thumb = generate_thumbnail(&app, &input);
            let filmstrip = generate_filmstrip(&app, &input, res.duration_secs);
            let size_bytes = std::fs::metadata(&input)
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                lib.update_media(
                    id,
                    res.duration_secs,
                    res.width,
                    res.height,
                    size_bytes,
                    thumb.as_deref(),
                    filmstrip.as_deref(),
                )?;
                lib.update_event_marks(
                    id,
                    &shift_marks(
                        &rebase_marks(&rec.event_marks, start, end),
                        res.start_shift_secs,
                    ),
                )?;
                lib.get(id)?.ok_or("clip vanished after trim")?
            };
            tracing::info!("trimmed clip {id} → overwrite {}", record.path);
            Ok(record)
        }
    }
}

/// The audio tracks in a clip (count + names), for the editor's per-track
/// mute/solo/volume controls. Audio track 0 is the master "All Audio" mix;
/// 1..N are the stems. A clip with ≤1 track shows no per-track UI.
#[tauri::command]
pub async fn clip_audio_tracks(
    app: AppHandle,
    id: i64,
) -> Result<Vec<crate::library::remux::AudioTrackInfo>, String> {
    // `probe_audio_tracks` opens the file with FFmpeg — file IO + demux setup that
    // runs on the main thread for a sync command. This fires the moment a clip is
    // opened in the editor, so do it on the blocking pool to keep the open snappy.
    tauri::async_runtime::spawn_blocking(move || {
        let library = app.state::<LibraryState>();
        let rec = {
            let lib = library.0.lock().map_err(|_| "library poisoned")?;
            lib.get(id)?.ok_or("clip not found")?
        };
        crate::library::remux::probe_audio_tracks(&PathBuf::from(&rec.path))
    })
    .await
    .map_err(|e| format!("probe task failed: {e}"))?
}

/// Read a byte range `[start, end)` of a clip file for the editor's live
/// per-stem audio mixer. mediabunny decodes the stems in the webview, but it
/// can't `fetch()` the `hakoclip://` streaming scheme — WebView2 blocks
/// cross-origin fetch to a custom scheme by CORS (the `<video>` element is
/// exempt, which is why playback and export work but the mixer's decode didn't).
/// So the stem bytes are pulled over IPC via mediabunny's `CustomSource`.
/// Returns the raw slice as an `ArrayBuffer`; `end` is clamped to the file size.
#[tauri::command]
pub async fn read_clip_range(
    library: State<'_, LibraryState>,
    id: i64,
    start: u64,
    end: u64,
) -> Result<tauri::ipc::Response, String> {
    use std::io::{Read, Seek, SeekFrom};
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let mut file = std::fs::File::open(&rec.path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    let end = end.min(size);
    let start = start.min(end);
    let mut buf = vec![0u8; (end - start) as usize];
    if !buf.is_empty() {
        file.seek(SeekFrom::Start(start))
            .map_err(|e| e.to_string())?;
        file.read_exact(&mut buf).map_err(|e| e.to_string())?;
    }
    Ok(tauri::ipc::Response::new(buf))
}

/// One selected stem from the editor: its audio-track index, 0–100 volume, and
/// whether to apply offline noise suppression to it (the mic stem's "noise
/// cancel"). `denoise` defaults to false for older callers (serde default).
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct TrackVolume {
    pub index: u32,
    pub volume: f32,
    #[serde(default)]
    pub denoise: bool,
}

/// Export a clip to `[start, end)` with its audio being the chosen `tracks`
/// (stems) mixed at their volumes — the editor's per-track mute/solo/volume
/// applied on export (browsers can't switch MP4 audio tracks live). Empty
/// `tracks` ⇒ video-only; one stem at full volume ⇒ loss-less stream copy;
/// otherwise the stems are decoded, mixed, and re-encoded to one master track.
/// `Copy` writes a new library clip; `Overwrite` replaces the original.
/// The FFmpeg decode/mix/re-encode (and, on overwrite, the retry-sleep loop in
/// `replace_file_retrying`) is heavy, blocking work. A synchronous `#[command]`
/// runs on the app's main thread, so doing this inline froze the whole UI while a
/// clip exported. The command (below) runs this on the blocking pool instead, so
/// the main thread stays free and the webview's "Saving…" state keeps animating.
#[tauri::command]
pub async fn remux_with_tracks(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    tracks: Vec<TrackVolume>,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    tauri::async_runtime::spawn_blocking(move || {
        remux_with_tracks_blocking(app, id, start, end, tracks, mode)
    })
    .await
    .map_err(|e| format!("export task failed: {e}"))?
}

fn remux_with_tracks_blocking(
    app: AppHandle,
    id: i64,
    start: f64,
    end: f64,
    tracks: Vec<TrackVolume>,
    mode: TrimMode,
) -> Result<ClipRecord, String> {
    let library = app.state::<LibraryState>();
    let rec = {
        let lib = library.0.lock().map_err(|_| "library poisoned")?;
        lib.get(id)?.ok_or("clip not found")?
    };
    let input = PathBuf::from(&rec.path);
    let sel: Vec<crate::library::remux::TrackSel> = tracks
        .iter()
        .map(|t| crate::library::remux::TrackSel {
            index: t.index,
            gain: (t.volume.max(0.0)) / 100.0,
            denoise: t.denoise,
        })
        .collect();

    match mode {
        TrimMode::Copy => {
            let out = clip_output_path(&app)?;
            let res = crate::library::remux::remux_with_tracks(&input, &out, start, end, &sel)?;
            let thumb = generate_thumbnail(&app, &out);
            let filmstrip = generate_filmstrip(&app, &out, res.duration_secs);
            let size_bytes = std::fs::metadata(&out).map(|m| m.len() as i64).unwrap_or(0);
            let new = NewClip {
                path: out.to_string_lossy().to_string(),
                title: format!("{} (export)", rec.title),
                event: rec.event.clone(),
                events: rec.events.clone(),
                event_marks: shift_marks(
                    &rebase_marks(&rec.event_marks, start, end),
                    res.start_shift_secs,
                ),
                duration_secs: res.duration_secs,
                width: res.width,
                height: res.height,
                size_bytes,
                thumb_path: thumb,
                filmstrip_path: filmstrip,
                ..NewClip::context_from(&rec)
            };
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                let id = lib.insert(&new)?;
                lib.get(id)?.ok_or("inserted clip vanished")?
            };
            let _ = app.emit(crate::events::CLIP_CREATED, &record);
            tracing::info!("remuxed clip {id} → copy {}", record.path);
            Ok(record)
        }
        TrimMode::Overwrite => {
            let tmp = clip_output_path(&app)?;
            let res = crate::library::remux::remux_with_tracks(&input, &tmp, start, end, &sel)?;
            replace_file_retrying(&tmp, &input)?;
            let thumb = generate_thumbnail(&app, &input);
            let filmstrip = generate_filmstrip(&app, &input, res.duration_secs);
            let size_bytes = std::fs::metadata(&input)
                .map(|m| m.len() as i64)
                .unwrap_or(0);
            let record = {
                let lib = library.0.lock().map_err(|_| "library poisoned")?;
                lib.update_media(
                    id,
                    res.duration_secs,
                    res.width,
                    res.height,
                    size_bytes,
                    thumb.as_deref(),
                    filmstrip.as_deref(),
                )?;
                lib.update_event_marks(
                    id,
                    &shift_marks(
                        &rebase_marks(&rec.event_marks, start, end),
                        res.start_shift_secs,
                    ),
                )?;
                lib.get(id)?.ok_or("clip vanished after remux")?
            };
            tracing::info!("remuxed clip {id} → overwrite {}", record.path);
            Ok(record)
        }
    }
}

/// Replace `dst` with `src`, retrying briefly: right after a trim the webview's
/// `<video>` may still hold the old file open (Windows share-deny), so a rename
/// can transiently fail until the element releases it.
fn replace_file_retrying(src: &Path, dst: &Path) -> Result<(), String> {
    let mut last = String::new();
    for attempt in 0..20 {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e.to_string();
                std::thread::sleep(std::time::Duration::from_millis(if attempt < 5 {
                    50
                } else {
                    150
                }));
            }
        }
    }
    let _ = std::fs::remove_file(src);
    Err(format!(
        "could not replace original clip (file in use?): {last}"
    ))
}
