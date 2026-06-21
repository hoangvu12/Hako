//! Live Valorant match orchestration — the Mode-B auto-clip driver.
//!
//! A single background task (spawned from `main`) polls our Riot presence every
//! ~2 s, feeds the [`StateMachine`], and reacts to its [`Action`]s:
//!
//! - **MatchStarted** — if a capture is running, open a [`SessionWriter`] and
//!   install it into the live [`ClipBuffer`] so every encoded packet is teed to
//!   a full-match MP4; mark the [`RoundTracker`] match-found; open a [`LogTail`]
//!   at EOF for round-start anchors; and kick off the remote-API bootstrap
//!   ([`cut::bootstrap_remote`]) so the match id + tokens are ready by match end.
//! - **RoundBoundary** — anchors come from the log tail now, so this is just a
//!   UI signal.
//! - **MatchEnded** — detach + finish the session writer and hand the file +
//!   timeline + anchors to the post-match cut pipeline ([`cut::post_match`]).
//!
//! Each tick also drains the log tail (round-ended lines → [`RoundTracker`]) and
//! emits a [`events::MATCH_STATE_CHANGED`] snapshot for the `/valorant` panel.
//!
//! Capture is **auto-started** when the VALORANT window appears (Medal-style
//! game detection, [`auto_manage_capture`]) and auto-stopped when the game
//! exits, so the encoder is already warm before a match begins. A capture the
//! user started manually is left untouched (we only auto-stop our own). The
//! session tees off this *existing* encode stream.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::JoinHandle;

use crate::commands::{self, CaptureState, SettingsState};
use crate::core::capture::{self, ClipBuffer};
use crate::core::session::SessionWriter;
use crate::events;
use crate::settings::AutoCaptureMode;
use crate::valorant::cut::{self, RemoteReady};
use crate::valorant::live::{LiveMatch, LiveMatchState};
use crate::valorant::local_api::LocalClient;
use crate::valorant::log_watch::{self, LogTail, RoundTracker};
use crate::valorant::model::{self, LoopState, PrivatePresence};
use crate::valorant::remote_api;
use crate::valorant::service::{Action, StateMachine};

/// Presence poll cadence (Medal polls the local API on a similar interval). The
/// log tail is drained on the same tick; ±10 s clip padding absorbs the latency.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Snapshot pushed to the webview as [`events::MATCH_STATE_CHANGED`].
#[derive(Debug, Clone, Serialize)]
pub struct MatchStatePayload {
    /// `MENUS` / `PREGAME` / `INGAME` / etc.
    pub loop_state: String,
    /// True while a match is in progress (state machine INGAME).
    pub in_match: bool,
    /// True while a full-match session is actually being recorded.
    pub recording: bool,
    pub score_ally: i32,
    pub score_enemy: i32,
    pub map: String,
}

/// State for the in-progress match recording.
struct ActiveMatch {
    /// The capture's clip buffer (we installed our session into it).
    clip: Arc<ClipBuffer>,
    /// The full-match session writer (also installed in `clip`).
    session: Arc<SessionWriter>,
    /// Round-start anchors gathered from the log.
    tracker: RoundTracker,
    /// Incremental `ShooterGame.log` tailer (None if the log wasn't found).
    log_tail: Option<LogTail>,
    /// Wall-clock tick at match start (fallback reconciliation anchor).
    started_ticks: i64,
    /// Session temp MP4 path (deleted after clips are cut).
    session_path: PathBuf,
    /// Capture fps (session video time base).
    fps: u32,
    /// Remote bootstrap (tokens + match id), resolved by match end.
    bootstrap: JoinHandle<Option<RemoteReady>>,
}

impl ActiveMatch {
    /// Tear down without cutting (stale/aborted match): stop teeing, finish the
    /// writer, drop the temp file, cancel the bootstrap.
    fn discard(self) {
        self.clip.take_session();
        let _ = self.session.finish();
        let _ = std::fs::remove_file(&self.session_path);
        self.bootstrap.abort();
    }
}

/// Spawn the orchestrator on the Tauri async runtime. Idempotent per app.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move { run(app).await });
}

