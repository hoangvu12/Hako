//! Shared recording machinery for every game integration.
//!
//! This is the game-agnostic half of what used to be Valorant's orchestrator +
//! cut pipeline:
//! - [`GameCtx`] — the surface a game's `run` loop uses instead of touching
//!   capture/session/library directly: auto-manage capture by window detection,
//!   open/finish a Mode-B [`RecordingSession`] teed off the live encode stream,
//!   and consult the config-restart signal.
//! - [`cut_placed_windows`] — the windowed cut tail: merge overlapping windows,
//!   skip mostly-frozen clips, stream-copy each window out of the session file,
//!   and register it in the library. Both games reconcile their events to PTS in
//!   their own way, then hand the placed windows here.
//!
//! A game's `run` owns its cadence (Valorant polls presence + state machine;
//! League polls a live event feed); everything below is identical for both.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, CaptureState, RecorderStatus};
use crate::core::capture::{self, ClipBuffer};
use crate::core::session::{SessionOutput, SessionWriter};
use crate::events;
use crate::games::event::EventKind;
use crate::games::{CaptureOwner, GameId, GameIntegration};
use crate::library::db::{EventMark, NewClip};
use crate::settings::AutoCaptureMode;

/// The user's configured auto-capture mode for `game`, read from the shared
/// settings state. Defaults to Highlights when settings are momentarily
/// unavailable (e.g. an IPC call racing startup) — the historical default. One
/// helper for every smart game instead of a `current_auto_mode` per integration.
pub fn game_auto_mode(app: &AppHandle, game: GameId) -> AutoCaptureMode {
    app.try_state::<commands::SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.game_auto_mode(game)))
        .unwrap_or(AutoCaptureMode::Highlights)
}

/// Whether the user has fully disabled Hako for `game` (no buffer, no
/// auto-record). Defaults to enabled when settings are unavailable.
pub fn game_capture_disabled(app: &AppHandle, game: GameId) -> bool {
    app.try_state::<commands::SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.game_disabled(game)))
        .unwrap_or(false)
}

/// Session-mode continuous recording: start a rolling full-session recording
/// while capture is live and the mode is Session, and stop (persist) it when it
/// isn't. Shared by every game's run loop — the only per-game difference was the
/// session-name label, now derived from the game id.
pub fn manage_full_session(
    ctx: &GameCtx,
    mode: AutoCaptureMode,
    slot: &mut Option<RecordingSession>,
) {
    let want = mode == AutoCaptureMode::Session && ctx.is_capturing();
    match (want, slot.is_some()) {
        (true, false) => {
            let name = format!("{}_fullsession", ctx.id().as_str());
            if let Some(fs) = ctx.open_session(&name, true) {
                tracing::info!("session-record: rolling → {}", fs.session_path.display());
                *slot = Some(fs);
            }
        }
        (false, true) => {
            if let Some(fs) = slot.take() {
                finish_full_session(&ctx.app, fs);
            }
        }
        _ => {}
    }
}

/// Persist a completed full-session recording as one "Full Session" clip, then
/// remove the temp file. The blocking save runs off the async runtime.
pub fn finish_full_session(app: &AppHandle, fs: RecordingSession) {
    let Some((path, _output)) = fs.finish() else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = save_whole_session(
            &app,
            &path,
            "Full Session",
            "Full Session",
            crate::library::db::NewClip::default(),
        ) {
            tracing::warn!("session-record: save failed: {e}");
        }
        let _ = std::fs::remove_file(&path);
    });
}

// ===========================================================================
// GameCtx — the per-game handle into the shared recording machinery
// ===========================================================================

/// The shared surface a game integration's `run` loop drives. Holds the app
/// handle and a reference back to the integration (for window detection /
/// process checks), nothing game-specific.
pub struct GameCtx {
    pub app: AppHandle,
    game: Arc<dyn GameIntegration>,
    /// Last recorder-status payload emitted from this loop, so identical
    /// consecutive snapshots aren't re-emitted (see [`emit_recorder_status`]).
    last_status: Mutex<Option<RecorderStatus>>,
}

impl GameCtx {
    pub fn new(app: AppHandle, game: Arc<dyn GameIntegration>) -> Self {
        GameCtx {
            app,
            game,
            last_status: Mutex::new(None),
        }
    }

    pub fn id(&self) -> GameId {
        self.game.id()
    }

