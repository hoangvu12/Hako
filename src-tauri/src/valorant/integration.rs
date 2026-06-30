//! Valorant integration — the post-match-reconcile [`GameIntegration`].
//!
//! A single background task (spawned by the games supervisor) polls our Riot
//! presence every ~2 s, feeds the [`StateMachine`], and reacts to its
//! [`Action`]s, teeing the live encode stream into a full-match MP4 while in a
//! match and cutting per-event highlights at match end. The generic capture /
//! session / cut plumbing lives in [`crate::games::recording`]; everything here
//! is Riot-specific (presence, the log tail's round anchors, the post-match
//! match-details fetch + reconciliation).
//!
//! Capture is **auto-started** when the VALORANT window appears and auto-stopped
//! when the game exits (handled by [`GameCtx::auto_manage_capture`]); a capture
//! the user started manually is left untouched.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinHandle;

use crate::commands::SettingsState;
use crate::core::capture;
use crate::events;
use crate::games::recording::{AutoCaptureState, GameCtx, RecordingSession};
use crate::games::{GameId, GameIntegration};
use crate::settings::AutoCaptureMode;
use crate::valorant::cut::{self, RemoteReady};
use crate::valorant::live::{LiveMatch, LiveMatchState};
use crate::valorant::local_api::LocalClient;
use crate::valorant::log_watch::{self, LogTail, RoundTracker};
use crate::valorant::model::{self, LoopState, PrivatePresence};
use crate::valorant::remote_api;
use crate::valorant::service::{self, Action, StateMachine};

/// Presence poll cadence. The log tail is drained on the same tick; ±10 s clip
/// padding absorbs the latency.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long to wait for the audio encoders to publish all planned track metadata
/// before opening the session writer anyway.
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);

/// Cadence for retrying pending (details-fetch-failed) matches.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// The Valorant [`GameIntegration`] (zero-sized; all state is loop-local).
pub struct Integration;

#[async_trait]
impl GameIntegration for Integration {
    fn id(&self) -> GameId {
        GameId::Valorant
    }

    fn find_window(&self) -> Option<i64> {
        capture::find_valorant_window()
    }

    fn detect_running(&self) -> bool {
        service::valorant_running()
    }

    async fn run(self: Arc<Self>, ctx: GameCtx) {
        run(ctx).await;
    }
}

/// Snapshot pushed to the webview as [`events::MATCH_STATE_CHANGED`].
#[derive(Debug, Clone, Serialize)]
pub struct MatchStatePayload {
    pub loop_state: String,
    pub in_match: bool,
    pub recording: bool,
    pub score_ally: i32,
    pub score_enemy: i32,
    pub map: String,
}

/// State for the in-progress match recording.
struct ActiveMatch {
    /// Full-match session writer teed off the live capture.
    rec: RecordingSession,
    /// Round-start anchors gathered from the log.
    tracker: RoundTracker,
    /// Incremental `ShooterGame.log` tailer (None if the log wasn't found).
    log_tail: Option<LogTail>,
    /// Wall-clock tick at match start (fallback reconciliation anchor).
    started_ticks: i64,
    /// Live queue id this match is in (per-game-mode gate).
    queue_id: String,
    /// Remote bootstrap (tokens + match id), resolved by match end.
    bootstrap: JoinHandle<Option<RemoteReady>>,
}

impl ActiveMatch {
    /// Tear down without cutting (stale/aborted match).
    fn discard(self) {
        self.rec.discard();
        self.bootstrap.abort();
    }
}

