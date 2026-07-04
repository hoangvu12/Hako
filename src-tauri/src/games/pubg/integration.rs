//! PUBG integration — a **post-match** [`GameIntegration`] driven by replay
//! sidecars on disk (Valorant's reconcile shape, sourced from PUBG's `Demos\`
//! replays instead of a remote match API).
//!
//! PUBG has no live feed, so we record continuously while the game runs and watch
//! `%LOCALAPPDATA%\TslGame\Saved\Demos\` ([`super::watch`]) for a match's replay
//! to *finalize*. When one does, we parse its kill / knockdown / death / chicken-
//! dinner events ([`super::parse`]), map each event's replay wall-clock (Unix ms)
//! onto the capture clock via an anchor sampled at session start, reconcile those
//! to session PTS through the recorded [`TimelineIndex`], cut the highlights, and
//! re-open a fresh session for the next match.

#![allow(dead_code)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::now_ticks;
use crate::games::engine::{run_live_feed, LiveDriver, Wanting};
use crate::games::event::EventKind;
use crate::games::pubg::detect;
use crate::games::pubg::events::{PubgEventTimings, PubgEventToggles};
use crate::games::pubg::parse;
use crate::games::pubg::watch;
use crate::games::recording::{finish_and_cut, GameCtx, MatchCut, RecordingSession};
use crate::games::timeline::TICKS_PER_MS;
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
/// Reconciling replay times onto the capture clock relies on two free-running
/// clocks (Unix time and QPC) staying in step over a match; a slightly wider slack
/// than the live-feed games absorbs any drift.
const PLACEMENT_TOL_SECS: i64 = 3;

/// The PUBG [`GameIntegration`] (zero-sized; all state lives in [`PubgDriver`]).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Pubg
    }

    fn find_window(&self) -> Option<i64> {
        detect::find_window()
    }

    fn detect_running(&self) -> bool {
        detect::game_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run_live_feed(ctx, PubgDriver::new()).await;
    }
}

/// The local player + a wall-clock anchor pairing Unix time with the capture
/// clock, so replay event times (Unix ms) can be mapped onto session PTS.
#[derive(Debug, Clone, Default)]
struct PubgContext {
    /// The recording player's nickname (from `PUBG.replayinfo`), for clip tagging.
    user: String,
}

impl PubgContext {
    fn clip_context(&self) -> crate::library::db::NewClip {
        crate::library::db::NewClip {
            game: Some("pubg".to_string()),
            ..Default::default()
        }
    }
}

/// In-progress PUBG recording: the session writer, the `(Unix ms, capture tick)`
/// anchor captured when it opened, and the replay events attached at finalize
/// (empty until a demo finalizes; empty forever on game-close / config-restart).
struct PubgActive {
    rec: RecordingSession,
    anchor_unix_ms: i64,
    anchor_ticks: i64,
    ctx: PubgContext,
    /// `(kind, Unix ms)` from the finalized replay, set just before finishing.
    events: Vec<(EventKind, i64)>,
}

/// Current wall-clock in Unix milliseconds (0 if the system clock predates the
/// epoch, which never happens in practice).
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// PUBG's driver: the set of replay dirs already handled + whether we've seeded it
/// this play session, plus the cached event config. PUBG has no live feed — it
/// records continuously and finalizes when a match's replay lands on disk.
struct PubgDriver {
    processed: HashSet<PathBuf>,
    seeded: bool,
    toggles: PubgEventToggles,
    timings: PubgEventTimings,
}

impl PubgDriver {
    fn new() -> Self {
        PubgDriver {
            processed: HashSet::new(),
            seeded: false,
            toggles: PubgEventToggles::default(),
            timings: PubgEventTimings::default(),
        }
    }
}

#[async_trait]
impl LiveDriver for PubgDriver {
    type Active = PubgActive;

    fn id(&self) -> GameId {
        GameId::Pubg
    }

    fn refresh_settings(&mut self, app: &AppHandle) {
        (self.toggles, self.timings) = current_pubg_config(app);
    }