async fn run(app: AppHandle) {
    let mut sm = StateMachine::new();
    let mut active: Option<ActiveMatch> = None;
    // Continuous full-session recording (only used in `session` mode).
    let mut full_session: Option<FullSession> = None;
    // True while *we* auto-started the capture (so we only auto-stop our own,
    // never the user's manual capture).
    let mut auto_capturing = false;
    // Earliest time to (re)try an auto-start after a failure — backs off so a
    // failing hook (e.g. game minimized / not rendering) isn't re-injected every
    // tick into the anti-cheat-protected process.
    let mut next_capture_attempt = Instant::now();
    // True once we've kicked off the (best-effort, once-per-match) live-agent
    // resolver for the current match; reset when we leave the match.
    let mut live_resolver_spawned = false;
    // Set when a match starts but the session writer couldn't open yet (encoder
    // not warm — app opened mid-game). Latches the intent so we keep retrying
    // start_match each tick until it succeeds or the match ends, instead of
    // losing the whole match because `MatchStarted` is a one-shot edge.
    let mut want_match_record = false;
    // When the match-record intent was first latched, so we can wait a bounded
    // grace period for the audio encoders to publish their track metadata before
    // opening the session writer (mid-game, capture only just began and the
    // per-process loopback inputs take a moment to come up). Without the wait the
    // session — and every auto-clip cut from it — would be declared video-only.
    let mut want_match_since: Option<Instant> = None;
    // Periodic retry of matches whose post-match details fetch failed (pending
    // queue), and the in-flight reconcile task (so passes never overlap).
    let mut next_reconcile = Instant::now();
    let mut reconcile_task: Option<JoinHandle<()>> = None;
    tracing::info!("valorant orchestrator started");

    loop {
        tokio::time::sleep(POLL_INTERVAL).await;

        // Medal-style game detection: auto-start capture when the VALORANT window
        // appears (encoder is warm before any match) and auto-stop when it exits.
        // Independent of presence so it works even if the local API is flaky.
        auto_manage_capture(&app, &mut auto_capturing, &mut next_capture_attempt);

        // Retry any matches whose details fetch failed earlier, whenever the Riot
        // client is reachable again. Throttled + non-overlapping; cheap no-op when
        // nothing is pending or the client is down.
        maybe_reconcile_pending(&app, &mut next_reconcile, &mut reconcile_task);

        // The user's capture mode (Manual / Highlights / FullMatch / Session).
        // Read each tick so changing it in settings takes effect without restart.
        let mode = current_auto_mode(&app);
        // Session mode: keep one clip rolling for as long as capture is live,
        // independent of match boundaries. A no-op in the other modes.
        manage_full_session(&app, mode, &mut full_session);

        // Push the live recorder snapshot (game-detected / capturing) so the
        // titlebar's "Now Clipping" indicator updates without the game's presence.
        let _ = app.emit(events::RECORDER_STATUS, &commands::recorder_status_snapshot(&app));

        // Resolve the local API + our presence; transient failures just skip the
        // tick (Riot not up yet, account switching, etc.).
        let (presence, puuid) = match poll_presence().await {
            Some(p) => p,
            None => {
                // Local API gone. If Valorant is *also* gone while we were
                // recording a match, the game crashed / we were kicked to desktop
                // — finalize what we captured instead of holding it until the next
                // match discards it, and reset the machine so a reconnect to the
                // same match registers as a fresh start (recorded again by #1's
                // latch). A still-running game with a flaky local API is left
                // alone (we keep recording and retry the presence next tick).
                if active.is_some() && !crate::valorant::service::valorant_running() {
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

        // Keep the shared live-match context current (map/mode from presence,
        // agent resolved best-effort) so a manual F9 save can tag its clip with
        // the same context an auto-clip would. Independent of capture mode.
        update_live_match(&app, &presence, loop_state, &puuid, &mut live_resolver_spawned);

        for action in sm.update(loop_state, rounds_played) {
            match action {
                Action::MatchStarted => {
                    // Only Highlights / FullMatch record a per-match session.
                    // Manual and Session don't (Manual = buffer + hotkey only;
                    // Session records continuously via `manage_full_session`).
                    if !mode.records_match() {
                        continue;
                    }
                    // Per-game-mode gate: skip a match whose live queue the user
                    // turned off (unknown/rotating ids fall to the `other`
                    // catch-all). Read each tick so it tracks settings changes.
                    let qid = presence.queue_id();
                    if !current_auto_clip_modes(&app).enabled(qid) {
                        tracing::info!(
                            "auto-clip: skipping match — game mode '{qid}' disabled"
                        );
                        continue;
                    }
                    if let Some(stale) = active.take() {
                        tracing::warn!("auto-clip: new match started over an unfinished one");
                        stale.discard();
                    }
                    // Latch the intent; the writer is actually opened below (this
                    // same tick when the encoder is warm, or on a later tick if we
                    // started mid-game and it isn't producing frames yet).
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

        // Open the session writer for a latched match start. On a normal start
        // the encoder is already warm and this runs the same tick as MatchStarted;
        // when the app was opened mid-game, capture only just began, so we keep
        // retrying here until the encoder produces frames (or the match ends and
        // `MatchEnded` clears the latch). Re-checks the mode so switching to a
        // non-recording mode mid-match stops the attempts.
        if want_match_record && active.is_none() && mode.records_match() {
            // Stop deferring for audio once the grace window elapses, so a capture
            // whose audio genuinely never comes up still records (video-only) the
            // match rather than dropping it entirely.
            let audio_grace_expired = want_match_since
                .map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            if let Some(am) = start_match(&app, &puuid, &presence, audio_grace_expired) {
                tracing::info!("auto-clip: recording match → {}", am.session_path.display());
                active = Some(am);
                want_match_record = false;
                want_match_since = None;
            }
        }

        // Drain the log tail for round-ended markers (anchors) while recording.
        if let Some(am) = active.as_mut() {
            drain_log(am);
        }

        emit_state(&app, &presence, active.is_some());
    }
}

/// Cadence for retrying pending (details-fetch-failed) matches.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn a [`cut::reconcile_pending`] pass if it's due, none is already running,
/// and there's actually something queued. Spawned (not awaited) so a slow Riot
/// round-trip never blocks presence polling.
fn maybe_reconcile_pending(
    app: &AppHandle,
    next: &mut Instant,
    task: &mut Option<JoinHandle<()>>,
) {
    if Instant::now() < *next {
        return;
    }
    // Don't start a new pass while the previous one is still running.
    if task.as_ref().map_or(false, |t| !t.is_finished()) {
        return;
    }
    *next = Instant::now() + RECONCILE_INTERVAL;
    if !crate::valorant::pending::any(app) {
        return;
    }
    *task = Some(tokio::spawn(cut::reconcile_pending(app.clone())));
}

/// Connect to the local API and read our decoded presence + puuid. `None` on any
/// transient failure (no lockfile, not connected, no Valorant presence yet).
async fn poll_presence() -> Option<(PrivatePresence, String)> {
    let client = LocalClient::connect().ok()?;
    let puuid = client.chat_session().await.ok()?.puuid;
    if puuid.is_empty() {
        return None;
    }
    let presence = client.our_presence(&puuid).await.ok()??;
    Some((presence, puuid))
}

/// How long to wait for the audio encoders to publish all planned track metadata
/// before opening the session writer anyway. Per-process loopback inputs can take
/// a second or two to activate when capture has only just begun (Hako opened
/// mid-game); this bounds that wait so a genuinely audio-less capture still
/// records video rather than dropping the match.
const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);

/// Begin recording a match: open the session writer over the live capture's
/// clip buffer, set up round tracking + the log tail, and start the remote
/// bootstrap. `None` if no capture is running / the encoder isn't ready yet, or
/// (until `audio_grace_expired`) if not all planned audio tracks have published
/// their metadata — the caller latches and retries, so we just wait.
fn start_match(
    app: &AppHandle,
    puuid: &str,
    presence: &PrivatePresence,
    audio_grace_expired: bool,
) -> Option<ActiveMatch> {
    let clip = capture_clip(app)?;
    let meta = clip.clip_meta()?; // encoder not open yet ⇒ can't record
    // All published audio tracks (track 0 = master mix, 1..N = stems) so the
    // session — and the auto-clips cut from it — are multi-track too. Defer until
    // every planned track has published: opening mid-game, the audio thread may
    // still be bringing up its per-process loopback inputs, and a partial (or
    // empty) snapshot here would declare a video-only session, leaving every
    // auto-clip silent. The grace flag caps the wait so we never block forever.
    let audio_tracks = clip.audio_track_metas();
    if !audio_grace_expired && audio_tracks.len() < clip.audio_track_count() {
        tracing::debug!(
            "auto-clip: deferring session start — audio {}/{} tracks published",
            audio_tracks.len(),
            clip.audio_track_count()
        );
        return None;
    }

    let session_path = std::env::temp_dir().join(format!("hako_session_{}.mp4", unix_millis()));
    let writer = match SessionWriter::start(&session_path, &meta, &audio_tracks) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            tracing::warn!("auto-clip: could not open session writer: {e}");
            return None;
        }
    };
    clip.install_session(writer.clone());

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
        clip,
        session: writer,
        tracker,
        log_tail,
        started_ticks,
        session_path,
        fps: meta.fps,
        bootstrap,
    })
}

/// Finish the session and hand it to the post-match cut pipeline on its own task
/// (the match-details retry can take tens of seconds — never block the loop).
fn end_match(app: &AppHandle, am: ActiveMatch, mode: AutoCaptureMode) {
    am.clip.take_session(); // stop teeing into the (now finishing) writer
    let (path, output) = match am.session.finish() {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("auto-clip: finishing session failed: {e}");
            let _ = std::fs::remove_file(&am.session_path);
            am.bootstrap.abort();
            return;
        }
    };
    let timeline = output.timeline;
    let frozen_spans = output.frozen_spans;
    let anchors = am.tracker.anchors();
    let app = app.clone();
    let bootstrap = am.bootstrap;
    let (fps, started_ticks) = (am.fps, am.started_ticks);
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

/// Drain new log lines, feeding round-ended + round-live markers into the
/// tracker. Each is back-dated from the ~2 s poll read time to the line's own
/// embedded `[UTC]` timestamp ([`log_watch::line_event_ticks`]) — otherwise the
/// poll lag stamps round boundaries up to 2 s late and drags every reconciled
/// marker the same amount. A round's precise start comes from its `Gameplay
/// started` (barriers-dropped) line; `OnRoundEnded` only seeds the coarse
/// buy-phase fallback.
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

/// The running capture's clip buffer, or `None` if nothing is capturing.
fn capture_clip(app: &AppHandle) -> Option<Arc<ClipBuffer>> {
    let state = app.state::<CaptureState>();
    let guard = state.0.lock().ok()?;
    guard.as_ref().map(|rc| rc.clip())
}

/// The user's configured auto-capture mode (defaults to Highlights).
fn current_auto_mode(app: &AppHandle) -> AutoCaptureMode {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.auto_mode()))
        .unwrap_or(AutoCaptureMode::Highlights)
}

