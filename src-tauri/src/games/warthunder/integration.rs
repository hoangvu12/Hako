//! War Thunder integration — a poll-fed [`GameIntegration`] in League's live-feed
//! shape (the CS2 loop, sourced from an outbound HTTP poll instead of a GSI feed).
//!
//! While War Thunder runs we poll its web-HUD server ([`super::api`]) each tick:
//! `/hudmsg` for the combat log and `/indicators` for the vehicle class. Damage
//! lines mentioning the player's nickname become Kill / Death / Crash events
//! ([`super::events`]) stamped with the capture-clock wall-clock at receipt, and
//! at battle end (an id-numbering reset, or the game closing) we reconcile them to
//! session PTS and hand the placed windows to the shared cut — exactly like CS2.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::now_ticks;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::warthunder::api::{Vehicle, WarThunderApi};
use crate::games::warthunder::detect;
use crate::games::warthunder::events::{
    classify, WarThunderContext, WarThunderEventTimings, WarThunderEventToggles,
};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
const PLACEMENT_TOL_SECS: i64 = 2;

/// The War Thunder [`GameIntegration`] (zero-sized; state lives in [`WtDriver`]).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::WarThunder
    }

    fn find_window(&self) -> Option<i64> {
        detect::find_window()
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run_live_feed(ctx, WtDriver::new()).await;
    }
}

/// In-progress War Thunder recording: the session writer plus the events
/// accumulated from the HUD poll (each stamped with the wall-clock at receipt).
struct WarThunderActive {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: WarThunderContext,
}

/// War Thunder's live-feed driver: the web-HUD client + rolling battle context,
/// plus the cached event config + nickname (needed to attribute damage lines).
struct WtDriver {
    api: Option<WarThunderApi>,
    battle_ctx: WarThunderContext,
    warned_no_nickname: bool,
    toggles: WarThunderEventToggles,
    timings: WarThunderEventTimings,
    nickname: String,
}

impl WtDriver {
    fn new() -> Self {
        WtDriver {
            api: None,
            battle_ctx: WarThunderContext::default(),
            warned_no_nickname: false,
            toggles: WarThunderEventToggles::default(),
            timings: WarThunderEventTimings::default(),
            nickname: String::new(),
        }
    }
}

#[async_trait]
impl LiveDriver for WtDriver {
    type Active = WarThunderActive;

    fn id(&self) -> GameId {
        GameId::WarThunder
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_warthunder_config(app);
        self.nickname = current_nickname(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> WarThunderActive {
        WarThunderActive {
            rec,
            events: Vec::new(),
            ctx: self.battle_ctx.clone(),
        }
    }

    fn discard(&mut self, active: WarThunderActive) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: WarThunderActive, mode: AutoCaptureMode) {
        let WarThunderActive { rec, events, ctx } = active;
        tracing::info!("warthunder: battle end — {} highlight event(s) collected", events.len());
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
                game_label: "War Thunder",
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
        active: &mut Option<WarThunderActive>,
        want: &mut Wanting,
    ) {
        // Keep the web-HUD client while the game runs; drop it (and finalize any
        // active battle) when the game exits.
        if running {
            if self.api.is_none() {
                match WarThunderApi::new() {
                    Ok(c) => self.api = Some(c),
                    Err(e) => tracing::warn!("warthunder: http client init failed: {e}"),
                }
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("warthunder: game closed mid-battle — finalizing recording");
                self.finish(app, am, mode);
            }
            self.api = None;
            self.battle_ctx = WarThunderContext::default();
            want.clear();
        }

        // Drain the HUD. Take the client out so we can `&mut self` in the loop.
        if let Some(mut c) = self.api.take() {
            let vehicle: Vehicle = c.poll_vehicle().await;
            self.battle_ctx.observe(vehicle);

            if let Ok(hud) = c.poll_damage().await {
                if hud.reset {
                    if let Some(am) = active.take() {
                        tracing::info!("warthunder: new battle detected — finalizing previous recording");
                        self.finish(app, am, mode);
                    }
                    self.battle_ctx = WarThunderContext::default();
                    self.battle_ctx.observe(vehicle);
                }

                // A blank nickname makes attribution impossible — record nothing
                // and say so once, so the user knows to fill it in.
                if self.nickname.trim().is_empty() {
                    if !self.warned_no_nickname && !hud.rows.is_empty() {
                        tracing::info!(
                            "warthunder: set your in-game nickname in settings to auto-clip kills"
                        );
                        self.warned_no_nickname = true;
                    }
                } else {
                    self.warned_no_nickname = false;
                    for row in &hud.rows {
                        let Some(kind) = classify(&row.msg, &self.nickname, vehicle) else {
                            continue;
                        };
                        if let Some(am) = active.as_mut() {
                            am.ctx = self.battle_ctx.clone();
                            if self.toggles.enabled(kind) {
                                am.events.push((kind, now_ticks()));
                                tracing::debug!("warthunder: event {}", kind.label());
                            }
                        }
                    }
                }
            }

            // War Thunder has no clean battle-*start* signal, so latch a recording
            // as soon as the game is up and the encoder is warming — a battle that
            // produces no enabled events is discarded at finalize.
            if mode.records_match() && active.is_none() {
                want.arm();
                update_live_match(app, &self.battle_ctx);
            }

            self.api = Some(c);
        }
    }
}

/// The user's War Thunder event toggles + timings (defaults when unavailable).
fn current_warthunder_config(app: &AppHandle) -> (WarThunderEventToggles, WarThunderEventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.warthunder.events, g.games.warthunder.event_timings))
        })
        .unwrap_or_default()
}

/// The user's configured in-game nickname (empty ⇒ event attribution is skipped).
fn current_nickname(app: &AppHandle) -> String {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.warthunder.nickname.clone()))
        .unwrap_or_default()
}

/// Mirror the current War Thunder context into the shared [`LiveMatchState`] so a
/// manual save mid-battle is tagged with the vehicle like an auto-clip.
fn update_live_match(app: &AppHandle, ctx: &WarThunderContext) {
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
        mode: (!ctx.vehicle.is_empty()).then(|| ctx.vehicle.clone()),
        agent: None,
        agent_id: None,
        game: Some("warthunder".to_string()),
    };
}
