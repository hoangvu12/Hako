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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::{now_ticks, TICKS_PER_SECOND};
use crate::games::event::EventKind;
use crate::games::recording::{
    clip_window_span, cut_placed_windows, finish_full_session, game_auto_mode,
    game_capture_disabled, manage_full_session, save_whole_session, AutoCaptureState, CutWindows,
    GameCtx, RecordingSession,
};
use crate::games::warthunder::api::{Vehicle, WarThunderApi};
use crate::games::warthunder::detect;
use crate::games::warthunder::events::{
    classify, WarThunderContext, WarThunderEventTimings, WarThunderEventToggles,
};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
const PLACEMENT_TOL_SECS: i64 = 2;

/// The War Thunder [`GameIntegration`] (zero-sized; all state is loop-local).
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
        run(ctx).await;
    }
}

/// In-progress War Thunder recording: the session writer plus the events
/// accumulated from the HUD poll (each stamped with the wall-clock at receipt).
struct WarThunderActive {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: WarThunderContext,
}

impl WarThunderActive {
    fn discard(self) {
        self.rec.discard();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<WarThunderActive> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut api: Option<WarThunderApi> = None;
    let mut battle_ctx = WarThunderContext::default();
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    let mut warned_no_nickname = false;
    let mut poll = POLL_INTERVAL;
    tracing::info!("warthunder integration started");

    loop {
        tokio::time::sleep(poll).await;

        let disabled = game_capture_disabled(&app, ctx.id());
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            game_auto_mode(&app, ctx.id())
        };
        let (toggles, timings) = current_warthunder_config(&app);
        let nickname = current_nickname(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-battle → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("warthunder: capture mode disabled mid-battle — discarding recording");
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

        // Poll the web HUD while the game runs; drop the client (and finalize any
        // active battle) when the game exits.
        let running = ctx.game_running();
        if running {
            if api.is_none() {
                match WarThunderApi::new() {
                    Ok(c) => api = Some(c),
                    Err(e) => tracing::warn!("warthunder: http client init failed: {e}"),
                }
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("warthunder: game closed mid-battle — finalizing recording");
                end_match(&app, am, mode, toggles, timings);
            }
            api = None;
            battle_ctx = WarThunderContext::default();
            want_match = false;
            want_since = None;
        }
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        // Drain the HUD: classify new damage lines, react to the battle boundary.
        if let Some(c) = api.as_mut() {
            let vehicle: Vehicle = c.poll_vehicle().await;
            battle_ctx.observe(vehicle);

            if let Ok(hud) = c.poll_damage().await {
                if hud.reset {
                    if let Some(am) = active.take() {
                        tracing::info!("warthunder: new battle detected — finalizing previous recording");
                        end_match(&app, am, mode, toggles, timings);
                    }
                    battle_ctx = WarThunderContext::default();
                    battle_ctx.observe(vehicle);
                }

                // A blank nickname makes attribution impossible — record nothing
                // and say so once, so the user knows to fill it in.
                if nickname.trim().is_empty() {
                    if !warned_no_nickname && !hud.rows.is_empty() {
                        tracing::info!(
                            "warthunder: set your in-game nickname in settings to auto-clip kills"
                        );
                        warned_no_nickname = true;
                    }
                } else {
                    warned_no_nickname = false;
                    for row in &hud.rows {
                        let Some(kind) = classify(&row.msg, &nickname, vehicle) else {
                            continue;
                        };
                        if let Some(am) = active.as_mut() {
                            am.ctx = battle_ctx.clone();
                            if toggles.enabled(kind) {
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
                want_match = true;
                want_since.get_or_insert_with(Instant::now);
                update_live_match(&app, &battle_ctx);
            }
        }

        // Open the session once a battle has latched + the encoder is warm.
        if want_match && active.is_none() && mode.records_match() {
            let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(rec) = ctx.open_session("warthunder_session", grace) {
                tracing::info!("warthunder: recording battle → {}", rec.session_path.display());
                active = Some(WarThunderActive {
                    rec,
                    events: Vec::new(),
                    ctx: battle_ctx.clone(),
                });
                want_match = false;
                want_since = None;
            }
        }
    }
}

/// Finish the session and (on its own blocking task) reconcile + cut the
/// highlights, or save the whole battle (FullMatch mode).
fn end_match(
    app: &AppHandle,
    am: WarThunderActive,
    mode: AutoCaptureMode,
    toggles: WarThunderEventToggles,
    timings: WarThunderEventTimings,
) {
    let fps = am.rec.fps.max(1);
    let wt_ctx = am.ctx;
    let events = am.events;

    tracing::info!("warthunder: battle end — {} highlight event(s) collected", events.len());

    let Some((path, output)) = am.rec.finish() else {
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let clip_context = wt_ctx.clip_context();
        let title_suffix = wt_ctx.title_suffix();

        if mode == AutoCaptureMode::FullMatch {
            let title = if title_suffix.is_empty() {
                "Full Match".to_string()
            } else {
                format!("Full Match — {title_suffix}")
            };
            if let Err(e) = save_whole_session(&app, &path, &title, "Full Match", clip_context) {
                tracing::warn!("warthunder: full-match save failed: {e}");
            }
            let _ = std::fs::remove_file(&path);
            return;
        }

        // Highlights: reconcile each event's receipt wall-clock to a session PTS.
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
            "warthunder: placed {}/{} event(s) onto the recorded timeline",
            placed.len(),
            events.len()
        );
        if placed.is_empty() {
            tracing::info!("warthunder: no enabled highlights landed in the recording");
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
                game_label: "War Thunder",
                title_suffix: &title_suffix,
                clip_context,
            },
            &placed,
            &marks,
        );
        let _ = std::fs::remove_file(&path);
    });
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
