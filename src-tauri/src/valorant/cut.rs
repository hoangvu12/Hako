//! Post-match auto-clip cut pipeline (Mode B → library).
//!
//! Runs when a Valorant match ends. Given the finished Mode-B session file +
//! its [`TimelineIndex`] (wall-clock ↔ session PTS) and the live round anchors,
//! it:
//! 1. bootstraps the remote API (already done at match start) and pulls
//!    `match-details` (retried — Riot finalizes a few seconds after the match),
//! 2. derives our highlight events ([`derive_events`], filtered by the user's
//!    [`EventToggles`]),
//! 3. reconciles each event to a session PTS (Medal's `matchStart` calibration
//!    from the logged round starts, falling back to the game-start anchor),
//! 4. builds ±10 s clip windows (clamped to 120 s), merges ones within 10 s
//!    (Medal's `OverlapMergeGrouper`),
//! 5. cuts each merged window out of the session file by stream copy
//!    ([`crate::library::trim`]) and registers it in the library tagged ICYMI,
//! 6. deletes the session temp file.
//!
//! Mirrors Medal's `ValorantPostMatchHandler` + `EventManagementSystem` cut step
//! (constants in `docs/valorant-detection-plan.md` §1).

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Emitter, Manager};

use crate::commands::SettingsState;
use crate::core::clock::TICKS_PER_SECOND;
use crate::events;
use crate::settings::AutoCaptureMode;
use crate::valorant::local_api::LocalClient;
use crate::valorant::model::{EventKind, MatchDetails};
use crate::valorant::pending::{self, PendingMatch};
use crate::valorant::reconcile::{self, EventTimings, EventToggles, RoundAnchor, TimelineIndex};
use crate::valorant::remote_api::{self, RemoteClient};
use crate::valorant::service::{self, SessionData};
use crate::valorant::summary;

/// How long a pending match is retried before we give up and save the whole
/// recording as a single fallback clip (footage is never silently lost). One day
/// covers the common "finish a late match, reopen Valorant next morning" case
/// without holding a full-match MP4 on disk for longer than necessary. Riot's
/// match-history endpoint stays fetchable far longer, so this is a disk-hygiene
/// cap, not a Riot limit.
const MAX_PENDING_AGE_MS: u128 = 24 * 60 * 60 * 1000;
/// Backstop on reconcile attempts (bumped once per pass while the Riot client is
/// up) in case the clock is unreliable. Set above one day of 60 s retries so the
/// age cap above is normally what governs, not this.
const MAX_PENDING_ATTEMPTS: u32 = 2000;

/// `MaxAutoClipLength` — clamp each merged window to 120 s.
const MAX_AUTOCLIP_SECS: i64 = 120;
/// Slack (seconds) for landing an event's anchor on the recorded timeline —
/// absorbs round-anchor read-time jitter. An event whose last action falls more
/// than this outside the recording was never captured (recording started after
/// it — app opened mid-game — or stopped before it) and is dropped, not clamped
/// onto a file end.
const PLACEMENT_TOL_SECS: i64 = 2;
/// `match-details` retry budget (Riot finalizes a few s after match end).
const MATCH_DETAILS_ATTEMPTS: u32 = 6;
const MATCH_DETAILS_DELAY_SECS: u64 = 20;
/// current-game (match id) retry budget while the match is live.
const CURRENT_GAME_ATTEMPTS: u32 = 6;
const CURRENT_GAME_DELAY_SECS: u64 = 10;

/// The remote API + match id, captured at match start (current-game 404s once a
/// match ends, so the id must be grabbed while in-game).
pub struct RemoteReady {
    pub data: SessionData,
    pub match_id: Option<String>,
}

