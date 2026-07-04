//! Dota 2 integration — a GSI-fed [`GameIntegration`] in League's live-feed
//! shape (the CS2 loop, minus the round/spectator specifics).
//!
//! Auto-start capture on the Dota 2 window; while the game runs, host a localhost
//! GSI server ([`super::gsi`]), diff successive payloads ([`super::events`]) into
//! kill / multi-kill / death / assist events stamped with the capture-clock
//! wall-clock at receipt, and reconcile them to session PTS at match end.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::{now_ticks, TICKS_PER_SECOND};
use crate::games::dota2::detect;
use crate::games::dota2::events::{Dota2Context, Dota2EventTimings, Dota2EventToggles, Dota2Tracker};
use crate::games::dota2::gsi::Dota2Gsi;
use crate::games::dota2::payload;
use crate::games::event::EventKind;
use crate::games::recording::{
    clip_window_span, cut_placed_windows, save_whole_session, AutoCaptureState, CutWindows,
    GameCtx, RecordingSession,
};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
/// Clamp each merged window (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
const PLACEMENT_TOL_SECS: i64 = 2;

/// The Dota 2 [`GameIntegration`] (zero-sized; all state is loop-local).
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
        run(ctx).await;
    }
}

struct Dota2Active {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: Dota2Context,
}

impl Dota2Active {
    fn discard(self) {
        self.rec.discard();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<Dota2Active> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut gsi: Option<Dota2Gsi> = None;
    let mut tracker = Dota2Tracker::new();
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    let mut poll = POLL_INTERVAL;
    tracing::info!("dota2 integration started");

    loop {
        tokio::time::sleep(poll).await;

        let disabled = current_capture_disabled(&app);
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            current_auto_mode(&app)
        };
        let (toggles, timings) = current_dota2_config(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("dota2: capture mode disabled mid-match — discarding recording");
                am.discard();
            }
            want_match = false;
            want_since = None;
        }

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

        let running = ctx.game_running();
        if running {
            if gsi.is_none() {
                gsi = crate::games::dota2::gsi::start(&app);
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("dota2: game closed mid-match — finalizing recording");
                end_match(&app, am, mode, toggles, timings);
            }
            gsi = None;
            tracker = Dota2Tracker::new();
            want_match = false;
            want_since = None;
        }
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        if let Some(g) = gsi.as_ref() {
            while let Ok(body) = g.rx.try_recv() {
                let Some(p) = payload::parse_valid(&body) else {
                    continue;
                };
                let now = now_ticks();
                let res = tracker.feed(&p, now);

                if res.new_match {
                    if let Some(am) = active.take() {
                        tracing::info!("dota2: new match detected — finalizing previous recording");
                        end_match(&app, am, mode, toggles, timings);
                    }
                    if mode.records_match() {
                        want_match = true;
                        want_since.get_or_insert_with(Instant::now);
                    }
                    update_live_match(&app, tracker.context());
                }

                if let Some(am) = active.as_mut() {
                    am.ctx = tracker.context().clone();
                    for kind in res.events {
                        if toggles.enabled(kind) {
                            am.events.push((kind, now));
                            tracing::debug!("dota2: event {}", kind.label());
                        }
                    }
                }
            }
        }

        if want_match && active.is_none() && mode.records_match() {
            let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(rec) = ctx.open_session("dota2_session", grace) {
                tracing::info!("dota2: recording match → {}", rec.session_path.display());
                active = Some(Dota2Active {
                    rec,
                    events: Vec::new(),
                    ctx: tracker.context().clone(),
                });
                want_match = false;
                want_since = None;
            }
        }
    }
}

fn end_match(
    app: &AppHandle,
    am: Dota2Active,
    mode: AutoCaptureMode,
    toggles: Dota2EventToggles,
    timings: Dota2EventTimings,
) {
    let fps = am.rec.fps.max(1);
    let dota_ctx = am.ctx;
    let events = am.events;

    tracing::info!("dota2: match end — {} highlight event(s) collected", events.len());

    let Some((path, output)) = am.rec.finish() else {
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let clip_context = dota_ctx.clip_context();
        let title_suffix = dota_ctx.title_suffix();

        if mode == AutoCaptureMode::FullMatch {
            let title = if title_suffix.is_empty() {
                "Full Match".to_string()
            } else {
                format!("Full Match — {title_suffix}")
            };
            if let Err(e) = save_whole_session(&app, &path, &title, "Full Match", clip_context) {
                tracing::warn!("dota2: full-match save failed: {e}");
            }
            let _ = std::fs::remove_file(&path);
            return;
        }

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
            "dota2: placed {}/{} event(s) onto the recorded timeline",
            placed.len(),
            events.len()
        );
        if placed.is_empty() {
            tracing::info!("dota2: no enabled highlights landed in the recording");
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
                game_label: "Dota 2",
                title_suffix: &title_suffix,
                clip_context,
            },
            &placed,
            &marks,
        );
        let _ = std::fs::remove_file(&path);
    });
}

fn manage_full_session(ctx: &GameCtx, mode: AutoCaptureMode, slot: &mut Option<RecordingSession>) {
    let want = mode == AutoCaptureMode::Session && ctx.is_capturing();
    match (want, slot.is_some()) {
        (true, false) => {
            if let Some(fs) = ctx.open_session("dota2_fullsession", true) {
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

fn current_auto_mode(app: &AppHandle) -> AutoCaptureMode {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.dota2_auto_mode()))
        .unwrap_or(AutoCaptureMode::Highlights)
}

fn current_capture_disabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.games.dota2.disabled))
        .unwrap_or(false)
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