async fn run(ctx: GameCtx) {
    let app = ctx.app.clone();
    let mut sm = StateMachine::new();
    let mut active: Option<ActiveMatch> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut autocap = AutoCaptureState::new();
    let mut live_resolver_spawned = false;
    let mut want_match_record = false;
    let mut want_match_since: Option<Instant> = None;
    let mut next_reconcile = Instant::now();
    let mut reconcile_task: Option<JoinHandle<()>> = None;
    tracing::info!("valorant integration started");

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Medal-style game detection (auto start/stop capture via the shared arbiter).
        // When the game is "disabled", we never auto-attach and force Manual below
        // so any in-flight auto-recording is torn down by the existing paths.
        let disabled = current_capture_disabled(&app);
        ctx.auto_manage_capture(&mut autocap, disabled);

        // Retry pending (details-fetch-failed) matches when the client is back.
        maybe_reconcile_pending(&app, &mut next_reconcile, &mut reconcile_task);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            current_auto_mode(&app)
        };
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!("auto-clip: capture disabled mid-match — discarding recording");
                am.discard();
            }
            want_match_record = false;
            want_match_since = None;
        }

        // Per-game-mode mid-match gate.
        if let Some(am) = active.as_ref() {
            if !current_auto_clip_modes(&app).enabled(&am.queue_id) {
                tracing::info!(
                    "auto-clip: game mode '{}' disabled mid-match — discarding recording",
                    am.queue_id
                );
                active.take().unwrap().discard();
                want_match_record = false;
                want_match_since = None;
            }
        }

        // Restart-class settings change mid-session → clean split.
        if ctx.take_config_restart() {
            let mut resume_match = false;
            if let Some(am) = active.take() {
                tracing::info!("auto-clip: config changed mid-match — splitting clip + restarting capture");
                end_match(&app, am, mode);
                resume_match = mode.records_match();
            }
            if let Some(fs) = full_session.take() {
                tracing::info!("session-record: config changed — splitting session + restarting capture");
                finish_full_session(&app, fs);
            }
            ctx.restart_capture();
            if resume_match {
                want_match_record = true;
                want_match_since = Some(Instant::now());
            }
        }

        ctx.emit_recorder_status();

        // Resolve presence; transient failures skip the tick.
        let (presence, puuid) = match poll_presence().await {
            Some(p) => p,
            None => {
                if active.is_some() && !ctx.game_running() {
                    if let Some(am) = active.take() {
                        tracing::warn!("auto-clip: game vanished mid-match — finalizing recording");
                        end_match(&app, am, mode);
                    }
                    want_match_record = false;
                    want_match_since = None;
                    sm = StateMachine::new();
                }
                continue;
            }
        };
        let loop_state = presence.loop_state();
        let rounds_played = presence.score_ally + presence.score_enemy;

        update_live_match(&app, &presence, loop_state, &puuid, &mut live_resolver_spawned);

        for action in sm.update(loop_state, rounds_played) {
            match action {
                Action::MatchStarted => {
                    if !mode.records_match() {
                        continue;
                    }
                    let qid = presence.queue_id();
                    if !current_auto_clip_modes(&app).enabled(qid) {
                        tracing::info!("auto-clip: skipping match — game mode '{qid}' disabled");
                        continue;
                    }
                    if let Some(stale) = active.take() {
                        tracing::warn!("auto-clip: new match started over an unfinished one");
                        stale.discard();
                    }
                    want_match_record = true;
                    want_match_since.get_or_insert_with(Instant::now);
                }
                Action::RoundBoundary { rounds_played } => {
                    tracing::debug!("auto-clip: round boundary ({rounds_played} played)");
                }
                Action::MatchEnded => {
                    want_match_record = false;
                    want_match_since = None;
                    if let Some(am) = active.take() {
                        end_match(&app, am, mode);
                    }
                }
            }
        }

        // Open the session writer for a latched match start (retried until the
        // encoder is warm + audio tracks published, or the match ends).
        if want_match_record
            && active.is_none()
            && mode.records_match()
            && current_auto_clip_modes(&app).enabled(presence.queue_id())
        {
            let audio_grace_expired = want_match_since
                .map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(am) = start_match(&ctx, &puuid, &presence, audio_grace_expired) {
                tracing::info!("auto-clip: recording match → {}", am.rec.session_path.display());
                active = Some(am);
                want_match_record = false;
                want_match_since = None;
            }
        }

        if let Some(am) = active.as_mut() {
            drain_log(am);
        }

        emit_state(&app, &presence, active.is_some());
    }
}

