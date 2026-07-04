//! Dota 2 integration — a GSI-fed [`GameIntegration`] in League's live-feed
//! shape (the CS2 loop, minus the round/spectator specifics).
//!
//! Auto-start capture on the Dota 2 window; while the game runs, host a localhost
//! GSI server ([`super::gsi`]), diff successive payloads ([`super::events`]) into
//! kill / multi-kill / death / assist events stamped with the capture-clock
//! wall-clock at receipt, and reconcile them to session PTS at match end.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::now_ticks;
use crate::games::dota2::detect;
use crate::games::dota2::events::{Dota2Context, Dota2EventTimings, Dota2EventToggles, Dota2Tracker};
use crate::games::dota2::gsi::Dota2Gsi;
use crate::games::dota2::payload;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
const PLACEMENT_TOL_SECS: i64 = 2;

/// The Dota 2 [`GameIntegration`] (zero-sized; all state lives in [`Dota2Driver`]).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Dota2
    }

    fn find_window(&self) -> Option<i64> {
        detect::find_window()
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run_live_feed(ctx, Dota2Driver::new()).await;
    }
}

struct Dota2Active {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: Dota2Context,
}

/// Dota 2's live-feed driver: the hosted GSI server + rolling diff tracker, plus
/// the user's cached event config.
struct Dota2Driver {
    gsi: Option<Dota2Gsi>,
    tracker: Dota2Tracker,
    toggles: Dota2EventToggles,
    timings: Dota2EventTimings,
}

impl Dota2Driver {
    fn new() -> Self {
        Dota2Driver {
            gsi: None,
            tracker: Dota2Tracker::new(),
            toggles: Dota2EventToggles::default(),
            timings: Dota2EventTimings::default(),
        }
    }
}

#[async_trait]
impl LiveDriver for Dota2Driver {
    type Active = Dota2Active;

    fn id(&self) -> GameId {
        GameId::Dota2
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_dota2_config(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> Dota2Active {
        Dota2Active {
            rec,
            events: Vec::new(),
            ctx: self.tracker.context().clone(),
        }
    }

    fn discard(&mut self, active: Dota2Active) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: Dota2Active, mode: AutoCaptureMode) {
        let Dota2Active { rec, events, ctx } = active;
        tracing::info!("dota2: match end — {} highlight event(s) collected", events.len());
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
                game_label: "Dota 2",
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
        active: &mut Option<Dota2Active>,
        want: &mut Wanting,
    ) {
        if running {
            if self.gsi.is_none() {
                self.gsi = crate::games::dota2::gsi::start(app);
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("dota2: game closed mid-match — finalizing recording");
                self.finish(app, am, mode);
            }
            self.gsi = None;
            self.tracker = Dota2Tracker::new();
            want.clear();
        }

        // Take the handle out so we can `&mut self` (tracker/finish) in the loop.
        if let Some(g) = self.gsi.take() {
            while let Ok(body) = g.rx.try_recv() {
                let Some(p) = payload::parse_valid(&body) else {
                    continue;
                };
                let now = now_ticks();
                let res = self.tracker.feed(&p, now);

                if res.new_match {
                    if let Some(am) = active.take() {
                        tracing::info!("dota2: new match detected — finalizing previous recording");
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
                            am.events.push((kind, now));
                            tracing::debug!("dota2: event {}", kind.label());
                        }
                    }
                }
            }
            self.gsi = Some(g);
        }
    }
}

fn current_dota2_config(app: &AppHandle) -> (Dota2EventToggles, Dota2EventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.dota2.events, g.games.dota2.event_timings))
        })
        .unwrap_or_default()
}

/// Mirror the current Dota 2 context into the shared [`LiveMatchState`] so a
/// manual F9 save mid-match is tagged with the hero like an auto-clip.
fn update_live_match(app: &AppHandle, ctx: &Dota2Context) {
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
        map: None,
        mode: None,
        agent: (!ctx.hero.is_empty()).then(|| ctx.hero.clone()),
        agent_id: None,
        game: Some("dota2".to_string()),
    };
}