    /// The game window HWND, or `None` if not running/visible.
    pub fn find_window(&self) -> Option<i64> {
        self.game.find_window()
    }

    /// Whether the game process is running.
    pub fn game_running(&self) -> bool {
        self.game.detect_running()
    }

    /// Whether a capture is currently running (ours or the user's).
    pub fn is_capturing(&self) -> bool {
        commands::is_capturing(&self.app)
    }

    /// Consume a pending mid-session config-restart request (see
    /// [`commands::ConfigRestartSignal`]). True at most once per request.
    pub fn take_config_restart(&self) -> bool {
        commands::take_config_restart_request(&self.app)
    }

    /// Restart the live capture so a new encode/audio config takes effect. Only
    /// meaningful once nothing is teeing into the buffer.
    pub fn restart_capture(&self) {
        commands::restart_capture_now(&self.app);
    }

    /// Push the live recorder-status snapshot (drives the titlebar indicator).
    ///
    /// Gated two ways, since this fires on every game-loop tick (1–2 Hz × three
    /// loops): skipped entirely while the main window is hidden to tray (nothing
    /// consumes it and the renderer is suspended — the popover refetches on demand
    /// when it reopens, mirroring the CAPTURE_STATS gate in `core::capture`), and
    /// skipped when the payload is unchanged since the last emit.
    pub fn emit_recorder_status(&self) {
        let visible = self
            .app
            .get_webview_window("main")
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(true);
        if !visible {
            return;
        }
        let status = commands::recorder_status_snapshot(&self.app);
        if let Ok(mut last) = self.last_status.lock() {
            if last.as_ref() == Some(&status) {
                return;
            }
            *last = Some(status.clone());
        }
        let _ = self.app.emit(events::RECORDER_STATUS, &status);
    }

    /// The running capture's clip buffer, or `None` if nothing is capturing.
    pub fn capture_clip(&self) -> Option<Arc<ClipBuffer>> {
        let state = self.app.state::<CaptureState>();
        let guard = state.0.lock().ok()?;
        guard.as_ref().map(|rc| rc.clip())
    }

    /// Open a Mode-B [`RecordingSession`] teed off the live capture's clip buffer.
    /// `None` if no capture is running / the encoder isn't producing frames yet,
    /// or (until `audio_grace_expired`) if not all planned audio tracks have
    /// published their metadata — the caller latches and retries.
    pub fn open_session(
        &self,
        prefix: &str,
        audio_grace_expired: bool,
    ) -> Option<RecordingSession> {
        let clip = self.capture_clip()?;
        let meta = clip.clip_meta()?; // encoder not open yet ⇒ can't record
        let audio_tracks = clip.audio_track_metas();
        if !audio_grace_expired && audio_tracks.len() < clip.audio_track_count() {
            tracing::debug!(
                "auto-clip: deferring session start — audio {}/{} tracks published",
                audio_tracks.len(),
                clip.audio_track_count()
            );
            return None;
        }
        let session_path =
            std::env::temp_dir().join(format!("hako_{prefix}_{}.mp4", unix_millis()));
        let writer = match SessionWriter::start(&session_path, &meta, &audio_tracks) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::warn!("auto-clip: could not open session writer: {e}");
                return None;
            }
        };
        clip.install_session(writer.clone());
        Some(RecordingSession {
            clip,
            session: writer,
            session_path,
            fps: meta.fps,
        })
    }

    /// Medal-style game detection: auto-start capture when the game window appears
    /// (so the encoder is warm before a match begins) and auto-stop when the game
    /// exits. Only ever touches a capture *we* started (tracked in `st` +
    /// [`CaptureOwner`]), so a user's manual capture is never auto-stopped, and two
    /// games never fight over the single global capture (first to detect wins).
    pub fn auto_manage_capture(&self, st: &mut AutoCaptureState, disabled: bool) {
        // A disabled game is treated as "not present" for *our* auto-capture: we
        // never start, and if we'd previously auto-started, the `None` arm below
        // stops it and releases the arbiter. A user's manual capture is untouched
        // (the `None` arm only stops captures we started, tracked in `st`).
        let game = if disabled { None } else { self.find_window() };
        let capturing = self.is_capturing();
        match game {
            // Game up but nothing recording → claim ownership + start.
            Some(hwnd) if !capturing => {
                if capture::is_window_minimized(hwnd) {
                    return; // not presenting; don't inject into a frozen window
                }
                if Instant::now() < st.next_attempt {
                    return; // backing off from a recent failure
                }
                // Another game already owns the capture arbiter → leave it be.
                if !self.claim_owner() {
                    return;
                }
                match commands::start_capture_with(&self.app, hwnd, None, None) {
                    Ok(()) => {
                        st.auto_capturing = true;
                        tracing::info!(
                            "auto-capture: {} detected → capture started",
                            self.id().display_name()
                        );
                    }
                    Err(e) => {
                        // Release our tentative claim so another game (or a retry)
                        // can take it; back off before re-injecting.
                        self.release_owner();
                        st.next_attempt = Instant::now() + CAPTURE_RETRY_BACKOFF;
                        tracing::warn!(
                            "auto-capture: could not start capture (retrying in {}s): {e}",
                            CAPTURE_RETRY_BACKOFF.as_secs()
                        );
                    }
                }
            }
            // Game gone → stop only the capture we started ourselves.
            None => {
                if st.auto_capturing && capturing {
                    commands::stop_capture_with(&self.app);
                    tracing::info!(
                        "auto-capture: {} closed → capture stopped",
                        self.id().display_name()
                    );
                }
                if st.auto_capturing {
                    self.release_owner();
                }
                st.auto_capturing = false;
                st.next_attempt = Instant::now();
            }
            // Game up and already capturing (ours or the user's) → leave it be.
            Some(_) => {}
        }
    }

    /// Try to claim the global capture arbiter for this game. Returns true if we
    /// now own it (was free or already ours), false if another game holds it.
    fn claim_owner(&self) -> bool {
        let Some(owner) = self.app.try_state::<CaptureOwner>() else {
            return true; // arbiter not managed (shouldn't happen) — don't block
        };
        let mut guard = match owner.0.lock() {
            Ok(g) => g,
            Err(_) => return true,
        };
        match *guard {
            Some(g) if g != self.id() => false,
            _ => {
                *guard = Some(self.id());
                true
            }
        }
    }

    /// Release the capture arbiter if we hold it (no-op otherwise).
    fn release_owner(&self) {
        if let Some(owner) = self.app.try_state::<CaptureOwner>() {
            if let Ok(mut guard) = owner.0.lock() {
                if *guard == Some(self.id()) {
                    *guard = None;
                }
            }
        }
    }
}