/// The user's per-game-mode auto-clip gate (defaults to all-on when settings
/// state isn't available yet — never silently drop a match on a read miss).
fn current_auto_clip_modes(app: &AppHandle) -> model::GameModeToggles {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.auto_clip_modes.clone()))
        .unwrap_or_default()
}

/// A continuously-rolling full-session recording (Session mode): one writer teed
/// off the live capture for as long as capture is up, saved as a single clip.
struct FullSession {
    clip: Arc<ClipBuffer>,
    session: Arc<SessionWriter>,
    session_path: PathBuf,
}

/// Drive Session-mode recording: while in `session` mode and capture is live,
/// keep one [`FullSession`] open (opened lazily once the encoder is ready); when
/// capture stops, the mode changes, or the game closes, finish it and save the
/// whole file as one clip. A no-op in every other mode.
fn manage_full_session(app: &AppHandle, mode: AutoCaptureMode, slot: &mut Option<FullSession>) {
    let want = mode == AutoCaptureMode::Session && commands::is_capturing(app);
    match (want, slot.is_some()) {
        // Should be recording and isn't yet → try to open (may not be ready).
        (true, false) => {
            if let Some(fs) = start_full_session(app) {
                tracing::info!("session-record: rolling → {}", fs.session_path.display());
                *slot = Some(fs);
            }
        }
        // Shouldn't be recording but is → finish + save what we have.
        (false, true) => {
            if let Some(fs) = slot.take() {
                finish_full_session(app, fs);
            }
        }
        _ => {}
    }
}

