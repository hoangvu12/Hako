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

use std::path::PathBuf;

use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{self, SettingsState};
use crate::core::clock::TICKS_PER_SECOND;
use crate::events;
use crate::settings::AutoCaptureMode;
use crate::valorant::local_api::LocalClient;
use crate::valorant::model::{EventKind, MatchDetails};
use crate::valorant::reconcile::{
    self, EventTimings, EventToggles, RoundAnchor, TimelineIndex,
};
use crate::valorant::remote_api::{self, RemoteClient};
use crate::valorant::service::{self, SessionData};
use crate::valorant::summary;

/// `MaxAutoClipLength` — clamp each merged window to 120 s.
const MAX_AUTOCLIP_SECS: i64 = 120;
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
        tracing::warn!("auto-clip: could not resolve current match id; details fetch will be skipped");
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
    let remote = input.remote.as_ref().ok_or("no remote API (bootstrap failed)")?;
    let match_id = remote.match_id.as_deref().ok_or("no match id captured during the match")?;

    // A local-API handle to refresh the RSO token before each match-details
    // attempt — it expires during the match (a stale one returns 400 BAD_CLAIMS).
    let local = LocalClient::connect().ok();
    if local.is_none() {
        tracing::warn!("auto-clip: local API unavailable — can't refresh the match-details token");
    }
    let details = fetch_match_details_retry(&remote.data.remote, local.as_ref(), match_id)
        .await
        .ok_or("match-details never became available")?;

    // Post-match summary (K/D/A, headshot %, agent, win/loss, title). Built +
    // emitted independently of clips so a match with no enabled highlights still
    // surfaces its result on the panel.
    let mut summary = summary::build_summary(&details, &remote.data.puuid);
    if let Some(name) = remote_api::fetch_agent_name(&summary.agent_id).await {
        summary.agent = name;
    }
    summary.title = summary.build_title();
    tracing::info!("auto-clip: match summary — {}", summary.title);
    let _ = input.app.emit(events::MATCH_SUMMARY, &summary);

    // Full-match mode keeps the whole session as a single clip — no event
    // derivation or cutting. The summary above still fires for the panel.
    if input.mode == AutoCaptureMode::FullMatch {
        let title = if summary.title.is_empty() {
            "Full Match".to_string()
        } else {
            summary.title.clone()
        };
        return save_whole_session(&input.app, &input.session_path, &title, "Full Match");
    }

    let toggles = load_toggles(&input.app);
    let timings = load_timings(&input.app);
    let events = reconcile::derive_events(&details, &remote.data.puuid, &toggles);
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
        reconcile::calibrate_match_start(&events, &input.anchors).map(|ms| ms + skirmish_offset);
    let game_start_ticks = input.game_start_ticks + skirmish_offset;
    let fps = input.fps.max(1);
    let max_len_pts = MAX_AUTOCLIP_SECS * fps as i64;

    // Reconcile each event to a session PTS span and build its padded window.
    let mut placed: Vec<(i64, i64, EventKind)> = Vec::new(); // (start_pts, end_pts, kind)
    for e in &events {
        // Reconcile the event's [first-action, last-action] span to PTS. A
        // multi-kill spans first→last so the whole sequence is captured, not just
        // a fixed pad around the last kill; single-moment events have a zero-width
        // span (start == end).
        let Some((start_pts, end_pts)) = reconcile::event_span_pts(
            e,
            match_start,
            &input.anchors,
            Some(game_start_ticks),
            &input.timeline,
        ) else {
            continue;
        };
        // Per-event clip window (Outplayed's "Events timing"): each kind has its
        // own before/after padding instead of one global pad.
        let t = timings.for_kind(e.kind);
        let (s, end) = reconcile::clip_window_span(start_pts, end_pts, t.before, t.after, fps);
        let end = end.min(s + max_len_pts);
        placed.push((s, end, e.kind));
    }
    if placed.is_empty() {
        return Err("no events could be reconciled to the session timeline".into());
    }

    // Merge windows whose padding nearly touches (Medal `OverlapMergeGrouper`,
    // tol = the widest after-pad among enabled kinds so near events still fuse).
    let (_, max_after) = timings.max_pad(&toggles);
    let tol_pts = max_after.max(1) as i64 * fps as i64;
    let windows: Vec<(i64, i64)> = placed.iter().map(|&(s, e, _)| (s, e)).collect();
    let merged = reconcile::merge_windows_tol(windows, tol_pts);

    tracing::info!(
        "auto-clip: {} event(s) → {} clip(s)",
        placed.len(),
        merged.len()
    );

    let mut cut = 0usize;
    let mut skipped_frozen = 0usize;
    for (s, e) in merged {
        // Re-clamp the merged span and pick the strongest event kind inside it.
        let end = e.min(s + max_len_pts);
        let kind = dominant_kind(&placed, s, end).unwrap_or(EventKind::Kill);
        let start_sec = s as f64 / fps as f64;
        let end_sec = end as f64 / fps as f64;
        if end_sec <= start_sec {
            continue;
        }

        // Skip a clip whose window was mostly frozen (game minimized / stale
        // swapchain) — it would be a dead, single-frame clip. Both the window and
        // the spans are in session-PTS (1/fps) units, so they compare directly.
        let span_pts = end - s;
        let frozen_pts = frozen_overlap(&input.frozen_spans, s, end);
        if span_pts > 0 && frozen_pts * 2 > span_pts {
            tracing::warn!(
                "auto-clip: skipping {start_sec:.1}-{end_sec:.1}s — {}% frozen \
                 (game minimized/not presenting during the match)",
                (frozen_pts * 100 / span_pts).min(100)
            );
            skipped_frozen += 1;
            continue;
        }

        let out = match commands::auto_clip_output_path(&input.app) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("auto-clip: output path: {e}");
                continue;
            }
        };
        // Stream-copy the window out of the session file (no re-encode).
        match crate::library::trim::trim_clip(&input.session_path, &out, start_sec, end_sec, false) {
            Ok(res) => {
                // Tag the clip with its event + the match's agent when known
                // (e.g. "Ace — Jett"), else just the event + duration.
                let title = if summary.agent.is_empty() {
                    format!("{} — {:.0}s", kind.label(), end_sec - start_sec)
                } else {
                    format!("{} — {}", kind.label(), summary.agent)
                };
                if let Err(err) = commands::finalize_auto_clip(
                    &input.app,
                    out,
                    title,
                    kind.label(),
                    res.width,
                    res.height,
                    res.duration_secs,
                ) {
                    tracing::warn!("auto-clip: library insert failed: {err}");
                } else {
                    cut += 1;
                }
            }
            Err(err) => tracing::warn!("auto-clip: cut {start_sec:.1}-{end_sec:.1}s failed: {err}"),
        }
    }
    tracing::info!("auto-clip: wrote {cut} clip(s) to the library");
    if skipped_frozen > 0 {
        // Surface it honestly: the user alt-tabbed / switched display mode and the
        // injection-path capture froze, so some highlights had no live footage.
        let msg = if cut == 0 {
            format!(
                "Skipped {skipped_frozen} clip(s) — Valorant was minimized or not \
                 rendering for the match, so there was no live gameplay to clip."
            )
        } else {
            format!("Skipped {skipped_frozen} clip(s) — the game was minimized during those moments.")
        };
        tracing::warn!("auto-clip: {msg}");
        let _ = input.app.emit(events::RECORDER_ERROR, &msg);
    }
    Ok(())
}

