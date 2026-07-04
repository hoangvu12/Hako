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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::{now_ticks, TICKS_PER_SECOND};
use crate::games::event::EventKind;
use crate::games::pubg::detect;
use crate::games::pubg::events::{PubgEventTimings, PubgEventToggles};
use crate::games::pubg::parse;
use crate::games::pubg::watch;
use crate::games::recording::{
    clip_window_span, cut_placed_windows, finish_full_session, game_auto_mode,
    game_capture_disabled, manage_full_session, save_whole_session, AutoCaptureState, CutWindows,
    GameCtx, RecordingSession,
};
use crate::games::timeline::TICKS_PER_MS;
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
/// Reconciling replay times onto the capture clock relies on two free-running
/// clocks (Unix time and QPC) staying in step over a match; a slightly wider slack
/// than the live-feed games absorbs any drift.
const PLACEMENT_TOL_SECS: i64 = 3;

/// The PUBG [`GameIntegration`] (zero-sized; all state is loop-local).
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
        run(ctx).await;
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

/// In-progress PUBG recording: the session writer plus the `(Unix ms, capture
/// tick)` anchor captured when it opened.
struct PubgActive {
    rec: RecordingSession,
    anchor_unix_ms: i64,
    anchor_ticks: i64,
    ctx: PubgContext,
}

impl PubgActive {
    fn discard(self) {
        self.rec.discard();
    }
}

/// Current wall-clock in Unix milliseconds (0 if the system clock predates the
/// epoch, which never happens in practice).
fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<PubgActive> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut processed: HashSet<PathBuf> = HashSet::new();
    let mut seeded = false;
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    let mut poll = POLL_INTERVAL;
    tracing::info!("pubg integration started");

    loop {
        tokio::time::sleep(poll).await;

        let disabled = game_capture_disabled(&app, ctx.id());
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            game_auto_mode(&app, ctx.id())
        };
        let (toggles, timings) = current_pubg_config(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("pubg: capture mode disabled mid-match — discarding recording");
                am.discard();
            }
            want_match = false;
            want_since = None;
        }

        // Restart-class settings change mid-session → clean split (no demo events).
        if ctx.take_config_restart() {
            let mut resume = false;
            if let Some(am) = active.take() {
                end_match(&app, am, Vec::new(), mode, toggles, timings);
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

        let running = ctx.game_running();
        if running {
            // On the first tick of a play session, mark every *already finalized*
            // replay as handled so a match from a previous session isn't clipped
            // onto this fresh recording. In-progress replays (not yet finalized)
            // stay unhandled and are picked up when they finalize.
            if !seeded {
                for dir in watch::demo_dirs() {
                    if parse::parse_demo(&dir).is_some() {
                        processed.insert(dir);
                    }
                }
                seeded = true;
            }
            // A match's replay just finalized ⇒ its match ended: finalize the
            // current recording with the replay's events, then re-arm for the next.
            if let Some((dir, demo)) = next_finished_demo(&processed) {
                processed.insert(dir);
                let events: Vec<(EventKind, i64)> = demo
                    .events
                    .iter()
                    .map(|e| (e.kind, e.unix_ms))
                    .collect();
                if let Some(mut am) = active.take() {
                    am.ctx.user = demo.user.clone();
                    update_live_match(&app, &am.ctx);
                    tracing::info!(
                        "pubg: match replay finalized ({} event(s)) — cutting highlights",
                        events.len()
                    );
                    end_match(&app, am, events, mode, toggles, timings);
                } else {
                    tracing::info!("pubg: match replay finalized but no active recording — skipping");
                }
                if mode.records_match() {
                    want_match = true;
                    want_since = Some(Instant::now());
                }
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("pubg: game closed — finalizing recording");
                end_match(&app, am, Vec::new(), mode, toggles, timings);
            }
            seeded = false;
            want_match = false;
            want_since = None;
        }
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        // With no live match-start signal, latch a recording as soon as the game
        // is up and the encoder is warming; a match that yields no enabled events
        // is discarded at finalize.
        if running && mode.records_match() && active.is_none() {
            want_match = true;
            want_since.get_or_insert_with(Instant::now);
        }

        // Open the session once latched + the encoder is warm, sampling the
        // Unix↔capture-clock anchor at the same instant.
        if want_match && active.is_none() && mode.records_match() {
            let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(rec) = ctx.open_session("pubg_session", grace) {
                tracing::info!("pubg: recording match → {}", rec.session_path.display());
                active = Some(PubgActive {
                    rec,
                    anchor_unix_ms: now_unix_ms(),
                    anchor_ticks: now_ticks(),
                    ctx: PubgContext::default(),
                });
                want_match = false;
                want_since = None;
            }
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

/// Finish the session and (on its own blocking task) reconcile the replay events
/// onto the recorded timeline + cut the highlights, or save the whole match
/// (FullMatch mode). `events` are `(kind, Unix ms)`; empty when finalizing without
/// a replay (game close / config restart).
fn end_match(
    app: &AppHandle,
    am: PubgActive,
    events: Vec<(EventKind, i64)>,
    mode: AutoCaptureMode,
    toggles: PubgEventToggles,
    timings: PubgEventTimings,
) {
    let fps = am.rec.fps.max(1);
    let pubg_ctx = am.ctx;
    let anchor_unix_ms = am.anchor_unix_ms;
    let anchor_ticks = am.anchor_ticks;

    tracing::info!("pubg: match end — {} highlight event(s) collected", events.len());

    let Some((path, output)) = am.rec.finish() else {
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let clip_context = pubg_ctx.clip_context();
        let title_suffix = String::new();

        if mode == AutoCaptureMode::FullMatch {
            if let Err(e) = save_whole_session(&app, &path, "Full Match", "Full Match", clip_context) {
                tracing::warn!("pubg: full-match save failed: {e}");
            }
            let _ = std::fs::remove_file(&path);
            return;
        }

        // Highlights: map each replay Unix-ms time onto the capture clock via the
        // session anchor, then reconcile to a session PTS.
        let tol = PLACEMENT_TOL_SECS * TICKS_PER_SECOND;
        let mut placed: Vec<(i64, i64, EventKind)> = Vec::new();
        let mut marks: Vec<(i64, EventKind)> = Vec::new();
        let max_len_pts = MAX_AUTOCLIP_SECS * fps as i64;
        for (kind, unix_ms) in &events {
            if !toggles.enabled(*kind) {
                continue;
            }
            let wall = anchor_ticks + (unix_ms - anchor_unix_ms) * TICKS_PER_MS;
            let Some(pts) = timeline.pts_at_within(wall, tol) else {
                continue;
            };
            let t = timings.for_kind(*kind);
            let (s, end) = clip_window_span(pts, pts, t.before, t.after, fps);
            let end = end.min(s + max_len_pts);
            placed.push((s, end, *kind));
            marks.push((pts, *kind));
        }
        tracing::info!(
            "pubg: placed {}/{} event(s) onto the recorded timeline",
            placed.len(),
            events.len()
        );
        if placed.is_empty() {
            tracing::info!("pubg: no enabled highlights landed in the recording");
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
                game_label: "PUBG",
                title_suffix: &title_suffix,
                clip_context,
            },
            &placed,
            &marks,
        );
        let _ = std::fs::remove_file(&path);
    });
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
