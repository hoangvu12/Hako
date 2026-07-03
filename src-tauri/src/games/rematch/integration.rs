//! Rematch integration — a log-tailed [`GameIntegration`] in League's live-feed
//! shape.
//!
//! A single background task auto-starts capture when the Rematch window appears,
//! then tails `Runtime.log`. Each goal cue is stamped with the capture-clock
//! wall-clock at receipt (back-dated to the log line's own timestamp), and at
//! match end those wall-clocks are reconciled to session PTS via the recorded
//! [`crate::core::session`] timeline — exactly like League, just sourced from a
//! log tail instead of the Live Client API. No remote API, no memory reading.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::TICKS_PER_SECOND;
use crate::games::event::EventKind;
use crate::games::recording::{
    clip_window_span, cut_placed_windows, save_whole_session, AutoCaptureState, CutWindows,
    GameCtx, RecordingSession,
};
use crate::games::rematch::context::RematchContext;
use crate::games::rematch::detect;
use crate::games::rematch::events::{RematchEventTimings, RematchEventToggles};
use crate::games::rematch::log_watch;
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;
use crate::valorant::log_watch::{line_event_ticks, LogTail};

/// Tail poll cadence while Rematch is running. The log appends sub-second; 1 s +
/// the ±pad absorbs jitter.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Relaxed cadence while the game isn't running — nothing to tail, so poll (and
/// hit the shared process table) far less often. Tightens back the first tick the
/// process is seen, before the game window appears, so auto-capture is unaffected.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace for audio-track metadata before opening the session writer.
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
/// Clamp each merged window to this many seconds.
const MAX_AUTOCLIP_SECS: i64 = 120;
/// Slack for landing an event on the recorded timeline.
const PLACEMENT_TOL_SECS: i64 = 2;

/// The Rematch [`GameIntegration`] (zero-sized; all state is loop-local).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Rematch
    }

    fn find_window(&self) -> Option<i64> {
        detect::find_window()
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run(ctx).await;
    }
}

/// In-progress Rematch recording: the session writer plus the goal events
/// accumulated from the log (each stamped with the capture-clock at receipt).
struct RematchActive {
    rec: RecordingSession,
    /// `(kind, wall_clock_ticks)` for each clippable goal seen this match.
    events: Vec<(EventKind, i64)>,
    /// Goals seen this match (ours or not) — diagnostics for "no clippable goals".
    goals_seen: u32,
    /// Latest match context (player / mode / stadium).
    ctx: RematchContext,
}

impl RematchActive {
    fn discard(self) {
        self.rec.discard();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<RematchActive> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut tail: Option<LogTail> = None;
    // Match context accumulates across the whole game session (player name is set
    // once at sign-in; mode/stadium update per match) and seeds each match.
    let mut ctx_acc = RematchContext::default();
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    // Idle back-off: fast while the game runs, relaxed otherwise (set at the tick
    // where game presence is checked).
    let mut poll = POLL_INTERVAL;
    tracing::info!("rematch integration started");

    loop {
        tokio::time::sleep(poll).await;

        // "Disabled" fully ignores Rematch: no buffer auto-attach, and forcing
        // Manual below tears down any in-flight auto-recording via the paths that
        // already handle a mid-match mode change.
        let disabled = current_capture_disabled(&app);
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            current_auto_mode(&app)
        };
        let (toggles, timings) = current_rematch_config(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("rematch: capture mode disabled mid-match — discarding recording");
                am.discard();
            }
            want_match = false;
            want_since = None;
        }

        // Restart-class settings change mid-session → clean split.
        if ctx.take_config_restart() {
            let mut resume = false;
            if let Some(am) = active.take() {
                end_match(&app, am, mode, toggles, timings);
                resume = mode.records_match();
            }
            if let Some(fs) = full_session.take() {
                finish_full_session(&app, fs);
            }
            ctx.restart_capture();
            if resume {
                want_match = true;
                want_since = Some(Instant::now());
            }
        }