/// Everything the cut needs once a match ends.
pub struct CutInput {
    pub app: AppHandle,
    /// Finished Mode-B session MP4.
    pub session_path: PathBuf,
    /// Wall-clock ↔ session-PTS map built while recording.
    pub timeline: TimelineIndex,
    /// Session-PTS `[start, end]` spans recorded while capture was frozen (game
    /// minimized / stale swapchain). Clips that overlap these beyond a threshold
    /// are skipped so we never ship a dead, frozen auto-clip.
    pub frozen_spans: Vec<(i64, i64)>,
    /// Round-start anchors from the log tail.
    pub anchors: Vec<RoundAnchor>,
    pub fps: u32,
    /// Fallback anchor (match-found wall-clock) when no round anchor matches.
    pub game_start_ticks: i64,
    /// Remote client + match id (None ⇒ bootstrap failed; we can't fetch details).
    pub remote: Option<RemoteReady>,
    /// Whether to cut per-event highlights or keep the whole match
    /// ([`AutoCaptureMode::Highlights`] vs [`AutoCaptureMode::FullMatch`]).
    pub mode: AutoCaptureMode,
}

/// Bootstrap the remote API and grab the live match id. Spawned at match start
/// (chat is already connected, so `start_session` returns fast) so the id is in
/// hand before the match ends. Returns `None` on any failure (manual clips only).
pub async fn bootstrap_remote(puuid: String) -> Option<RemoteReady> {
    let client = match LocalClient::connect() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("auto-clip: local API unavailable for bootstrap: {e}");
            return None;
        }
    };
    let data = match service::start_session(&client).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("auto-clip: session bootstrap failed: {e}");
            return None;
        }
    };
    let match_id = current_match_id_retry(&data.remote, &puuid).await;
    if match_id.is_none() {
        tracing::warn!(
            "auto-clip: could not resolve current match id; details fetch will be skipped"
        );
    }
    Some(RemoteReady { data, match_id })
}

/// Poll core-game for the live match id (6 × 10 s); `None` if never in a game.
async fn current_match_id_retry(remote: &RemoteClient, puuid: &str) -> Option<String> {
    for attempt in 0..CURRENT_GAME_ATTEMPTS {
        match remote.current_match_id(puuid).await {
            Ok(Some(id)) => return Some(id),
            Ok(None) => {}
            Err(e) => tracing::debug!("current-game attempt {}: {e}", attempt + 1),
        }
        if attempt + 1 < CURRENT_GAME_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_secs(CURRENT_GAME_DELAY_SECS)).await;
        }
    }
    None
}

/// Run the full post-match pipeline. Best-effort: logs and cleans up on any
/// failure (a failed auto-clip must never crash the recorder).
pub async fn post_match(input: CutInput) {
    let result = run(&input).await;
    if let Err(e) = &result {
        tracing::warn!("auto-clip pipeline: {e}");
    }
    // Always drop the (large) session temp file when we're done with it.
    if let Err(e) = std::fs::remove_file(&input.session_path) {
        tracing::debug!("auto-clip: session temp cleanup: {e}");
    }
}

async fn run(input: &CutInput) -> Result<(), String> {
    // FullMatch: the footage IS the product — keep the whole match no matter
    // what. Details only enrich the title/summary, so fetch them best-effort and
    // never let a details failure discard the recording.
    if input.mode == AutoCaptureMode::FullMatch {
        let details = fetch_details_opt(input).await;
        return save_full_match(input, details.as_ref()).await;
    }

    // Highlights: we need match-details to derive the events to cut.
    let details = fetch_details_opt(input).await;
    let Some(remote) = input.remote.as_ref() else {
        // Bootstrap failed entirely (no client / match id ever captured): details
        // can never be fetched, so don't discard the footage — save it whole.
        tracing::warn!("auto-clip: no remote API (bootstrap failed) — saving whole match");
        return save_full_match(input, details.as_ref()).await;
    };
    let Some(details) = details else {
        // Details unavailable right now (kicked / token dead / match not yet
        // finalized). Persist for later retry instead of throwing away footage.
        return pend_for_retry(input, remote);
    };

    let params = CutParams {
        app: &input.app,
        session_path: &input.session_path,
        timeline: &input.timeline,
        frozen_spans: &input.frozen_spans,
        anchors: &input.anchors,
        fps: input.fps,
        game_start_ticks: input.game_start_ticks,
        puuid: &remote.data.puuid,
    };
    cut_highlights(&params, &details).await
}

