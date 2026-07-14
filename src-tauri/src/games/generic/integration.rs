//! The generic "record any game" [`GameIntegration`].
//!
//! A single background task refreshes generic detection each tick, auto-starts
//! capture when a detected game's window appears (reusing the shared
//! [`GameCtx::auto_manage_capture`] loop), and — because a generic game has **no
//! event feed** — records only in Manual / Session / Full-match modes (never
//! Highlights, never `cut_placed_windows`). The *real* detected title is published
//! to the status pill and stamped on every clip's `game` column, even though the
//! arbiter/settings key is the single [`GameId::Other`] bucket.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::games::generic::detect;
use crate::games::recording::{save_whole_session, AutoCaptureState, GameCtx, RecordingSession};
use crate::games::{GameId, GameIntegration};
use crate::library::db::NewClip;
use crate::settings::AutoCaptureMode;
use crate::valorant::live::{LiveMatch, LiveMatchState};

/// Detection + record cadence. Matches the smart games' active poll — the shared
/// process snapshot coalesces the reads across all loops.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The generic [`GameIntegration`] (zero-sized; all state is loop-local).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Other
    }

    fn find_window(&self) -> Option<i64> {
        // Reads the cache the `run` loop refreshes each tick — no scan here, so
        // `detected_game()` (status) stays cheap.
        detect::current_generic().map(|g| g.hwnd)
    }

    fn detect_running(&self) -> bool {
        detect::current_generic().is_some()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run(ctx).await;
    }
}

/// An in-progress whole-session recording of a generic game (Session / Full-match
/// modes): the session writer plus the real game name to stamp on the saved clip.
struct GenericActive {
    rec: RecordingSession,
    /// Real detected title, for the clip's `game` column + title.
    name: String,
    /// The mode that opened it (labels the saved clip).
    mode: AutoCaptureMode,
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    // The one whole-session recording slot (Session / Full-match). Manual mode
    // never opens one — the live buffer + save-hotkey already produce clips.
    let mut session: Option<GenericActive> = None;
    // Whether we've published a generic live-match context (so a manual F9 save is
    // tagged with the real game); cleared when our capture stops.
    let mut published_live = false;
    tracing::info!("generic (record-any-game) integration started");

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // One catalog scan per tick, shared with `find_window` + the status label.
        let detected = detect::refresh(&app);

        let disabled = current_disabled(&app);
        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            current_auto_mode(&app)
        };

        // Reuse the shared Medal-style loop: it claims the arbiter under
        // `GameId::Other` (last, so smart games win), starts capture on the
        // detected HWND, and stops the capture *we* started when the game exits.
        ctx.auto_manage_capture(&mut autocap, disabled);

        // A config change that needs a capture restart (encode/audio layout): if a
        // whole-session recording is open, close it cleanly first, then restart —
        // the same clean-split the smart games do.
        if ctx.take_config_restart() {
            if let Some(active) = session.take() {
                finish_session(&app, active);
            }
            ctx.restart_capture();
        }

        // Audio-only layout change deferred from mid-match → apply now it's safe.
        ctx.apply_pending_audio_layout();

        // Manage the whole-session recording for Session / Full-match: open one
        // while we're auto-capturing a generic game, save it when capture stops
        // (game exited / disabled / mode changed away).
        let records_whole = matches!(
            mode,
            AutoCaptureMode::Session | AutoCaptureMode::FullMatch
        );
        // Only record a session for a capture *we* auto-started — never a user's
        // manual capture (`auto_manage_capture` leaves `auto_capturing` false for
        // those, and we must not tee a whole-session writer into it).
        let want_session = records_whole && autocap.auto_capturing && ctx.is_capturing();
        match (want_session, session.is_some()) {
            (true, false) => {
                let name = detected
                    .as_ref()
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| "Game".to_string());
                // grace=true: don't defer a generic session on audio metadata
                // (mirrors the smart games' Session recording).
                if let Some(rec) = ctx.open_session("other_session", true) {
                    tracing::info!(
                        "generic: recording {mode:?} for \"{name}\" → {}",
                        rec.session_path.display()
                    );
                    session = Some(GenericActive { rec, name, mode });
                }
            }
            (false, true) => {
                if let Some(active) = session.take() {
                    finish_session(&app, active);
                }
            }
            _ => {}
        }

        // Publish the generic live-match context while our capture is active, so a
        // manual F9 save mid-session is tagged with the real game name (mirrors the
        // smart games' `update_live_match`). Cleared when our capture stops.
        if autocap.auto_capturing {
            if let Some(g) = detected.as_ref() {
                set_live_game(&app, &g.name);
                published_live = true;
            }
        } else if published_live {
            clear_live_game(&app);
            published_live = false;
        }

        ctx.emit_recorder_status();
    }
}

/// Finish a whole-session recording and save it as one library clip tagged with
/// the real game name. Runs the copy on the blocking pool (heavy IO).
fn finish_session(app: &AppHandle, active: GenericActive) {
    let GenericActive { rec, name, mode } = active;
    let Some((path, _output)) = rec.finish() else {
        return;
    };
    // Full-match keeps the whole game as "Full Match"; Session is the continuous
    // "Full Session" recording. Either way the clip is tagged with the real title.
    let event = if mode == AutoCaptureMode::FullMatch {
        "Full Match"
    } else {
        "Full Session"
    };
    let title = format!("{name} — {event}");
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let context = NewClip {
            game: Some(name),
            ..Default::default()
        };
        if let Err(e) = save_whole_session(&app, &path, &title, event, context) {
            tracing::warn!("generic: whole-session save failed: {e}");
        }
        let _ = std::fs::remove_file(&path);
    });
}

/// The generic-capture auto mode (from `games.other`). Highlights is folded to
/// Manual by [`crate::settings::Settings::other_auto_mode`].
fn current_auto_mode(app: &AppHandle) -> AutoCaptureMode {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.other_auto_mode()))
        .unwrap_or(AutoCaptureMode::Manual)
}

/// Whether the user has fully disabled generic capture (master off switch).
fn current_disabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.other.disabled))
        .unwrap_or(false)
}

/// Mirror the detected generic game into the shared [`LiveMatchState`] so a manual
/// F9 save is tagged with the real game name (no map/mode/agent for a generic
/// game).
fn set_live_game(app: &AppHandle, name: &str) {
    let Some(state) = app.try_state::<LiveMatchState>() else {
        return;
    };
    let mut g = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    *g = LiveMatch {
        in_match: true,
        map: None,
        mode: None,
        agent: None,
        agent_id: None,
        game: Some(name.to_string()),
    };
}

/// Clear the generic live-match context when our capture stops.
fn clear_live_game(app: &AppHandle) {
    let Some(state) = app.try_state::<LiveMatchState>() else {
        return;
    };
    let mut g = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    *g = LiveMatch::default();
}