        ctx.emit_recorder_status();

        // Keep a tail open while the game runs; drop it (and finalize any active
        // match) when the game exits.
        let running = ctx.game_running();
        if running {
            if tail.is_none() {
                tail = open_tail();
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("rematch: game closed mid-match — finalizing recording");
                end_match(&app, am, mode, toggles, timings);
            }
            tail = None;
            want_match = false;
            want_since = None;
        }
        // Relax the cadence while the game isn't running.
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        // Drain new log lines: update context + react to match / goal markers.
        if let Some(t) = tail.as_mut() {
            let lines = match t.poll_new_lines() {
                Ok(lines) => lines,
                Err(e) => {
                    tracing::debug!("rematch: log tail read: {e}");
                    Vec::new()
                }
            };
            for line in lines {
                update_context(&mut ctx_acc, &line);

                if log_watch::is_match_start(&line) {
                    if mode.records_match() {
                        // Seed the next recording with the latest known context.
                        if let Some(am) = active.as_mut() {
                            am.ctx = ctx_acc.clone();
                        } else {
                            want_match = true;
                            want_since.get_or_insert_with(Instant::now);
                        }
                        update_live_match(&app, &ctx_acc);
                    }
                    continue;
                }

                if log_watch::is_match_end(&line) {
                    want_match = false;
                    want_since = None;
                    if let Some(am) = active.take() {
                        tracing::info!("rematch: match ended — finalizing recording");
                        end_match(&app, am, mode, toggles, timings);
                    }
                    continue;
                }

                if log_watch::is_goal_scored(&line) {
                    if let Some(am) = active.as_mut() {
                        am.goals_seen += 1;
                        if toggles.enabled(EventKind::Goal) {
                            am.events.push((EventKind::Goal, line_event_ticks(&line)));
                            tracing::debug!("rematch: goal scored");
                        }
                    }
                }
            }
        }

        // Open the session once a kickoff has latched + the encoder is warm.
        if want_match && active.is_none() && mode.records_match() {
            let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(rec) = ctx.open_session("rematch_session", grace) {
                tracing::info!("rematch: recording match → {}", rec.session_path.display());
                active = Some(RematchActive {
                    rec,
                    events: Vec::new(),
                    goals_seen: 0,
                    ctx: ctx_acc.clone(),
                });
                want_match = false;
                want_since = None;
            }
        }
    }
}

/// Resolve + open the log tail at end-of-file (so we only react to new lines).
fn open_tail() -> Option<LogTail> {
    let path = log_watch::log_path()?;
    match LogTail::open_at_end(path) {
        Ok(t) => {
            tracing::info!("rematch: tailing Runtime.log");
            Some(t)
        }
        Err(e) => {
            tracing::warn!("rematch: could not open Runtime.log tail: {e}");
            None
        }
    }
}

/// Fold a log line into the running match context (player / mode / stadium).
fn update_context(ctx: &mut RematchContext, line: &str) {
    if let Some(name) = log_watch::parse_player_name(line) {
        ctx.player = name;
    }
    if let Some(mode) = log_watch::parse_game_mode(line) {
        ctx.mode = mode.to_string();
    }
    if let Some(map) = log_watch::parse_map(line) {
        ctx.map = map;
    }
}