/// Fetch match-details for the cut (6 × 20 s, refreshing the RSO token before
/// each attempt — it expires during a long match; a stale one returns 400
/// BAD_CLAIMS). `None` when no match id was ever captured or Riot never
/// finalized/served the match within the budget.
async fn fetch_details_opt(input: &CutInput) -> Option<MatchDetails> {
    let remote = input.remote.as_ref()?;
    let match_id = remote.match_id.as_deref()?;
    let local = LocalClient::connect().ok();
    if local.is_none() {
        tracing::warn!("auto-clip: local API unavailable — can't refresh the match-details token");
    }
    fetch_match_details_retry(&remote.data.remote, local.as_ref(), match_id).await
}

/// The pieces the highlight cut needs, shared by the live path and the pending
/// reconciler (which rebuilds them from a [`PendingMatch`]).
struct CutParams<'a> {
    app: &'a AppHandle,
    session_path: &'a Path,
    timeline: &'a TimelineIndex,
    frozen_spans: &'a [(i64, i64)],
    anchors: &'a [RoundAnchor],
    fps: u32,
    game_start_ticks: i64,
    puuid: &'a str,
}

/// Save the whole recording as one library clip, using match-details (when
/// available) for the title + game-context tags. FullMatch mode's normal path,
/// and the footage-preserving fallback when highlights can't be produced.
async fn save_full_match(input: &CutInput, details: Option<&MatchDetails>) -> Result<(), String> {
    let (title, context) = match details {
        Some(d) => {
            let puuid = input
                .remote
                .as_ref()
                .map(|r| r.data.puuid.as_str())
                .unwrap_or("");
            let mut summary = summary::build_summary(d, puuid);
            if let Some(name) = remote_api::fetch_agent_name(&summary.agent_id).await {
                summary.agent = name;
            }
            summary.title = summary.build_title();
            let _ = input.app.emit(events::MATCH_SUMMARY, &summary);
            let title = if summary.title.is_empty() {
                "Full Match".to_string()
            } else {
                summary.title.clone()
            };
            (title, summary.clip_context())
        }
        None => {
            tracing::warn!(
                "auto-clip: match-details unavailable — saving whole match without summary"
            );
            (
                "Full Match".to_string(),
                crate::library::db::NewClip::default(),
            )
        }
    };
    save_whole_session(
        &input.app,
        &input.session_path,
        &title,
        "Full Match",
        context,
    )
}

