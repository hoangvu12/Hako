//! Counter-Strike 2 integration — a GSI-fed [`GameIntegration`] in League's
//! live-feed shape.
//!
//! A single background task auto-starts capture when the CS2 window appears,
//! then — while the game runs — hosts a localhost GSI server ([`super::gsi`]).
//! CS2 POSTs its live state; we diff successive payloads ([`super::events`]) into
//! kill/headshot/multi-kill/death/assist events, stamp each with the
//! capture-clock wall-clock at receipt, and at match end reconcile those to
//! session PTS via the recorded [`TimelineIndex`] and hand the placed windows to
//! the shared cut — exactly like League, just sourced from an inbound HTTP feed
//! instead of a local API poll.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::now_ticks;
use crate::games::cs2::detect;
use crate::games::cs2::events::{Cs2Context, Cs2EventTimings, Cs2EventToggles, Cs2Tracker};
use crate::games::cs2::gsi::Cs2Gsi;
use crate::games::cs2::payload;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Clamp each merged window to this many seconds (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
/// Slack for landing an event on the recorded timeline.
const PLACEMENT_TOL_SECS: i64 = 2;

/// The CS2 [`GameIntegration`] (zero-sized; all state lives in [`Cs2Driver`]).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Cs2
    }

    fn find_window(&self) -> Option<i64> {
        detect::find_window()
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run_live_feed(ctx, Cs2Driver::new()).await;
    }
}

/// In-progress CS2 recording: the session writer plus the events accumulated
/// from the GSI feed (each stamped with the capture-clock wall-clock at receipt).
struct Cs2Active {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: Cs2Context,
}

/// CS2's live-feed driver: the hosted GSI server + rolling diff tracker, plus the
/// user's cached event config.
struct Cs2Driver {
    gsi: Option<Cs2Gsi>,
    tracker: Cs2Tracker,
    toggles: Cs2EventToggles,
    timings: Cs2EventTimings,
}

impl Cs2Driver {
    fn new() -> Self {
        Cs2Driver {
            gsi: None,
            tracker: Cs2Tracker::new(),
            toggles: Cs2EventToggles::default(),
            timings: Cs2EventTimings::default(),
        }
    }
}

#[async_trait]
impl LiveDriver for Cs2Driver {
    type Active = Cs2Active;

    fn id(&self) -> GameId {
        GameId::Cs2
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_cs2_config(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> Cs2Active {
        Cs2Active {
            rec,
            events: Vec::new(),
            ctx: self.tracker.context().clone(),
        }
    }

    fn discard(&mut self, active: Cs2Active) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: Cs2Active, mode: AutoCaptureMode) {
        let Cs2Active { rec, events, ctx } = active;
        tracing::info!("cs2: match end — {} highlight event(s) collected", events.len());
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
                game_label: "Counter-Strike 2",
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
        active: &mut Option<Cs2Active>,
        want: &mut Wanting,
    ) {
        // Host the GSI server while the game runs; drop it (and finalize any
        // active match) when the game exits.
        if running {
            if self.gsi.is_none() {
                self.gsi = crate::games::cs2::gsi::start(app);
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("cs2: game closed mid-match — finalizing recording");
                self.finish(app, am, mode);
            }
            self.gsi = None;
            self.tracker = Cs2Tracker::new();
            want.clear();
        }

        // Drain queued GSI payloads: diff into events, react to match lifecycle.
        // Take the handle out so we can `&mut self` (tracker/finish) in the loop.
        if let Some(g) = self.gsi.take() {
            while let Ok(body) = g.rx.try_recv() {
                let Some(p) = payload::parse_valid(&body) else {
                    continue;
                };
                let res = self.tracker.feed(&p);

                if res.new_match {
                    if let Some(am) = active.take() {
                        tracing::info!("cs2: new match detected — finalizing previous recording");
                        self.finish(app, am, mode);
                    }
                    if mode.records_match() {
                        want.arm();
                    }
                    update_live_match(app, self.tracker.context());
                }

                if let Some(am) = active.as_mut() {
                    am.ctx = self.tracker.context().clone();
                    for kind in res.events {
                        if self.toggles.enabled(kind) {
                            am.events.push((kind, now_ticks()));
                            tracing::debug!("cs2: event {}", kind.label());
                        }
                    }
                }

                if res.game_over {
                    if let Some(am) = active.take() {
                        tracing::info!("cs2: match over — finalizing recording");
                        self.finish(app, am, mode);
                    }
                    want.clear();
                }
            }
            self.gsi = Some(g);
        }
    }
}

/// The user's CS2 event toggles + timings (defaults when unavailable).
fn current_cs2_config(app: &AppHandle) -> (Cs2EventToggles, Cs2EventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.cs2.events, g.games.cs2.event_timings))
        })
        .unwrap_or_default()
}

/// Mirror the current CS2 context into the shared [`LiveMatchState`] so a manual
/// F9 save mid-match is tagged with map/mode like an auto-clip.
fn update_live_match(app: &AppHandle, ctx: &Cs2Context) {
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
        game: Some("cs2".to_string()),
    };
}