/// Begin recording a match: open the session writer over the live capture, set up
/// round tracking + the log tail, and start the remote bootstrap. `None` if no
/// capture is running / the encoder isn't ready / audio tracks not yet published.
fn start_match(
    ctx: &GameCtx,
    puuid: &str,
    presence: &PrivatePresence,
    audio_grace_expired: bool,
) -> Option<ActiveMatch> {
    let rec = ctx.open_session("session", audio_grace_expired)?;

    let started_ticks = log_watch::now_ticks();
    let mut tracker = RoundTracker::new(log_watch::buy_phase_ticks(buy_phase_mode(presence)));
    tracker.set_match_found(started_ticks);

    let log_tail = log_watch::log_path().and_then(|p| match LogTail::open_at_end(p) {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::warn!("auto-clip: could not open ShooterGame.log tail: {e}");
            None
        }
    });

    let bootstrap = tokio::spawn(cut::bootstrap_remote(puuid.to_string()));

    Some(ActiveMatch {
        rec,
        tracker,
        log_tail,
        started_ticks,
        queue_id: presence.queue_id().to_string(),
        bootstrap,
    })
}

/// Finish the session and hand it to the post-match cut pipeline on its own task.
fn end_match(app: &AppHandle, am: ActiveMatch, mode: AutoCaptureMode) {
    let fps = am.rec.fps;
    let started_ticks = am.started_ticks;
    let anchors = am.tracker.anchors();
    let bootstrap = am.bootstrap;
    let Some((path, output)) = am.rec.finish() else {
        bootstrap.abort();
        return;
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let app = app.clone();
    tracing::info!("auto-clip: match ended, reconciling {} round anchor(s)", anchors.len());

    tokio::spawn(async move {
        let remote = bootstrap.await.ok().flatten();
        cut::post_match(cut::CutInput {
            app,
            session_path: path,
            timeline,
            frozen_spans,
            anchors,
            fps,
            game_start_ticks: started_ticks,
            remote,
            mode,
        })
        .await;
    });
}

/// Drain new log lines, feeding round-ended + round-live markers into the tracker.
fn drain_log(am: &mut ActiveMatch) {
    let Some(tail) = am.log_tail.as_mut() else {
        return;
    };
    match tail.poll_new_lines() {
        Ok(lines) => {
            for line in lines {
                if let Some(round) = log_watch::parse_round_ended(&line) {
                    am.tracker.on_round_ended(round, log_watch::line_event_ticks(&line));
                } else if log_watch::is_round_live(&line) {
                    am.tracker.on_round_live(log_watch::line_event_ticks(&line));
                }
            }
        }
        Err(e) => tracing::debug!("auto-clip: log tail read: {e}"),
    }
}

/// Drive Session-mode recording: keep one [`RecordingSession`] open while in
/// `session` mode and capture is live; finish + save it as one clip otherwise.
fn manage_full_session(ctx: &GameCtx, mode: AutoCaptureMode, slot: &mut Option<RecordingSession>) {
    let want = mode == AutoCaptureMode::Session && ctx.is_capturing();
    match (want, slot.is_some()) {
        (true, false) => {
            if let Some(fs) = ctx.open_session("fullsession", true) {
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

/// Finish a full-session recording and save the whole file as one library clip.
fn finish_full_session(app: &AppHandle, fs: RecordingSession) {
    let Some((path, _output)) = fs.finish() else {
        return;
    };
    let app = app.clone();
    tokio::spawn(async move {
        if let Err(e) = cut::save_whole_session(
            &app,
            &path,
            "Full Session",
            "Full Session",
            crate::library::db::NewClip::default(),
        ) {
            tracing::warn!("session-record: save failed: {e}");
        }
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::debug!("session-record: temp cleanup: {e}");
        }
    });
}

/// Spawn a [`cut::reconcile_pending`] pass if it's due and none is running.
fn maybe_reconcile_pending(
    app: &AppHandle,
    next: &mut Instant,
    task: &mut Option<JoinHandle<()>>,
) {
    if Instant::now() < *next {
        return;
    }
    if task.as_ref().map_or(false, |t| !t.is_finished()) {
        return;
    }
    *next = Instant::now() + RECONCILE_INTERVAL;
    if !crate::valorant::pending::any(app) {
        return;
    }
    *task = Some(tokio::spawn(cut::reconcile_pending(app.clone())));
}

/// Connect to the local API and read our decoded presence + puuid.
async fn poll_presence() -> Option<(PrivatePresence, String)> {
    let client = LocalClient::connect().ok()?;
    let puuid = client.chat_session().await.ok()?.puuid;
    if puuid.is_empty() {
        return None;
    }
    let presence = client.our_presence(&puuid).await.ok()??;
    Some((presence, puuid))
}

/// The user's configured auto-capture mode (defaults to Highlights).
fn current_auto_mode(app: &AppHandle) -> AutoCaptureMode {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.auto_mode()))
        .unwrap_or(AutoCaptureMode::Highlights)
}

/// Whether the user has fully disabled Hako for Valorant ("don't capture this
/// game at all"). Defaults to enabled when settings are unavailable.
fn current_capture_disabled(app: &AppHandle) -> bool {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.auto_capture_disabled))
        .unwrap_or(false)
}