/// Persist a Highlights match whose details we couldn't fetch into the durable
/// pending store, to retry from [`reconcile_pending`] once the Riot client is
/// reachable again. Falls back to a whole-match save when there's no match id to
/// retry with, or the store itself is unavailable — footage is never discarded.
fn pend_for_retry(input: &CutInput, remote: &RemoteReady) -> Result<(), String> {
    let whole_match = || {
        save_whole_session(
            &input.app,
            &input.session_path,
            "Full Match",
            "Full Match",
            crate::library::db::NewClip::default(),
        )
    };
    let Some(match_id) = remote.match_id.as_deref() else {
        tracing::warn!("auto-clip: no match id captured — saving whole match instead of pending");
        return whole_match();
    };
    let entry = PendingMatch {
        session_file: String::new(), // filled in by pending::save
        timeline: input.timeline.clone(),
        frozen_spans: input.frozen_spans.clone(),
        anchors: input.anchors.clone(),
        fps: input.fps,
        game_start_ticks: input.game_start_ticks,
        puuid: remote.data.puuid.clone(),
        match_id: match_id.to_string(),
        region: remote.data.region.clone(),
        shard: remote.data.shard.clone(),
        client_version: remote.data.client_version.clone(),
        created_unix_ms: pending::now_unix_ms(),
        attempts: 0,
    };
    let stem = format!("{}_{}", sanitize_stem(match_id), pending::now_unix_ms());
    match pending::save(&input.app, &stem, &input.session_path, entry) {
        Ok(p) => {
            tracing::info!(
                "auto-clip: match-details unavailable — queued match {match_id} for retry ({})",
                p.display()
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!("auto-clip: could not queue pending match ({e}) — saving whole match");
            whole_match()
        }
    }
}

/// Retry every pending match once. Called periodically by the orchestrator when
/// the Riot client is up: rebuilds a fresh-token remote client, fetches the
/// (now-finalized) details and cuts the highlights, removing the entry on
/// success. Entries past [`MAX_PENDING_AGE_MS`] / [`MAX_PENDING_ATTEMPTS`] are
/// flushed to a whole-match clip so footage is never lost and the store can't
/// grow unbounded.
pub async fn reconcile_pending(app: AppHandle) {
    let entries = pending::list(&app);
    if entries.is_empty() {
        return;
    }

    // Flush aged-out entries first. This is purely local (copy footage → clips,
    // no Riot API), so it runs even when the client is closed — otherwise a match
    // whose client never reopens would keep its full MP4 on disk forever.
    let mut live = Vec::new();
    for (sidecar, entry) in entries {
        if pending_expired(&entry) {
            flush_expired(&app, &sidecar, &entry);
        } else {
            live.push((sidecar, entry));
        }
    }
    if live.is_empty() {
        return;
    }

    // The rest need the Riot client (fresh tokens + the details fetch). If it's
    // down, every entry fails identically — skip cheaply and retry next cycle.
    let local = match LocalClient::connect() {
        Ok(c) => c,
        Err(_) => return,
    };
    let ent = match local.entitlements().await {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!("reconcile: entitlements unavailable: {e}");
            return;
        }
    };

    for (sidecar, mut entry) in live {
        let remote = match RemoteClient::with_region_shard(
            &entry.region,
            &entry.shard,
            &ent.access_token,
            &ent.token,
            &entry.client_version,
        ) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("reconcile: build remote client: {e}");
                continue;
            }
        };

        let details = match remote.match_details(&entry.match_id).await {
            Ok(d) => d,
            Err(e) => {
                entry.attempts += 1;
                tracing::debug!(
                    "reconcile: match-details {} not ready (attempt {}): {e}",
                    entry.match_id,
                    entry.attempts
                );
                pending::update(&sidecar, &entry);
                continue;
            }
        };

        let Some(mp4) = pending::session_path(&app, &entry) else {
            pending::remove(&app, &sidecar, &entry);
            continue;
        };
        let params = CutParams {
            app: &app,
            session_path: &mp4,
            timeline: &entry.timeline,
            frozen_spans: &entry.frozen_spans,
            anchors: &entry.anchors,
            fps: entry.fps,
            game_start_ticks: entry.game_start_ticks,
            puuid: &entry.puuid,
        };
        match cut_highlights(&params, &details).await {
            Ok(()) => tracing::info!("auto-clip: reconciled pending match {}", entry.match_id),
            Err(e) => {
                // Details arrived but no highlight landed in this footage (e.g. a
                // pre-crash fragment) — keep the footage whole rather than
                // retrying forever.
                tracing::warn!(
                    "auto-clip: pending match {} produced no highlights ({e}) — saving whole match",
                    entry.match_id
                );
                let _ = save_whole_session(
                    &app,
                    &mp4,
                    "Full Match",
                    "Full Match",
                    crate::library::db::NewClip::default(),
                );
            }
        }
        pending::remove(&app, &sidecar, &entry);
    }
}

/// Give up on a pending match: preserve its footage as a whole-match clip and
/// drop the entry. Purely local (no Riot API), so it works with the client down.
fn flush_expired(app: &AppHandle, sidecar: &Path, entry: &PendingMatch) {
    tracing::warn!(
        "auto-clip: pending match {} expired ({} attempts) — saving whole match",
        entry.match_id,
        entry.attempts
    );
    if let Some(mp4) = pending::session_path(app, entry) {
        let _ = save_whole_session(
            app,
            &mp4,
            "Full Match",
            "Full Match",
            crate::library::db::NewClip::default(),
        );
    }
    pending::remove(app, sidecar, entry);
}