/// Finish the session and (on its own blocking task) reconcile + cut the goal
/// highlights, or save the whole match (FullMatch mode).
fn end_match(
    app: &AppHandle,
    am: RematchActive,
    mode: AutoCaptureMode,
    toggles: RematchEventToggles,
    timings: RematchEventTimings,
) {
    let fps = am.rec.fps.max(1);
    let goals_seen = am.goals_seen;
    let r_ctx = am.ctx;
    let events = am.events;

    tracing::info!(
        "rematch: match end — {} goal highlight(s) from {} goal(s) seen",
        events.len(),
        goals_seen
    );

    let Some((path, output)) = am.rec.finish() else {
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let clip_context = r_ctx.clip_context();
        let title_suffix = r_ctx.title_suffix();

        if mode == AutoCaptureMode::FullMatch {
            let title = if title_suffix.is_empty() {
                "Full Match".to_string()
            } else {
                format!("Full Match — {title_suffix}")
            };
            if let Err(e) = save_whole_session(&app, &path, &title, "Full Match", clip_context) {
                tracing::warn!("rematch: full-match save failed: {e}");
            }
            let _ = std::fs::remove_file(&path);
            return;
        }

        // Highlights: reconcile each goal's receipt wall-clock to a session PTS.
        let tol = PLACEMENT_TOL_SECS * TICKS_PER_SECOND;
        let mut placed: Vec<(i64, i64, EventKind)> = Vec::new();
        let mut marks: Vec<(i64, EventKind)> = Vec::new();
        let max_len_pts = MAX_AUTOCLIP_SECS * fps as i64;
        for (kind, wall) in &events {
            let Some(pts) = timeline.pts_at_within(*wall, tol) else {
                continue;
            };
            let t = timings.for_kind(*kind);
            let (s, end) = clip_window_span(pts, pts, t.before, t.after, fps);
            let end = end.min(s + max_len_pts);
            placed.push((s, end, *kind));
            marks.push((pts, *kind));
        }
        tracing::info!(
            "rematch: placed {}/{} goal(s) onto the recorded timeline",
            placed.len(),
            events.len()
        );
        if placed.is_empty() {
            tracing::info!("rematch: no goal highlights landed in the recording");
            let _ = std::fs::remove_file(&path);
            return;
        }
        cut_placed_windows(
            &CutWindows {
                app: &app,
                session_path: &path,
                frozen_spans: &frozen_spans,
                fps,
                max_clip_secs: MAX_AUTOCLIP_SECS,
                merge_after_secs: timings.max_after(&toggles),
                game_label: "Rematch",
                title_suffix: &title_suffix,
                clip_context,
            },
            &placed,
            &marks,
        );
        let _ = std::fs::remove_file(&path);
    });
}

/// Session-mode continuous recording (one clip while capture is live).
fn manage_full_session(ctx: &GameCtx, mode: AutoCaptureMode, slot: &mut Option<RecordingSession>) {
    let want = mode == AutoCaptureMode::Session && ctx.is_capturing();
    match (want, slot.is_some()) {
        (true, false) => {
            if let Some(fs) = ctx.open_session("rematch_fullsession", true) {
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

fn finish_full_session(app: &AppHandle, fs: RecordingSession) {
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

/// The user's configured auto-capture mode for Rematch (per-game settings).
fn current_auto_mode(app: &AppHandle) -> AutoCaptureMode {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.rematch_auto_mode()))
        .unwrap_or(AutoCaptureMode::Highlights)
}

/// Whether the user has fully disabled Hako for Rematch ("don't capture this
/// game at all"). Defaults to enabled when settings are unavailable.
fn current_capture_disabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.rematch.disabled))
        .unwrap_or(false)
}

/// The user's Rematch event toggles + timings (defaults when unavailable).
fn current_rematch_config(app: &AppHandle) -> (RematchEventToggles, RematchEventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.rematch.events, g.games.rematch.event_timings))
        })
        .unwrap_or_default()
}

/// Mirror the current Rematch context into the shared [`LiveMatchState`] so a
/// manual F9 save mid-match is tagged with stadium/mode like an auto-clip.
fn update_live_match(app: &AppHandle, ctx: &RematchContext) {
    use crate::valorant::live::{LiveMatch, LiveMatchState};
    let Some(state) = app.try_state::<LiveMatchState>() else {
        return;
    };
    let mut g = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    *g = LiveMatch {
        in_match: true,
        map: (!ctx.map.is_empty()).then(|| ctx.map.clone()),
        mode: (!ctx.mode.is_empty()).then(|| ctx.mode.clone()),
        agent: None,
        agent_id: None,
        game: Some("rematch".to_string()),
    };
}