/// The user's per-game-mode auto-clip gate (defaults to all-on).
fn current_auto_clip_modes(app: &AppHandle) -> model::GameModeToggles {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.auto_clip_modes.clone()))
        .unwrap_or_default()
}

/// Refresh the shared [`LiveMatchState`] from the current presence tick.
fn update_live_match(
    app: &AppHandle,
    presence: &PrivatePresence,
    loop_state: LoopState,
    puuid: &str,
    resolver_spawned: &mut bool,
) {
    let Some(state) = app.try_state::<LiveMatchState>() else {
        return;
    };
    if loop_state == LoopState::InGame {
        if let Ok(mut g) = state.0.lock() {
            g.in_match = true;
            g.game = Some("valorant".to_string());
            let map = presence.match_map();
            g.map = (!map.is_empty()).then(|| map.to_string());
            let qid = presence.queue_id();
            let mode = model::queue_id_name(qid);
            let mode = if mode.is_empty() { qid } else { mode };
            g.mode = (!mode.is_empty()).then(|| mode.to_string());
        }
        if !*resolver_spawned {
            *resolver_spawned = true;
            tauri::async_runtime::spawn(resolve_live_agent(app.clone(), puuid.to_string()));
        }
    } else {
        *resolver_spawned = false;
        if let Ok(mut g) = state.0.lock() {
            if g.in_match {
                *g = LiveMatch::default();
            }
        }
    }
}

/// Resolve our agent for the live match and write it into [`LiveMatchState`].
async fn resolve_live_agent(app: AppHandle, puuid: String) {
    let Some(ready) = cut::bootstrap_remote(puuid.clone()).await else {
        return;
    };
    let Some(match_id) = ready.match_id.as_deref() else {
        return;
    };
    let agent_id = match ready.data.remote.core_game_match(match_id).await {
        Ok(m) => m.agent_for(&puuid),
        Err(e) => {
            tracing::debug!("live-agent: core-game match fetch: {e}");
            return;
        }
    };
    let Some(agent_id) = agent_id else {
        return;
    };
    let name = remote_api::fetch_agent_name(&agent_id).await;
    if let Some(state) = app.try_state::<LiveMatchState>() {
        if let Ok(mut g) = state.0.lock() {
            if g.in_match {
                tracing::info!(
                    "live-agent: resolved {} for manual clips",
                    name.as_deref().unwrap_or(&agent_id)
                );
                g.agent_id = Some(agent_id);
                g.agent = name;
            }
        }
    }
}

/// Emit the current match-state snapshot for the UI.
fn emit_state(app: &AppHandle, presence: &PrivatePresence, recording: bool) {
    let ls = presence.session_loop_state();
    let payload = MatchStatePayload {
        loop_state: ls.to_string(),
        in_match: presence.loop_state() == LoopState::InGame,
        recording,
        score_ally: presence.score_ally,
        score_enemy: presence.score_enemy,
        map: presence.match_map().to_string(),
    };
    let _ = app.emit(events::MATCH_STATE_CHANGED, &payload);
}

/// Medal uses a 20 s buy phase for Spike Rush, 30 s otherwise.
fn buy_phase_mode(presence: &PrivatePresence) -> &'static str {
    if presence.queue_id().eq_ignore_ascii_case("spikerush") {
        "Spike Rush"
    } else {
        ""
    }
}