/// Total overlap (session PTS) between clip window `[s, end)` and the frozen
/// spans. The spans are non-overlapping and ascending, so this is a simple sum of
/// per-span intersections.
fn frozen_overlap(spans: &[(i64, i64)], s: i64, end: i64) -> i64 {
    spans
        .iter()
        .map(|&(a, b)| (end.min(b) - s.max(a)).max(0))
        .sum()
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
                tracing::warn!("auto-clip: token refresh failed (attempt {}): {e}", attempt + 1);
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

/// The strongest highlight kind whose anchor falls inside `[start, end]` (so the
/// merged clip is tagged by its best moment, e.g. an Ace over a stray Kill).
fn dominant_kind(placed: &[(i64, i64, EventKind)], start: i64, end: i64) -> Option<EventKind> {
    placed
        .iter()
        .filter(|&&(s, _, _)| s >= start - 1 && s <= end)
        .map(|&(_, _, k)| k)
        .max_by_key(|k| kind_priority(*k))
}

/// Tag priority: the headline moments (Victory/Ace/Clutch) outrank multi-kills,
/// which outrank single kills, spike plays, deaths, and assists.
fn kind_priority(k: EventKind) -> u8 {
    match k {
        EventKind::Victory => 11,
        EventKind::Ace => 10,
        EventKind::Clutch => 9,
        EventKind::QuadraKill => 8,
        EventKind::TripleKill => 7,
        EventKind::Knife => 6,
        EventKind::DoubleKill => 5,
        EventKind::SpikeDefused => 4,
        EventKind::SpikeDetonated => 3,
        EventKind::Kill => 2,
        EventKind::Assist => 1,
        EventKind::Death => 0,
    }
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

/// Save a whole Mode-B session file as a single library clip (FullMatch / Session
/// modes): stream-copy it into the clips dir (which also probes its real
/// dimensions + duration), tag it with `event`, and register it. The session temp
/// is dropped by the caller.
pub fn save_whole_session(
    app: &AppHandle,
    session_path: &std::path::Path,
    title: &str,
    event: &str,
) -> Result<(), String> {
    /// Upper bound on a session's length (s) — trim copies to EOF within it.
    const WHOLE_FILE_SECS: f64 = 24.0 * 60.0 * 60.0;
    let out = commands::auto_clip_output_path(app)?;
    let res = crate::library::trim::trim_clip(session_path, &out, 0.0, WHOLE_FILE_SECS, false)
        .map_err(|e| format!("whole-session copy failed: {e}"))?;
    commands::finalize_auto_clip(
        app,
        out,
        title.to_string(),
        event,
        res.width,
        res.height,
        res.duration_secs,
    )?;
    tracing::info!("auto-clip: saved {event} ({:.0}s)", res.duration_secs);
    Ok(())
}