/// Open a full-session writer over the live capture's clip buffer. `None` if no
/// capture is running or the encoder isn't producing frames yet (retried next
/// tick).
fn start_full_session(app: &AppHandle) -> Option<FullSession> {
    let clip = capture_clip(app)?;
    let meta = clip.clip_meta()?; // encoder not open yet ⇒ try again later
    let audio_tracks = clip.audio_track_metas();
    let session_path = std::env::temp_dir().join(format!("hako_fullsession_{}.mp4", unix_millis()));
    let writer = match SessionWriter::start(&session_path, &meta, &audio_tracks) {
        Ok(w) => Arc::new(w),
        Err(e) => {
            tracing::warn!("session-record: could not open session writer: {e}");
            return None;
        }
    };
    clip.install_session(writer.clone());
    Some(FullSession {
        clip,
        session: writer,
        session_path,
    })
}

/// Finish a full-session recording and save the whole file as one library clip
/// (on a blocking task — the copy can take a moment). Best-effort.
fn finish_full_session(app: &AppHandle, fs: FullSession) {
    fs.clip.take_session(); // stop teeing into the writer
    let (path, _output) = match fs.session.finish() {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("session-record: finishing session failed: {e}");
            let _ = std::fs::remove_file(&fs.session_path);
            return;
        }
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

/// Backoff after a failed auto-start before retrying (don't re-inject the hook
/// into the game every tick when it isn't capturable yet).
const CAPTURE_RETRY_BACKOFF: Duration = Duration::from_secs(20);

/// Auto-start capture of the VALORANT window when the game is running, and
/// auto-stop it when the game exits — Medal's "detect the game, start recording"
/// behavior. Only ever touches a capture *we* started (`auto_capturing`), so a
/// user's manual capture is never auto-stopped, and we never fight a capture the
/// user started manually. Starts the capture via [`commands::start_capture_with`].
fn auto_manage_capture(app: &AppHandle, auto_capturing: &mut bool, next_attempt: &mut Instant) {
    let game = capture::find_valorant_window();
    let capturing = commands::is_capturing(app);

    match game {
        // Game is up but nothing is recording → start (record that it's ours).
        Some(hwnd) if !capturing => {
            // A minimized game (exclusive fullscreen alt-tabbed) stops rendering,
            // so the hook gets no frames — wait until it's back on screen instead
            // of repeatedly injecting into a non-rendering, anti-cheat-watched app.
            if capture::is_window_minimized(hwnd) {
                return;
            }
            if Instant::now() < *next_attempt {
                return; // still backing off from a recent failure
            }
            match commands::start_capture_with(app, hwnd, None, None) {
                Ok(()) => {
                    *auto_capturing = true;
                    tracing::info!("auto-capture: VALORANT detected → capture started");
                }
                Err(e) => {
                    *next_attempt = Instant::now() + CAPTURE_RETRY_BACKOFF;
                    tracing::warn!(
                        "auto-capture: could not start capture (retrying in {}s): {e}",
                        CAPTURE_RETRY_BACKOFF.as_secs()
                    );
                }
            }
        }
        // Game gone → stop only the capture we started ourselves.
        None => {
            if *auto_capturing && capturing {
                commands::stop_capture_with(app);
                tracing::info!("auto-capture: VALORANT closed → capture stopped");
            }
            *auto_capturing = false;
            *next_attempt = Instant::now();
        }
        // Game up and already capturing (ours or the user's) → leave it be.
        Some(_) => {}
    }
}

/// Refresh the shared [`LiveMatchState`] from the current presence tick. While
/// INGAME it mirrors the live map + mode and, once per match, spawns a
/// best-effort task to resolve our agent. On leaving the match it resets the
/// context so a later manual save outside a game carries nothing.
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
            let map = presence.match_map();
            g.map = (!map.is_empty()).then(|| map.to_string());
            // Display name for the live queue id; fall back to the raw id so an
            // unmapped mode is still a filterable label.
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
        // Out of a match → clear context + allow a fresh resolve next match.
        *resolver_spawned = false;
        if let Ok(mut g) = state.0.lock() {
            if g.in_match {
                *g = LiveMatch::default();
            }
        }
    }
}

/// Resolve our agent for the live match and write it into [`LiveMatchState`].
/// Best-effort: bootstraps the remote API, reads the in-progress core-game match
/// for our `CharacterID`, resolves the display name, and stores it (only while
/// still in the same match). Any failure leaves `agent` `None`.
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
            // Only apply if we're still in a match (it may have ended meanwhile).
            if g.in_match {
                tracing::info!("live-agent: resolved {} for manual clips", name.as_deref().unwrap_or(&agent_id));
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
        in_match: presence.loop_state() == crate::valorant::model::LoopState::InGame,
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

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
