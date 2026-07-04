//! The generic live-feed run loop shared by every smart game integration.
//!
//! Every game (cs2 / dota2 / lol / rematch / warthunder / pubg) drove an almost
//! identical `run` loop: sleep a tick, auto-manage capture by window detection,
//! read the user's mode, roll the Session-mode full recording, tear down an
//! in-flight match when the mode flips or a config-restart is requested, push the
//! recorder-status snapshot, then — the one genuinely game-specific part — drain
//! its event source and react to the match lifecycle, and finally open a Mode-B
//! session once a match has latched and the encoder is warm.
//!
//! [`run_live_feed`] owns that shared scaffold; each game supplies a
//! [`LiveDriver`] for the parts that differ: its event-source state, how it opens
//! / discards / finishes a match, and the per-tick drain ([`LiveDriver::drive`]).

#![allow(dead_code)]

use std::time::{Duration, Instant};

use async_trait::async_trait;
use tauri::AppHandle;

use crate::games::recording::{
    finish_full_session, game_auto_mode, game_capture_disabled, manage_full_session,
    AutoCaptureState, GameCtx, RecordingSession,
};
use crate::games::GameId;
use crate::settings::AutoCaptureMode;

/// Loop cadence while the game is running (event latency well under clip padding).
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Relaxed cadence while the game isn't running (nothing to drain).
pub const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Grace for audio-track metadata before opening the session writer.
pub const AUDIO_READY_GRACE: Duration = Duration::from_secs(8);

/// The engine's "a match wants recording" latch: whether we're waiting to open a
/// session, and since when (the encoder-warm grace clock). A driver arms/clears it
/// from [`LiveDriver::drive`]; the engine consults it to open the session.
pub struct Wanting {
    pub want_match: bool,
    pub want_since: Option<Instant>,
}

impl Wanting {
    fn new() -> Self {
        Wanting {
            want_match: false,
            want_since: None,
        }
    }

    /// Latch on. Idempotent on the grace clock — the first arm wins, so the audio
    /// grace is measured from when the match was first wanted.
    pub fn arm(&mut self) {
        self.want_match = true;
        self.want_since.get_or_insert_with(Instant::now);
    }

    /// Latch on, restarting the grace clock — used when re-arming right after
    /// finalizing a previous match (a fresh match, a fresh grace window).
    pub fn rearm(&mut self) {
        self.want_match = true;
        self.want_since = Some(Instant::now());
    }

    /// Clear the latch.
    pub fn clear(&mut self) {
        self.want_match = false;
        self.want_since = None;
    }
}

/// The per-game half of the live-feed loop. The engine owns the loop, the
/// auto-capture/session/config-restart scaffold, and the `Option<Active>` /
/// [`Wanting`] state; the driver owns its event source and the match lifecycle.
#[async_trait]
pub trait LiveDriver: Send {
    /// The game's in-progress recording (its `XActive`: the session writer plus
    /// whatever events/context it accumulates over a match).
    type Active: Send + 'static;

    /// Which game this is (log tag + session-file prefix + settings key).
    fn id(&self) -> GameId;

    /// Re-read the user's per-game settings for this tick (toggles, timings,
    /// nickname…), cached on the driver for [`Self::drive`] / [`Self::finish`].
    fn refresh_settings(&mut self, app: &AppHandle);

    /// Open a fresh recording on `rec`, seeded from the driver's current snapshot
    /// (e.g. the live context, or a wall-clock anchor sampled at this instant).
    fn begin(&mut self, rec: RecordingSession) -> Self::Active;

    /// Tear down an in-progress recording without producing a clip (the user
    /// disabled auto-capture mid-match).
    fn discard(&mut self, active: Self::Active);

    /// Finish the session and reconcile + cut its highlights (or save the whole
    /// match in FullMatch mode). Typically delegates to
    /// [`crate::games::recording::finish_and_cut`].
    fn finish(&mut self, app: &AppHandle, active: Self::Active, mode: AutoCaptureMode);

    /// The per-game middle, run every tick after the shared scaffold: manage the
    /// event source for the current `running` state, drain new events into
    /// `*active`, and react to the match lifecycle — finalizing via [`Self::finish`]
    /// and arming/clearing `want` as matches begin and end.
    async fn drive(
        &mut self,
        app: &AppHandle,
        running: bool,
        mode: AutoCaptureMode,
        active: &mut Option<Self::Active>,
        want: &mut Wanting,
    );
}

/// Run a game's live-feed loop forever. Shared by every smart integration — the
/// only per-game code is `driver`.
pub async fn run_live_feed<D: LiveDriver>(ctx: GameCtx, mut driver: D) {
    let app = ctx.app.clone();
    let id = driver.id();
    let mut autocap = AutoCaptureState::new();
    let mut active: Option<D::Active> = None;
    let mut full_session: Option<RecordingSession> = None;
    let mut want = Wanting::new();
    let mut poll = POLL_INTERVAL;
    tracing::info!("{} integration started", id.as_str());

    loop {
        tokio::time::sleep(poll).await;

        // "Disabled" fully ignores the game: no buffer auto-attach, and forcing
        // Manual below tears down any in-flight auto-recording via the paths that
        // already handle a mid-match mode change.
        let disabled = game_capture_disabled(&app, id);
        ctx.auto_manage_capture(&mut autocap, disabled);

        let mode = if disabled {
            AutoCaptureMode::Manual
        } else {
            game_auto_mode(&app, id)
        };
        driver.refresh_settings(&app);
        manage_full_session(&ctx, mode, &mut full_session);

        // Global auto-clip toggle flipped off mid-match → discard.
        if !mode.records_match() {
            if let Some(am) = active.take() {
                tracing::info!(
                    "{}: capture mode disabled mid-match — discarding recording",
                    id.as_str()
                );
                driver.discard(am);
            }
            want.clear();
        }

        // Restart-class settings change mid-session → clean split.
        if ctx.take_config_restart() {
            let mut resume = false;
            if let Some(am) = active.take() {
                driver.finish(&app, am, mode);
                resume = mode.records_match();
            }
            if let Some(fs) = full_session.take() {
                finish_full_session(&app, fs);
            }
            ctx.restart_capture();
            if resume {
                want.rearm();
            }
        }

        ctx.emit_recorder_status();

        // The one game-specific step: drive the event source + match lifecycle.
        let running = ctx.game_running();
        driver
            .drive(&app, running, mode, &mut active, &mut want)
            .await;
        poll = if running {
            POLL_INTERVAL
        } else {
            IDLE_POLL_INTERVAL
        };

        // Open the session once a match has latched + the encoder is warm.
        if want.want_match && active.is_none() && mode.records_match() {
            let grace = want
                .want_since
                .map_or(true, |t| t.elapsed() >= AUDIO_READY_GRACE);
            let prefix = format!("{}_session", id.as_str());
            if let Some(rec) = ctx.open_session(&prefix, grace) {
                tracing::info!("{}: recording match → {}", id.as_str(), rec.session_path.display());
                active = Some(driver.begin(rec));
                want.clear();
            }
        }
    }
}