/// Whether a pending entry has exhausted its retry budget (age or attempts).
fn pending_expired(entry: &PendingMatch) -> bool {
    entry.attempts >= MAX_PENDING_ATTEMPTS
        || pending::now_unix_ms().saturating_sub(entry.created_unix_ms) >= MAX_PENDING_AGE_MS
}

/// Sanitize a match id into a safe filename stem (UUIDs already are; this guards
/// against anything unexpected).
fn sanitize_stem(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Derive + reconcile + cut the per-event highlights for a finished match, and
/// emit its summary. Shared by the live path and the pending reconciler.
async fn cut_highlights(cx: &CutParams<'_>, details: &MatchDetails) -> Result<(), String> {
    // Post-match summary (K/D/A, headshot %, agent, win/loss, title). Built +
    // emitted independently of clips so a match with no enabled highlights still
    // surfaces its result on the panel.
    let mut summary = summary::build_summary(details, cx.puuid);
    if let Some(name) = remote_api::fetch_agent_name(&summary.agent_id).await {
        summary.agent = name;
    }
    summary.title = summary.build_title();
    tracing::info!("auto-clip: match summary — {}", summary.title);
    let _ = cx.app.emit(events::MATCH_SUMMARY, &summary);

    let toggles = load_toggles(cx.app);
    let timings = load_timings(cx.app);
    let events = reconcile::derive_events(details, cx.puuid, &toggles);
    if events.is_empty() {
        tracing::info!("auto-clip: no enabled highlights in this match");
        return Ok(());
    }

    // Skirmish applies a −27 s match-start offset (Medal's
    // `ValorantPostMatchHandler`); apply it to both the calibrated anchor and the
    // game-start fallback so every event shifts consistently.
    let skirmish_offset = if summary.mode == "Skirmish" {
        -27 * TICKS_PER_SECOND
    } else {
        0
    };
    // Medal-faithful single match-start calibration from the logged round
    // anchors (falls back to the match-found anchor + per-event game time).
    let match_start =
        reconcile::calibrate_match_start(&events, cx.anchors).map(|ms| ms + skirmish_offset);
    let game_start_ticks = cx.game_start_ticks + skirmish_offset;
    let fps = cx.fps.max(1);
    let max_len_pts = MAX_AUTOCLIP_SECS * fps as i64;
    let place_tol = PLACEMENT_TOL_SECS * TICKS_PER_SECOND;

    // Reconcile each event to a session PTS span and build its padded window.
    // `placed` (window_start, window_end, kind) drives clip windowing + tagging;
    // `marks_all` (pts, kind) collects every seek-bar marker — one per kill of a
    // multi-kill, one per clutch kill, else the single moment — so the bar shows
    // where each action actually happened, not just the clip anchor.
    let mut placed: Vec<(i64, i64, EventKind)> = Vec::new();
    let mut marks_all: Vec<(i64, EventKind)> = Vec::new();
    let mut dropped = 0usize;
    for e in &events {
        // Reconcile the event's [first-action, last-action] span to PTS. A
        // multi-kill spans first→last so the whole sequence is captured, not just
        // a fixed pad around the last kill; single-moment events have a zero-width
        // span (start == end). An event whose anchor lands outside the recording
        // (app opened mid-game, or recording stopped early) is dropped here rather
        // than clamped onto a file end — see `event_span_pts`.
        let Some((start_pts, end_pts)) = reconcile::event_span_pts(
            e,
            match_start,
            cx.anchors,
            Some(game_start_ticks),
            cx.timeline,
            place_tol,
        ) else {
            dropped += 1;
            continue;
        };
        // Per-event clip window (Outplayed's "Events timing"): each kind has its
        // own before/after padding instead of one global pad.
        let t = timings.for_kind(e.kind);
        let (s, end) = reconcile::clip_window_span(start_pts, end_pts, t.before, t.after, fps);
        let end = end.min(s + max_len_pts);
        placed.push((s, end, e.kind));
        // Reconcile every marker moment of this event independently (each kill of
        // a multi-kill, each clutch kill, or the single anchor) and clamp it into
        // the event's own window. Falls back to the anchor if none reconcile.
        let mut any = false;
        for mk in &e.marks {
            if let Some(p) = reconcile::moment_pts(
                mk,
                e.round,
                match_start,
                cx.anchors,
                Some(game_start_ticks),
                cx.timeline,
            ) {
                marks_all.push((p.clamp(s, end), mk.kind));
                any = true;
            }
        }
        if !any {
            marks_all.push((end_pts.clamp(s, end), e.kind));
        }
    }
    if dropped > 0 {
        tracing::info!(
            "auto-clip: dropped {dropped}/{} event(s) outside the recorded window \
             (occurred before recording started or after it stopped — likely opened mid-game)",
            events.len()
        );
    }
    if placed.is_empty() {
        return Err("no events could be reconciled to the session timeline".into());
    }

    // Hand the placed windows to the shared cut tail (merge overlapping windows
    // → skip mostly-frozen ones → stream-copy each out → register). `max_after`
    // sizes the merge tolerance (the widest after-pad among enabled kinds).
    let (_, max_after) = timings.max_pad(&toggles);
    crate::games::recording::cut_placed_windows(
        &crate::games::recording::CutWindows {
            app: cx.app,
            session_path: cx.session_path,
            frozen_spans: cx.frozen_spans,
            fps,
            max_clip_secs: MAX_AUTOCLIP_SECS,
            merge_after_secs: max_after,
            game_label: "Valorant",
            title_suffix: &summary.agent,
            clip_context: summary.clip_context(),
        },
        &placed,
        &marks_all,
    );
    Ok(())
}

/// Fetch `match-details`, retrying while Riot finalizes the match (6 × 20 s).
/// Refreshes the RSO token from the local API before each attempt — it expires
/// over a long match and a stale token returns 400 BAD_CLAIMS (Medal refreshes
/// before every remote call for the same reason).
async fn fetch_match_details_retry(
    remote: &RemoteClient,
    local: Option<&LocalClient>,
    match_id: &str,
) -> Option<MatchDetails> {
    for attempt in 0..MATCH_DETAILS_ATTEMPTS {
        if let Some(l) = local {
            if let Err(e) = remote.refresh_tokens(l).await {
                tracing::warn!(
                    "auto-clip: token refresh failed (attempt {}): {e}",
                    attempt + 1
                );
            }
        }
        match remote.match_details(match_id).await {
            Ok(d) => return Some(d),
            Err(e) => tracing::warn!("auto-clip: match-details attempt {}: {e}", attempt + 1),
        }
        if attempt + 1 < MATCH_DETAILS_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_secs(MATCH_DETAILS_DELAY_SECS)).await;
        }
    }
    None
}

/// Read the user's event toggles from settings (defaults match Medal's
/// `events.json` if settings are unavailable).
fn load_toggles(app: &AppHandle) -> EventToggles {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.events))
        .unwrap_or_default()
}

/// Read the user's per-event clip windows (Outplayed "Events timing"), falling
/// back to the default table when settings are unavailable.
fn load_timings(app: &AppHandle) -> EventTimings {
    app.try_state::<SettingsState>()
        .and_then(|s| s.0.lock().ok().map(|g| g.event_timings))
        .unwrap_or_default()
}

// Whole-session save now lives in the shared recording layer; re-exported so the
// `cut::save_whole_session` call sites (here + the integration loop) keep working.
// The window/marker tests that lived here moved with that logic to
// `crate::games::recording`.
pub use crate::games::recording::save_whole_session;