/// Loop-local auto-capture state (kept by each game's `run`).
pub struct AutoCaptureState {
    /// True while *we* auto-started the capture (so we only auto-stop our own).
    pub auto_capturing: bool,
    /// Earliest time to retry an auto-start after a failure (backoff).
    pub next_attempt: Instant,
}

impl AutoCaptureState {
    pub fn new() -> Self {
        AutoCaptureState {
            auto_capturing: false,
            next_attempt: Instant::now(),
        }
    }
}

impl Default for AutoCaptureState {
    fn default() -> Self {
        AutoCaptureState::new()
    }
}

/// Backoff after a failed auto-start before retrying (don't re-inject the hook
/// into the game every tick when it isn't capturable yet).
const CAPTURE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(20);

// ===========================================================================
// RecordingSession — a Mode-B full-match writer teed off the live capture
// ===========================================================================

/// An in-progress full-session recording: a [`SessionWriter`] teed off the live
/// capture's [`ClipBuffer`]. Finishing yields the MP4 path + its wall-clock↔PTS
/// timeline; discarding tears it down and removes the temp file.
pub struct RecordingSession {
    clip: Arc<ClipBuffer>,
    session: Arc<SessionWriter>,
    pub session_path: PathBuf,
    /// Capture fps (session video time base).
    pub fps: u32,
}

impl RecordingSession {
    /// Stop teeing and finish the writer, returning the MP4 path + session output
    /// (timeline + frozen spans). `None` on a writer error (temp file removed).
    pub fn finish(self) -> Option<(PathBuf, SessionOutput)> {
        self.clip.take_session();
        match self.session.finish() {
            Ok(x) => Some(x),
            Err(e) => {
                tracing::warn!("auto-clip: finishing session failed: {e}");
                let _ = std::fs::remove_file(&self.session_path);
                None
            }
        }
    }

