//! League of Legends integration — the live-event-feed [`GameIntegration`].
//!
//! A single background task (spawned by the games supervisor) auto-starts capture
//! when the game window appears, then — while a match is live — polls the local
//! Live Client Data API once a second, stamping each *new* event (deduped by
//! monotonic `EventID`) with the capture-clock wall-clock at receipt. At match end
//! it reconciles those wall-clocks to session PTS via the recorded
//! [`TimelineIndex`] (exactly like Valorant, just sourced live instead of from a
//! post-match fetch) and hands the placed windows to the shared cut.
//!
//! No remote API, log parsing, round reconciliation, or pending store — the live
//! feed already gives us timestamped events directly.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::capture;
use crate::core::clock::now_ticks;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::lol::context::LolContext;
use crate::games::lol::detect;
use crate::games::lol::events::{classify, is_owned_combat, LolEventTimings, LolEventToggles};
use crate::games::lol::live_client::LiveClient;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Clamp each merged window to this many seconds.
const MAX_AUTOCLIP_SECS: i64 = 120;
/// Slack for landing an event on the recorded timeline.
const PLACEMENT_TOL_SECS: i64 = 2;

/// The League [`GameIntegration`].
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Lol
    }

    fn find_window(&self) -> Option<i64> {
        capture::find_window_by_title(detect::GAME_WINDOW_TITLE)
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        let live = match LiveClient::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("lol: could not build live-client ({e}); integration disabled");
                return;
            }
        };
        run_live_feed(ctx, LolDriver::new(live)).await;
    }
}

/// In-progress League recording: the session writer plus the events accumulated
/// from the live feed (each stamped with the capture-clock wall-clock at receipt).
struct LolActive {
    rec: RecordingSession,
    /// `(kind, wall_clock_ticks)` for each clippable event seen this match.
    events: Vec<(EventKind, i64)>,
    /// Count of owned-combat events seen (ours or not) — diagnostics for telling
    /// "no clippable moments" apart from "we failed to recognize you".
    combat_seen: u32,
    /// Highest `EventID` consumed (dedup high-water mark).
    seen_max_id: i64,
    /// Latest live context (champion / map / mode / K-D-A).
    ctx: LolContext,
    /// Match result once `GameEnd` arrives.
    won: Option<bool>,
}

/// League's live-feed driver: the Live Client Data API client, the cached event
/// config, and the seed a freshly opened session inherits (dedup high-water +
/// initial context), refreshed each tick a match is live.
struct LolDriver {
    live: LiveClient,
    toggles: LolEventToggles,
    timings: LolEventTimings,
    /// Highest `EventID` present at the last live poll — a session opened this tick
    /// starts its dedup high-water here, so events before recording aren't clipped.
    seed_seen_max_id: i64,
    /// Latest live context, seeding a session opened this tick.
    seed_ctx: LolContext,
}

impl LolDriver {
    fn new(live: LiveClient) -> Self {
        LolDriver {
            live,
            toggles: LolEventToggles::default(),
            timings: LolEventTimings::default(),
            seed_seen_max_id: -1,
            seed_ctx: LolContext::default(),
        }
    }
}

#[async_trait]
impl LiveDriver for LolDriver {
    type Active = LolActive;

    fn id(&self) -> GameId {
        GameId::Lol
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_lol_config(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> LolActive {
        LolActive {
            rec,
            events: Vec::new(),
            combat_seen: 0,
            seen_max_id: self.seed_seen_max_id,
            ctx: self.seed_ctx.clone(),
            won: None,
        }
    }

    fn discard(&mut self, active: LolActive) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: LolActive, mode: AutoCaptureMode) {
        let LolActive {
            rec,
            events,
            combat_seen,
            ctx,
            won,
            ..
        } = active;

        tracing::info!(
            "lol: match end — {} highlight event(s) collected from {} owned-combat event(s) seen; identity {} ({} name form(s))",
            events.len(),
            combat_seen,
            if ctx.me.is_empty() { "UNRESOLVED" } else { "resolved" },
            ctx.me.alias_count(),
        );
        if combat_seen > 0 && events.is_empty() {
            tracing::warn!(
                "lol: saw {combat_seen} combat event(s) but attributed none to you — \
                 identity match failed (check your in-game name forms)"
            );
        }

        let merge_after_secs = self.timings.max_after(&self.toggles);
        let title_suffix = ctx.title_suffix();
        let clip_context = ctx.clip_context(won);
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
                game_label: "League of Legends",
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
        _running: bool,
        mode: AutoCaptureMode,
        active: &mut Option<LolActive>,
        want: &mut Wanting,
    ) {
        // Poll the live feed. Ok ⇒ a match is running; Err ⇒ not in a game.
        // (League drives cadence off the feed rather than window presence; the
        // engine's running-based cadence is equivalent once a match is live.)
        let data = self.live.all_game_data().await.ok();
        match data {
            None => {
                if let Some(am) = active.take() {
                    tracing::info!("lol: match ended — finalizing recording");
                    self.finish(app, am, mode);
                }
                want.clear();
            }
            Some(data) => {
                if mode.records_match() {
                    want.arm();
                }

                // Seed a session opened *this* tick (the engine opens after drive)
                // from the events already present, so we only clip what we record.
                self.seed_seen_max_id = data
                    .events
                    .events
                    .iter()
                    .map(|e| e.event_id)
                    .max()
                    .unwrap_or(-1);
                self.seed_ctx = LolContext::from_snapshot(&data);

                // Update live context + consume new events.
                if let Some(am) = active.as_mut() {
                    am.ctx = LolContext::from_snapshot(&data);
                    // Keep the shared live-match context fresh for manual F9 clips.
                    update_live_match(app, &am.ctx);
                    for ev in &data.events.events {
                        if ev.event_id <= am.seen_max_id {
                            continue;
                        }
                        am.seen_max_id = ev.event_id;
                        if is_owned_combat(&ev.event_name) {
                            am.combat_seen += 1;
                        }
                        if ev.event_name.eq_ignore_ascii_case("GameEnd") {
                            am.won = Some(ev.result.eq_ignore_ascii_case("Win"));
                        }
                        if let Some(kind) = classify(ev, &am.ctx.me, &am.ctx.team) {
                            if self.toggles.enabled(kind) {
                                am.events.push((kind, now_ticks()));
                                tracing::debug!(
                                    "lol: event {} at {:.1}s",
                                    kind.label(),
                                    ev.event_time
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The user's League event toggles + timings (defaults when unavailable).
fn current_lol_config(app: &AppHandle) -> (LolEventToggles, LolEventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.lol.events, g.games.lol.event_timings))
        })
        .unwrap_or_default()
}

/// Mirror the current League context into the shared [`LiveMatchState`] so a
/// manual F9 save mid-match is tagged with champion/map/mode like an auto-clip.
fn update_live_match(app: &AppHandle, ctx: &LolContext) {
    use crate::valorant::live::{LiveMatch, LiveMatchState};
    let Some(state) = app.try_state::<LiveMatchState>() else {
        return;
    };
    // Bind the guard to a named local (drops before `state`) so the lock's
    // temporary doesn't outlive the borrow at the function tail.
    let mut g = match state.0.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    *g = LiveMatch {
        in_match: true,
        map: (!ctx.map.is_empty()).then(|| ctx.map.clone()),
        mode: (!ctx.mode.is_empty()).then(|| ctx.mode.clone()),
        agent: (!ctx.champion.is_empty()).then(|| ctx.champion.clone()),
        agent_id: None,
        game: Some("lol".to_string()),
    };
}