    fn begin(&mut self, rec: RecordingSession) -> PubgActive {
        // Sample the Unix↔capture-clock anchor at the same instant we open.
        PubgActive {
            rec,
            anchor_unix_ms: now_unix_ms(),
            anchor_ticks: now_ticks(),
            ctx: PubgContext::default(),
            events: Vec::new(),
        }
    }

    fn discard(&mut self, active: PubgActive) {
        active.rec.discard();
    }

    fn finish(&mut self, app: &AppHandle, active: PubgActive, mode: AutoCaptureMode) {
        let PubgActive {
            rec,
            anchor_unix_ms,
            anchor_ticks,
            ctx,
            events,
        } = active;
        tracing::info!("pubg: match end — {} highlight event(s) collected", events.len());

        // Map each replay Unix-ms time onto the capture clock via the session
        // anchor, keeping only the user's enabled kinds. Unlike the live-feed games
        // (which filter at receipt), PUBG collects every demo event and filters here.
        let events: Vec<(EventKind, i64)> = events
            .into_iter()
            .filter(|(kind, _)| self.toggles.enabled(*kind))
            .map(|(kind, unix_ms)| (kind, anchor_ticks + (unix_ms - anchor_unix_ms) * TICKS_PER_MS))
            .collect();

        let merge_after_secs = self.timings.max_after(&self.toggles);
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
                game_label: "PUBG",
                title_suffix: String::new(),
                clip_context: ctx.clip_context(),
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
        active: &mut Option<PubgActive>,
        want: &mut Wanting,
    ) {
        if running {
            // On the first tick of a play session, mark every *already finalized*
            // replay as handled so a match from a previous session isn't clipped
            // onto this fresh recording. In-progress replays (not yet finalized)
            // stay unhandled and are picked up when they finalize.
            if !self.seeded {
                for dir in watch::demo_dirs() {
                    if parse::parse_demo(&dir).is_some() {
                        self.processed.insert(dir);
                    }
                }
                self.seeded = true;
            }
            // A match's replay just finalized ⇒ its match ended: finalize the
            // current recording with the replay's events, then re-arm for the next.
            if let Some((dir, demo)) = next_finished_demo(&self.processed) {
                self.processed.insert(dir);
                let events: Vec<(EventKind, i64)> =
                    demo.events.iter().map(|e| (e.kind, e.unix_ms)).collect();
                if let Some(mut am) = active.take() {
                    am.ctx.user = demo.user.clone();
                    am.events = events;
                    update_live_match(app, &am.ctx);
                    tracing::info!(
                        "pubg: match replay finalized ({} event(s)) — cutting highlights",
                        am.events.len()
                    );
                    self.finish(app, am, mode);
                } else {
                    tracing::info!("pubg: match replay finalized but no active recording — skipping");
                }
                if mode.records_match() {
                    want.rearm();
                }
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("pubg: game closed — finalizing recording");
                self.finish(app, am, mode);
            }
            self.seeded = false;
            want.clear();
        }

        // With no live match-start signal, latch a recording as soon as the game
        // is up and the encoder is warming; a match that yields no enabled events
        // is discarded at finalize.
        if running && mode.records_match() && active.is_none() {
            want.arm();
        }
    }
}

/// The newest finalized replay directory we haven't handled yet, parsed.
fn next_finished_demo(processed: &HashSet<PathBuf>) -> Option<(PathBuf, parse::ParsedDemo)> {
    for dir in watch::demo_dirs() {
        if processed.contains(&dir) {
            continue;
        }
        if let Some(demo) = parse::parse_demo(&dir) {
            return Some((dir, demo));
        }
    }
    None
}

/// The user's PUBG event toggles + timings (defaults when unavailable).
fn current_pubg_config(app: &AppHandle) -> (PubgEventToggles, PubgEventTimings) {
    app.try_state::<SettingsState>()
        .and_then(|s| {
            s.0.lock()
                .ok()
                .map(|g| (g.games.pubg.events, g.games.pubg.event_timings))
        })
        .unwrap_or_default()
}

/// Mirror the current PUBG context into the shared [`LiveMatchState`] so a manual
/// save is tagged with the game like an auto-clip.
fn update_live_match(app: &AppHandle, _ctx: &PubgContext) {
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
        agent: None,
        agent_id: None,
        game: Some("pubg".to_string()),
    };
}
