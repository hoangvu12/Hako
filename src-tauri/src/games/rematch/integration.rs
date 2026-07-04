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

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::rematch::context::RematchContext;
use crate::games::rematch::detect;
use crate::games::rematch::events::{RematchEventTimings, RematchEventToggles};
use crate::games::rematch::log_watch;
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;
use crate::valorant::log_watch::{line_event_ticks, LogTail};

/// Clamp each merged window to this many seconds.
const MAX_AUTOCLIP_SECS: i64 = 120;
/// Slack for landing an event on the recorded timeline.
const PLACEMENT_TOL_SECS: i64 = 2;

/// The Rematch [`GameIntegration`] (zero-sized; state lives in [`RematchDriver`]).
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
        run_live_feed(ctx, RematchDriver::new()).await;
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

/// Rematch's live-feed driver: the `Runtime.log` tail + the match context that
/// accumulates across the whole game session, plus the cached event config.
struct RematchDriver {
    tail: Option<LogTail>,
    /// Accumulates across the whole game session (player name is set once at
    /// sign-in; mode/stadium update per match) and seeds each match.
    ctx_acc: RematchContext,
    toggles: RematchEventToggles,
    timings: RematchEventTimings,
}

impl RematchDriver {
    fn new() -> Self {
        RematchDriver {
            tail: None,
            ctx_acc: RematchContext::default(),
            toggles: RematchEventToggles::default(),
            timings: RematchEventTimings::default(),
        }
    }
}

#[async_trait]
impl LiveDriver for RematchDriver {
    type Active = RematchActive;

    fn id(&self) -> GameId {
        GameId::Rematch
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_rematch_config(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> RematchActive {
        RematchActive {
            rec,
            events: Vec::new(),
            goals_seen: 0,
            ctx: self.ctx_acc.clone(),
        }
    }

    fn discard(&mut self, active: RematchActive) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: RematchActive, mode: AutoCaptureMode) {
        let RematchActive {
            rec,
            events,
            goals_seen,
            ctx,
        } = active;
        tracing::info!(
            "rematch: match end — {} goal highlight(s) from {} goal(s) seen",
            events.len(),
            goals_seen
        );
        let merge_after_secs = self.timings.max_after(&self.toggles);
        let title_suffix = ctx.title_suffix();
        let clip_context = ctx.clip_context();
        let timings = self.timings;
        finish_and_cut(
            app,
            rec,
            MatchCut {
                events,
                mode,
                max_clip_secs: MAX_AUTOCLIP_SECS,
                placement_tol_secs: PLACEMENT_TOL_SECS,
                merge_after_secs,
                game_label: "Rematch",
                title_suffix,
                clip_context,
            },
            move |kind| {
                let t = timings.for_kind(kind);
                (t.before, t.after)
            },
        );
    }

    async fn drive(
        &mut self,
        app: &AppHandle,
        running: bool,
        mode: AutoCaptureMode,
        active: &mut Option<RematchActive>,
        want: &mut Wanting,
    ) {
        // Keep a tail open while the game runs; drop it (and finalize any active
        // match) when the game exits.
        if running {
            if self.tail.is_none() {
                self.tail = open_tail();
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("rematch: game closed mid-match — finalizing recording");
                self.finish(app, am, mode);
            }
            self.tail = None;
            want.clear();
        }

        // Drain new log lines. Take the tail out so we can `&mut self` in the loop.
        if let Some(mut t) = self.tail.take() {
            let lines = match t.poll_new_lines() {
                Ok(lines) => lines,
                Err(e) => {
                    tracing::debug!("rematch: log tail read: {e}");
                    Vec::new()
                }
            };
            for line in lines {
                update_context(&mut self.ctx_acc, &line);

                if log_watch::is_match_start(&line) {
                    if mode.records_match() {
                        // Seed the next recording with the latest known context.
                        if let Some(am) = active.as_mut() {
                            am.ctx = self.ctx_acc.clone();
                        } else {
                            want.arm();
                        }
                        update_live_match(app, &self.ctx_acc);
                    }
                    continue;
                }

                if log_watch::is_match_end(&line) {
                    want.clear();
                    if let Some(am) = active.take() {
                        tracing::info!("rematch: match ended — finalizing recording");
                        self.finish(app, am, mode);
                    }
                    continue;
                }

                if log_watch::is_goal_scored(&line) {
                    if let Some(am) = active.as_mut() {
                        am.goals_seen += 1;
                        if self.toggles.enabled(EventKind::Goal) {
                            am.events.push((EventKind::Goal, line_event_ticks(&line)));
                            tracing::debug!("rematch: goal scored");
                        }
                    }
                }
            }
            self.tail = Some(t);
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
