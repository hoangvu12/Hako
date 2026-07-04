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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::{AppHandle, Manager};

use crate::commands::SettingsState;
use crate::core::clock::{now_ticks, TICKS_PER_SECOND};
use crate::games::cs2::detect;
use crate::games::cs2::events::{Cs2Context, Cs2EventTimings, Cs2EventToggles, Cs2Tracker};
use crate::games::cs2::gsi::Cs2Gsi;
use crate::games::cs2::payload;
use crate::games::event::EventKind;
use crate::games::recording::{
    clip_window_span, cut_placed_windows, game_auto_mode, game_capture_disabled,
    save_whole_session, AutoCaptureState, CutWindows, GameCtx, RecordingSession,
};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;

/// Loop cadence while CS2 is running (GSI payloads queue in the channel between
/// ticks; 1 s keeps event latency well under the clip padding).
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Relaxed cadence while CS2 isn't running.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace for audio-track metadata before opening the session writer.
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);
/// Clamp each merged window to this many seconds (Medal's MaxAutoClipLength 5m).
const MAX_AUTOCLIP_SECS: i64 = 300;
/// Slack for landing an event on the recorded timeline.
const PLACEMENT_TOL_SECS: i64 = 2;

/// The CS2 [`GameIntegration`] (zero-sized; all state is loop-local).
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
        run(ctx).await;
    }
}

/// In-progress CS2 recording: the session writer plus the events accumulated
/// from the GSI feed (each stamped with the capture-clock wall-clock at receipt).
struct Cs2Active {
    rec: RecordingSession,
    events: Vec<(EventKind, i64)>,
    ctx: Cs2Context,
}

impl Cs2Active {
    fn discard(self) {
        self.rec.discard();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<Cs2Active> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut gsi: Option<Cs2Gsi> = None;
    let mut tracker = Cs2Tracker::new();
    let mut want_match = false;
    let mut want_since: Option<Instant> = None;
    let mut poll = POLL_INTERVAL;
    tracing::info!("cs2 integration started");

    loop {
        tokio::time::sleep(poll).await;

        let disabled = game_capture_disabled(&app, ctx.id());
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            game_auto_mode(&app, ctx.id())
        };
        let (toggles, timings) = current_cs2_config(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("cs2: capture mode disabled mid-match — discarding recording");
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

        // Host the GSI server while the game runs; drop it (and finalize any
        // active match) when the game exits.
        let running = ctx.game_running();
        if running {
            if gsi.is_none() {
                gsi = crate::games::cs2::gsi::start(&app);
            }
        } else {
            if let Some(am) = active.take() {
                tracing::info!("cs2: game closed mid-match — finalizing recording");
                end_match(&app, am, mode, toggles, timings);
            }
            gsi = None;
            tracker = Cs2Tracker::new();
            want_match = false;
            want_since = None;
        }
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        // Drain queued GSI payloads: diff into events, react to match lifecycle.
        if let Some(g) = gsi.as_ref() {
            while let Ok(body) = g.rx.try_recv() {
                let Some(p) = payload::parse_valid(&body) else {
                    continue;
                };
                let res = tracker.feed(&p);

                if res.new_match {
                    if let Some(am) = active.take() {
                        tracing::info!("cs2: new match detected — finalizing previous recording");
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
                            am.events.push((kind, now_ticks()));
                            tracing::debug!("cs2: event {}", kind.label());
                        }
                    }
                }

                if res.game_over {
                    if let Some(am) = active.take() {
                        tracing::info!("cs2: match over — finalizing recording");
                        end_match(&app, am, mode, toggles, timings);
                    }
                    want_match = false;
                    want_since = None;
                }
            }
        }

        // Open the session once a match has latched + the encoder is warm.
        if want_match && active.is_none() && mode.records_match() {
            let grace = want_since.map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(rec) = ctx.open_session("cs2_session", grace) {
                tracing::info!("cs2: recording match → {}", rec.session_path.display());
                active = Some(Cs2Active {
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

/// Finish the session and (on its own blocking task) reconcile + cut the
/// highlights, or save the whole match (FullMatch mode).
fn end_match(
    app: &AppHandle,
    am: Cs2Active,
    mode: AutoCaptureMode,
    toggles: Cs2EventToggles,
    timings: Cs2EventTimings,
) {
    let fps = am.rec.fps.max(1);
    let cs2_ctx = am.ctx;
    let events = am.events;

    tracing::info!("cs2: match end — {} highlight event(s) collected", events.len());

    let Some((path, output)) = am.rec.finish() else {
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();

    tauri::async_runtime::spawn_blocking(move || {
        let clip_context = cs2_ctx.clip_context();
        let title_suffix = cs2_ctx.title_suffix();

        if mode == AutoCaptureMode::FullMatch {
            let title = if title_suffix.is_empty() {
                "Full Match".to_string()
            } else {
                format!("Full Match — {title_suffix}")
            };
            if let Err(e) = save_whole_session(&app, &path, &title, "Full Match", clip_context) {
                tracing::warn!("cs2: full-match save failed: {e}");
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
            "cs2: placed {}/{} event(s) onto the recorded timeline",
            placed.len(),
            events.len()
        );
        if placed.is_empty() {
            tracing::info!("cs2: no enabled highlights landed in the recording");
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
                game_label: "Counter-Strike 2",
                title_suffix: &title_suffix,
                clip_context,
            },
            &placed,
            &marks,
        );
        let _ = std::fs::remove_file(&path);
    });
}

/// Session-mode continuous recording (one clip while capture is live).
fn manage_full_session(ctx: &GameCtx, mode: AutoCaptureMode, slot: &mut Option<RecordingSession>) {
    let want = mode == AutoCaptureMode::Session && ctx.is_capturing();
    match (want, slot.is_some()) {
        (true, false) => {
            if let Some(fs) = ctx.open_session("cs2_fullsession", true) {
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