    /// Tear down without producing a clip (stale/aborted match): stop teeing,
    /// finish the writer, drop the temp file.
    pub fn discard(self) {
        self.clip.take_session();
        let _ = self.session.finish();
        let _ = std::fs::remove_file(&self.session_path);
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

// ===========================================================================
// Shared clip-window math (game-agnostic)
// ===========================================================================

/// Clip window `[center − before, center + after]` in PTS units, clamped to ≥ 0.
pub fn clip_window(
    center_pts: i64,
    pad_before_secs: u32,
    pad_after_secs: u32,
    fps: u32,
) -> (i64, i64) {
    clip_window_span(center_pts, center_pts, pad_before_secs, pad_after_secs, fps)
}

/// Clip window for a reconciled `[start_pts, end_pts]` span: pad `before` ahead
/// of the start (the sequence's first action) and `after` past the end (its
/// last). `[start − before, end + after]`, clamped to ≥ 0.
pub fn clip_window_span(
    start_pts: i64,
    end_pts: i64,
    pad_before_secs: u32,
    pad_after_secs: u32,
    fps: u32,
) -> (i64, i64) {
    let fps = fps.max(1) as i64;
    let start = (start_pts - pad_before_secs as i64 * fps).max(0);
    let end = end_pts + pad_after_secs as i64 * fps;
    (start, end)
}

/// Merge overlapping/adjacent clip windows into one. Input order-independent.
pub fn merge_windows(windows: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    merge_windows_tol(windows, 0)
}

/// Like [`merge_windows`] but also fuses windows separated by a gap of up to
/// `tol_pts` (Medal's `OverlapMergeGrouper`).
pub fn merge_windows_tol(mut windows: Vec<(i64, i64)>, tol_pts: i64) -> Vec<(i64, i64)> {
    if windows.is_empty() {
        return windows;
    }
    windows.sort_by_key(|w| w.0);
    let mut merged = vec![windows[0]];
    for &(s, e) in &windows[1..] {
        let last = merged.last_mut().unwrap();
        if s <= last.1 + tol_pts {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    merged
}

// ===========================================================================
// cut_placed_windows — the shared windowed cut tail
// ===========================================================================

/// Inputs for the shared cut: a finished session file + how to label/clamp the
/// clips it cuts. The events are already reconciled to session PTS by the caller.
pub struct CutWindows<'a> {
    pub app: &'a AppHandle,
    pub session_path: &'a Path,
    /// Session-PTS spans recorded while capture was frozen (skip mostly-frozen).
    pub frozen_spans: &'a [(i64, i64)],
    pub fps: u32,
    /// Clamp each merged window to this many seconds (`MaxAutoClipLength`).
    pub max_clip_secs: i64,
    /// Merge tolerance: the widest after-pad among enabled kinds (seconds).
    pub merge_after_secs: u32,
    /// Display name of the game (for the honest "frozen" notice).
    pub game_label: &'a str,
    /// Suffix for clip titles (agent/champion), empty for none.
    pub title_suffix: &'a str,
    /// Game context (agent/map/mode/result/K-D-A) applied to every cut clip.
    pub clip_context: NewClip,
}

/// Result of a cut pass.
pub struct CutOutcome {
    pub cut: usize,
    pub skipped_frozen: usize,
}

/// Cut the per-event highlight clips for a finished match. `placed` is the list
/// of `(start_pts, end_pts, kind)` windows (already reconciled to session PTS);
/// `marks_all` is every seek-bar marker `(pts, kind)`. Merges overlapping
/// windows, skips mostly-frozen ones, stream-copies each out of the session
/// file, and registers it in the library. Emits an honest notice if any clip was
/// skipped because the game was minimized.
pub fn cut_placed_windows(
    cx: &CutWindows,
    placed: &[(i64, i64, EventKind)],
    marks_all: &[(i64, EventKind)],
) -> CutOutcome {
    let fps = cx.fps.max(1);
    let max_len_pts = cx.max_clip_secs * fps as i64;
    let tol_pts = cx.merge_after_secs.max(1) as i64 * fps as i64;
    let windows: Vec<(i64, i64)> = placed.iter().map(|&(s, e, _)| (s, e)).collect();
    let merged = merge_windows_tol(windows, tol_pts);

    tracing::info!(
        "auto-clip: {} event(s) → {} clip(s)",
        placed.len(),
        merged.len()
    );

    let mut cut = 0usize;
    let mut skipped_frozen = 0usize;
    for (s, e) in merged {
        let end = e.min(s + max_len_pts);
        let kind = dominant_kind(placed, s, end).unwrap_or(EventKind::Kill);
        let kinds = kinds_in_window(placed, s, end);
        let event_labels: Vec<String> = kinds.iter().map(|k| k.label().to_string()).collect();
        let event_marks = marks_in_window(marks_all, s, end, fps as i64);
        let start_sec = s as f64 / fps as f64;
        let end_sec = end as f64 / fps as f64;
        if end_sec <= start_sec {
            continue;
        }

        // Skip a clip whose window was mostly frozen (game minimized / stale
        // swapchain) — it would be a dead, single-frame clip.
        let span_pts = end - s;
        let frozen_pts = frozen_overlap(cx.frozen_spans, s, end);
        if span_pts > 0 && frozen_pts * 2 > span_pts {
            tracing::warn!(
                "auto-clip: skipping {start_sec:.1}-{end_sec:.1}s — {}% frozen \
                 (game minimized/not presenting during the match)",
                (frozen_pts * 100 / span_pts).min(100)
            );
            skipped_frozen += 1;
            continue;
        }

        let out = match commands::auto_clip_output_path(cx.app) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("auto-clip: output path: {e}");
                continue;
            }
        };
        match crate::library::trim::trim_clip(cx.session_path, &out, start_sec, end_sec, false) {
            Ok(res) => {
                let event_marks =
                    crate::library::db::shift_marks(&event_marks, res.start_shift_secs);
                let label = commands::events_summary(kind.label(), &event_labels);
                let title = if cx.title_suffix.is_empty() {
                    format!("{} — {:.0}s", label, end_sec - start_sec)
                } else {
                    format!("{} — {}", label, cx.title_suffix)
                };
                if let Err(err) = commands::finalize_auto_clip(
                    cx.app,
                    out,
                    title,
                    kind.label(),
                    &event_labels,
                    event_marks,
                    res.width,
                    res.height,
                    res.duration_secs,
                    cx.clip_context.clone(),
                ) {
                    tracing::warn!("auto-clip: library insert failed: {err}");
                } else {
                    cut += 1;
                }
            }
            Err(err) => tracing::warn!("auto-clip: cut {start_sec:.1}-{end_sec:.1}s failed: {err}"),
        }
    }
    tracing::info!("auto-clip: wrote {cut} clip(s) to the library");
    if skipped_frozen > 0 {
        let msg = if cut == 0 {
            format!(
                "Skipped {skipped_frozen} clip(s) — {} was minimized or not rendering for the \
                 match, so there was no live gameplay to clip.",
                cx.game_label
            )
        } else {
            format!(
                "Skipped {skipped_frozen} clip(s) — the game was minimized during those moments."
            )
        };
        tracing::warn!("auto-clip: {msg}");
        let _ = cx.app.emit(events::RECORDER_ERROR, &msg);
    }
    CutOutcome {
        cut,
        skipped_frozen,
    }
}

/// Save a whole Mode-B session file as a single library clip (FullMatch / Session
/// modes): stream-copy it into the clips dir (which also probes its real
/// dimensions + duration), tag it with `event`, and register it. The session temp
/// is dropped by the caller.
pub fn save_whole_session(
    app: &AppHandle,
    session_path: &Path,
    title: &str,
    event: &str,
    context: NewClip,
) -> Result<(), String> {
    /// Upper bound on a session's length (s) — trim copies to EOF within it.
    const WHOLE_FILE_SECS: f64 = 24.0 * 60.0 * 60.0;
    let out = commands::auto_clip_output_path(app)?;
    let res = crate::library::trim::trim_clip(session_path, &out, 0.0, WHOLE_FILE_SECS, false)
        .map_err(|e| format!("whole-session copy failed: {e}"))?;
    commands::finalize_auto_clip(
        app,
        out,
        title.to_string(),
        event,
        std::slice::from_ref(&event.to_string()),
        Vec::new(),
        res.width,
        res.height,
        res.duration_secs,
        context,
    )?;
    tracing::info!("auto-clip: saved {event} ({:.0}s)", res.duration_secs);
    Ok(())
}

/// Total overlap (session PTS) between clip window `[s, end)` and the frozen
/// spans (non-overlapping, ascending).
fn frozen_overlap(spans: &[(i64, i64)], s: i64, end: i64) -> i64 {
    spans
        .iter()
        .map(|&(a, b)| (end.min(b) - s.max(a)).max(0))
        .sum()
}

/// The strongest highlight kind whose anchor falls inside `[start, end]`.
fn dominant_kind(placed: &[(i64, i64, EventKind)], start: i64, end: i64) -> Option<EventKind> {
    placed
        .iter()
        .filter(|&&(s, _, _)| s >= start - 1 && s <= end)
        .map(|&(_, _, k)| k)
        .max_by_key(|k| k.priority())
}

/// Every distinct event kind anchored inside `[start, end]`, in time order,
/// deduplicated by label.
fn kinds_in_window(placed: &[(i64, i64, EventKind)], start: i64, end: i64) -> Vec<EventKind> {
    let mut hits: Vec<(i64, EventKind)> = placed
        .iter()
        .filter(|&&(s, _, _)| s >= start - 1 && s <= end)
        .map(|&(s, _, k)| (s, k))
        .collect();
    hits.sort_by_key(|&(s, _)| s);
    let mut out: Vec<EventKind> = Vec::new();
    for (_, k) in hits {
        if !out.iter().any(|e| e.label() == k.label()) {
            out.push(k);
        }
    }
    out
}

/// The seek-bar markers landing inside `[start, end]` — each reconciled marker's
/// label plus its offset (seconds) from the clip's start. Markers at
/// (near-)identical times are de-duplicated, keeping the higher-priority label.
fn marks_in_window(marks: &[(i64, EventKind)], start: i64, end: i64, fps: i64) -> Vec<EventMark> {
    let fps = fps.max(1);
    let mut hits: Vec<(i64, EventKind)> = marks
        .iter()
        .filter(|&&(p, _)| p >= start && p <= end)
        .copied()
        .collect();
    hits.sort_by_key(|&(p, _)| p);
    let tol = (fps / 5).max(1);
    let mut deduped: Vec<(i64, EventKind)> = Vec::new();
    for (p, k) in hits {
        match deduped.last_mut() {
            Some(last) if (p - last.0).abs() <= tol => {
                if k.priority() > last.1.priority() {
                    *last = (p, k);
                }
            }
            _ => deduped.push((p, k)),
        }
    }
    deduped
        .into_iter()
        .map(|(p, k)| EventMark {
            label: k.label().to_string(),
            at: ((p - start).max(0) as f64) / fps as f64,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_clamp_and_merge() {
        let (s, e) = clip_window(60, 8, 4, 60); // center 1 s, −8/+4
        assert_eq!((s, e), (0, 60 + 240)); // start clamped to 0
        let merged = merge_windows(vec![(0, 300), (250, 500), (1000, 1200)]);
        assert_eq!(merged, vec![(0, 500), (1000, 1200)]);
    }

    #[test]
    fn span_window_pads_outward_from_first_and_last() {
        let (s, e) = clip_window_span(600, 1200, 8, 4, 60);
        assert_eq!((s, e), (600 - 480, 1200 + 240));
        assert_eq!(
            clip_window_span(600, 600, 8, 4, 60),
            clip_window(600, 8, 4, 60)
        );
        assert_eq!(clip_window_span(60, 300, 8, 4, 60).0, 0);
    }

    #[test]
    fn tolerance_merge_fuses_near_windows() {
        let w = vec![(0, 300), (400, 700)];
        assert_eq!(merge_windows_tol(w.clone(), 0), vec![(0, 300), (400, 700)]);
        assert_eq!(merge_windows_tol(w.clone(), 100), vec![(0, 700)]);
        assert_eq!(merge_windows_tol(w, 50), vec![(0, 300), (400, 700)]);
    }

    #[test]
    fn marks_dedup_keeps_higher_priority_at_same_moment() {
        let marks = vec![
            (600, EventKind::Kill),
            (600, EventKind::Clutch),
            (1200, EventKind::DoubleKill),
        ];
        let out = marks_in_window(&marks, 0, 2000, 60);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label, "Clutch");
        assert_eq!(out[0].at, 10.0);
        assert_eq!(out[1].label, "Double Kill");
        assert_eq!(out[1].at, 20.0);
    }

    #[test]
    fn marks_keep_distinct_close_kills() {
        let marks = vec![(1540, EventKind::Kill), (1610, EventKind::DoubleKill)];
        assert_eq!(marks_in_window(&marks, 0, 5000, 60).len(), 2);
    }

    #[test]
    fn marks_outside_window_are_dropped() {
        let marks = vec![(100, EventKind::Kill), (9000, EventKind::Ace)];
        let out = marks_in_window(&marks, 500, 2000, 60);
        assert_eq!(out.len(), 0);
    }
}
