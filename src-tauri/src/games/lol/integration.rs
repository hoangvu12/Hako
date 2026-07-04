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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::capture;
use crate::core::clock::now_ticks;
use crate::games::event::EventKind;
use crate::games::lol::context::LolContext;
use crate::games::lol::detect;
use crate::games::lol::events::{classify, is_owned_combat, LolEventTimings, LolEventToggles};
use crate::games::lol::live_client::LiveClient;
use crate::games::recording::{
    finish_and_cut, finish_full_session, game_auto_mode, game_capture_disabled,
    manage_full_session, AutoCaptureState, GameCtx, MatchCut, RecordingSession,
};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Live-feed poll cadence while League's in-game process is running (the feed
/// updates sub-second; 1 s is plenty and clip padding absorbs the jitter).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Relaxed cadence while the in-game process isn't running — no match can be
/// starting, so poll (and hit the shared process table) far less often. Tightens
/// back to [`POLL_INTERVAL`] the first tick the process is seen, well before the
/// in-game window appears, so auto-capture latency is unaffected.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace for audio-track metadata before opening the session writer.
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
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
        run(ctx).await;
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

impl LolActive {
    fn discard(self) {
        self.rec.discard();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let live = match LiveClient::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("lol: could not build live-client ({e}); integration disabled");
            return;
        }
    };
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<LolActive> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    // Idle back-off: start fast so first detection is prompt, then relax whenever
    // the in-game process isn't running (set at each loop tail).
    let mut poll = POLL_INTERVAL;
    tracing::info!("league integration started");

    loop {
        tokio::time::sleep(poll).await;

        // "Disabled" fully ignores League: no buffer auto-attach, and forcing
        // Manual below tears down any in-flight auto-recording via the paths that
        // already handle a mid-match mode change.
        let disabled = game_capture_disabled(&app, ctx.id());
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            game_auto_mode(&app, ctx.id())
        };
        let (toggles, timings) = current_lol_config(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("lol: capture mode disabled mid-match — discarding recording");
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

        // Poll the live feed. Ok ⇒ a match is running; Err ⇒ not in a game.
        let data = live.all_game_data().await.ok();
        match data {
            None => {
                if let Some(am) = active.take() {
                    tracing::info!("lol: match ended — finalizing recording");
                    end_match(&app, am, mode, toggles, timings);
                }
                want_match = false;
                want_since = None;
                // No live match — relax the cadence until one starts. Capture still
                // auto-starts within one idle tick of the in-game window appearing,
                // during the loading screen (no gameplay missed).
                poll = IDLE_POLL_INTERVAL;
            }
            Some(data) => {
                // A match is live — poll at the fast cadence for event latency.
                poll = POLL_INTERVAL;
                if mode.records_match() {
                    want_match = true;
                    want_since.get_or_insert_with(Instant::now);
                }

                // Open the session once latched + the encoder is warm. Seed the
                // dedup high-water from the events already present so we only clip
                // what we actually record (events before recording started can't be
                // clipped).
                if want_match && active.is_none() && mode.records_match() {
                    let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
                    if let Some(rec) = ctx.open_session("lol_session", grace) {
                        let seen_max_id = data
                            .events
                            .events
                            .iter()
                            .map(|e| e.event_id)
                            .max()
                            .unwrap_or(-1);
                        tracing::info!("lol: recording match → {}", rec.session_path.display());
                        active = Some(LolActive {
                            rec,
                            events: Vec::new(),
                            combat_seen: 0,
                            seen_max_id,
                            ctx: LolContext::from_snapshot(&data),
                            won: None,
                        });
                        want_match = false;
                        want_since = None;
                    }
                }

                // Update live context + consume new events.
                if let Some(am) = active.as_mut() {
                    am.ctx = LolContext::from_snapshot(&data);
                    // Keep the shared live-match context fresh for manual F9 clips.
                    update_live_match(&app, &am.ctx);
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
                            if toggles.enabled(kind) {
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

/// Finish the session and (on its own blocking task) reconcile + cut the
/// highlights, or save the whole match (FullMatch mode).
fn end_match(
    app: &AppHandle,
    am: LolActive,
    mode: AutoCaptureMode,
    toggles: LolEventToggles,
    timings: LolEventTimings,
) {
    let LolActive {
        rec,
        events,
        combat_seen,
        ctx,
        won,
        ..
    } = am;

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

    let merge_after_secs = timings.max_after(&toggles);
    let title_suffix = ctx.title_suffix();
    let clip_context = ctx.clip_context(won);
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
