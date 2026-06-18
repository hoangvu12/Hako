//! Kill timestamp → session-file PTS reconciliation + event derivation.
//!
//! Two pure stages, both unit-tested without a live game:
//!
//! 1. **Derive** ([`derive_events`]) — from post-match `MatchDetails`, compute
//!    our highlight events: one multi-kill tier per round (Kill/2K/3K/4K/Ace
//!    anchored at the *last* kill but carrying a `lead_in` back to the *first*
//!    kill so the clip spans the whole sequence), plus Knife / Death / Assist.
//!    Filtered by [`EventToggles`].
//! 2. **Reconcile** ([`reconcile_to_pts`]) — map an event's match-relative time
//!    to a session-file PTS. We logged each round's start wall-clock live (from
//!    presence score changes); `kill_wall ≈ round_start_wall +
//!    timeSinceRoundStartMillis`, then [`TimelineIndex`] maps wall-clock → PTS.
//!    Falls back to the game-start anchor + `timeSinceGameStartMillis` when a
//!    per-round anchor is missing (coarser, still fine under padding).
//!
//! Clip windows ([`clip_window`]) apply −before/+after padding and
//! [`merge_windows`] fuses overlapping highlights into one clip.

#![allow(dead_code)]

use crate::valorant::model::{EventKind, GameEvent, MatchDetails};

/// 100-ns ticks per millisecond (Riot times are ms; our clock is 100-ns ticks).
const TICKS_PER_MS: i64 = 10_000;

/// Per-event enable flags. Defaults match Medal's stored Valorant
/// config (`fW3AZxHf_c`): highlight-worthy on,
/// single Kill / 2K / Death / Assist off. The Outplayed-style additions
/// (victory / clutch / spike) default on — they're the headline moments.
///
/// New fields are additive (`#[serde(default)]`), so configs written before they
/// existed load forward-compatibly (the new toggles fall back to their defaults).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EventToggles {
    pub kill: bool,
    pub double_kill: bool,
    pub triple_kill: bool,
    pub quadra_kill: bool,
    pub ace: bool,
    pub knife: bool,
    pub death: bool,
    pub assist: bool,
    pub victory: bool,
    pub clutch: bool,
    pub spike_detonated: bool,
    pub spike_defused: bool,
}

impl Default for EventToggles {
    fn default() -> Self {
        EventToggles {
            kill: false,
            double_kill: false,
            triple_kill: true,
            quadra_kill: true,
            ace: true,
            knife: true,
            death: false,
            assist: false,
            victory: true,
            clutch: true,
            spike_detonated: true,
            spike_defused: true,
        }
    }
}

impl EventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Ace => self.ace,
            EventKind::Knife => self.knife,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            EventKind::Victory => self.victory,
            EventKind::Clutch => self.clutch,
            EventKind::SpikeDetonated => self.spike_detonated,
            EventKind::SpikeDefused => self.spike_defused,
        }
    }
}

/// Per-event clip padding (seconds before / after the moment). Outplayed's
/// "Events timing" advanced panel: each highlight kind gets its own window so a
/// spike plant (long fuse) keeps more lead-in than a snap kill.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for EventTiming {
    /// The neutral kill window; per-kind defaults override this in
    /// [`EventTimings::default`].
    fn default() -> Self {
        EventTiming { before: 8, after: 4 }
    }
}

impl EventTiming {
    const fn new(before: u32, after: u32) -> Self {
        EventTiming { before, after }
    }
}

/// Per-event clip windows. Field-per-kind (not a map) to match the rest of the
/// settings model and keep serde forward-compatible — an older config simply
/// gets every field's default. Defaults mirror Outplayed's shipped timings.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EventTimings {
    pub kill: EventTiming,
    pub double_kill: EventTiming,
    pub triple_kill: EventTiming,
    pub quadra_kill: EventTiming,
    pub ace: EventTiming,
    pub knife: EventTiming,
    pub death: EventTiming,
    pub assist: EventTiming,
    pub victory: EventTiming,
    pub clutch: EventTiming,
    pub spike_detonated: EventTiming,
    pub spike_defused: EventTiming,
}

impl Default for EventTimings {
    fn default() -> Self {
        EventTimings {
            kill: EventTiming::new(8, 4),
            double_kill: EventTiming::new(8, 4),
            triple_kill: EventTiming::new(10, 4),
            quadra_kill: EventTiming::new(10, 4),
            ace: EventTiming::new(12, 5),
            knife: EventTiming::new(8, 4),
            death: EventTiming::new(8, 4),
            assist: EventTiming::new(15, 4),
            victory: EventTiming::new(8, 5),
            clutch: EventTiming::new(12, 5),
            // The spike fuse is 45 s; lead in from the plant.
            spike_detonated: EventTiming::new(45, 10),
            spike_defused: EventTiming::new(25, 10),
        }
    }
}

impl EventTimings {
    /// The clip window for `kind`.
    pub fn for_kind(&self, kind: EventKind) -> EventTiming {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Ace => self.ace,
            EventKind::Knife => self.knife,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            EventKind::Victory => self.victory,
            EventKind::Clutch => self.clutch,
            EventKind::SpikeDetonated => self.spike_detonated,
            EventKind::SpikeDefused => self.spike_defused,
        }
    }

    /// The widest before/after across all *enabled* kinds — used to size the
    /// merge tolerance so two near events still fuse under the largest window.
    pub fn max_pad(&self, toggles: &EventToggles) -> (u32, u32) {
        let kinds = [
            EventKind::Kill,
            EventKind::DoubleKill,
            EventKind::TripleKill,
            EventKind::QuadraKill,
            EventKind::Ace,
            EventKind::Knife,
            EventKind::Death,
            EventKind::Assist,
            EventKind::Victory,
            EventKind::Clutch,
            EventKind::SpikeDetonated,
            EventKind::SpikeDefused,
        ];
        kinds
            .iter()
            .filter(|k| toggles.enabled(**k))
            .map(|k| self.for_kind(*k))
            .fold((0, 0), |(b, a), t| (b.max(t.before), a.max(t.after)))
    }
}

/// Derive our highlight events from a finished match, keeping only enabled kinds.
/// Events come back sorted by `time_since_game_start_millis`.
pub fn derive_events(details: &MatchDetails, puuid: &str, toggles: &EventToggles) -> Vec<GameEvent> {
    let mut events = Vec::new();
    // Our team id (for clutch round-win and the match Victory event). Empty if we
    // aren't in the players list — the team-dependent events then never fire.
    let our_team = details
        .players
        .iter()
        .find(|p| p.puuid == puuid)
        .map(|p| p.team_id.as_str())
        .unwrap_or("");

    for round in &details.round_results {
        // Our kills this round (we are the killer), sorted in time.
        let mut our_kills: Vec<_> = round
            .player_stats
            .iter()
            .filter(|ps| ps.puuid == puuid)
            .flat_map(|ps| ps.kills.iter())
            .filter(|k| k.killer == puuid)
            .collect();
        our_kills.sort_by_key(|k| k.time_since_round_start_millis);

        // One multi-kill tier event per round, anchored at the LAST kill with the
        // window reaching back to the FIRST so the *whole* sequence is captured.
        // (Medal spans first→last + padding via its OverlapMergeGrouper; a fixed
        // pad around just the last kill would drop the early kills of a spread-out
        // Ace.) `lead_in` = the first→last gap, same in game- and round-time.
        if let (Some(first), Some(last)) = (our_kills.first(), our_kills.last()) {
            let lead_in =
                last.time_since_round_start_millis - first.time_since_round_start_millis;
            events.push(GameEvent::span(
                EventKind::for_multikill(our_kills.len()),
                round.round_num,
                last.time_since_game_start_millis,
                last.time_since_round_start_millis,
                lead_in,
            ));
        }

        // Knife kills are their own highlight regardless of the round's tier.
        for k in our_kills.iter().filter(|k| k.is_knife()) {
            events.push(GameEvent::point(
                EventKind::Knife,
                round.round_num,
                k.time_since_game_start_millis,
                k.time_since_round_start_millis,
            ));
        }

        // Deaths (we are the victim) and assists (we assisted someone else).
        for ps in &round.player_stats {
            for k in &ps.kills {
                if k.victim == puuid {
                    events.push(GameEvent::point(
                        EventKind::Death,
                        round.round_num,
                        k.time_since_game_start_millis,
                        k.time_since_round_start_millis,
                    ));
                }
                if k.killer != puuid && k.assistants.iter().any(|a| a == puuid) {
                    events.push(GameEvent::point(
                        EventKind::Assist,
                        round.round_num,
                        k.time_since_game_start_millis,
                        k.time_since_round_start_millis,
                    ));
                }
            }
        }

        // Spike events (plant→detonate by us / defuse by us) and clutches need
        // the round's wall-clock origin to position events that aren't anchored
        // on one of our own kills; derive it from any kill in the round.
        let round_offset = round_game_offset(round);
        events.extend(derive_spike_events(round, puuid, round_offset));
        if let Some(c) = detect_clutch(details, round, puuid, our_team) {
            events.push(c);
        }
    }

    // Match Victory — one event at the final action of the match if our team won.
    if let Some(v) = detect_victory(details, our_team) {
        events.push(v);
    }

    events.retain(|e| toggles.enabled(e.kind));
    events.sort_by_key(|e| e.time_since_game_start_millis);
    events
}

/// `gameTime − roundTime` for a round (constant for every kill in it), i.e. the
/// round's start offset from match start in ms. `None` if the round has no kills
/// to read it from (then game-time-relative events in the round can't be timed).
fn round_game_offset(round: &crate::valorant::model::RoundResult) -> Option<i64> {
    round
        .player_stats
        .iter()
        .flat_map(|ps| ps.kills.iter())
        .map(|k| k.time_since_game_start_millis - k.time_since_round_start_millis)
        .next()
}

/// Spike highlights for a round: a [`EventKind::SpikeDetonated`] when *we* planted
/// and the round ended by detonation, and a [`EventKind::SpikeDefused`] when *we*
/// defused. Positioned at the detonation (plant + 45 s fuse) / defuse time.
/// Skipped when the round offset is unknown (can't place on the game clock).
fn derive_spike_events(
    round: &crate::valorant::model::RoundResult,
    puuid: &str,
    round_offset: Option<i64>,
) -> Vec<GameEvent> {
    /// Spike fuse length in ms (45 s).
    const SPIKE_FUSE_MS: i64 = 45_000;
    let Some(offset) = round_offset else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let code = round.round_result_code.as_str();

    if round.bomb_planter == puuid && code.eq_ignore_ascii_case("Detonate") {
        let round_ms = round.plant_round_time + SPIKE_FUSE_MS;
        out.push(GameEvent::point(
            EventKind::SpikeDetonated,
            round.round_num,
            offset + round_ms,
            round_ms,
        ));
    }
    if round.bomb_defuser == puuid && code.eq_ignore_ascii_case("Defuse") {
        let round_ms = round.defuse_round_time;
        out.push(GameEvent::point(
            EventKind::SpikeDefused,
            round.round_num,
            offset + round_ms,
            round_ms,
        ));
    }
    out
}

/// Detect a 1vX clutch: a round our team won in which we became the last player
/// alive on our team while ≥1 enemy still lived, then closed it out with ≥1 kill.
/// We simulate the round's alive sets from the full roster (`players[].team_id`)
/// and the kill timeline. Anchored at our clinching (last) kill of the round.
/// `None` if it wasn't a clutch (no team / the round wasn't ours / never 1vX).
fn detect_clutch(
    details: &MatchDetails,
    round: &crate::valorant::model::RoundResult,
    puuid: &str,
    our_team: &str,
) -> Option<GameEvent> {
    use std::collections::HashSet;
    if our_team.is_empty() || !round.winning_team.eq_ignore_ascii_case(our_team) {
        return None;
    }

    // Round rosters from the match-level player list (everyone starts the round
    // alive). A player with no team id is ignored.
    let mut alive_team: HashSet<&str> = details
        .players
        .iter()
        .filter(|p| p.team_id.eq_ignore_ascii_case(our_team) && !p.puuid.is_empty())
        .map(|p| p.puuid.as_str())
        .collect();
    let enemy_count = details
        .players
        .iter()
        .filter(|p| !p.team_id.is_empty() && !p.team_id.eq_ignore_ascii_case(our_team))
        .count();
    // We must actually be on the roster and not solo our own team.
    if !alive_team.contains(puuid) || alive_team.len() < 2 {
        return None;
    }

    let mut kills: Vec<&crate::valorant::model::Kill> = round
        .player_stats
        .iter()
        .flat_map(|ps| ps.kills.iter())
        .collect();
    kills.sort_by_key(|k| k.time_since_round_start_millis);

    let mut enemy_dead = 0usize;
    let mut became_alone = false; // we are the last teammate standing
    let mut our_kills_after_alone = 0usize;
    // First and last (clinching) of our kills once the clutch is on — the clip
    // spans the whole 1vX, not just a fixed pad around the final kill (same
    // reasoning as the multi-kill span: a drawn-out clutch would otherwise lose
    // its opening kills).
    let mut first_clutch: Option<&crate::valorant::model::Kill> = None;
    let mut clinch: Option<&crate::valorant::model::Kill> = None;

    for k in &kills {
        // Tally an enemy elimination (victim is on the other team).
        let victim_is_enemy = !alive_team.contains(k.victim.as_str())
            && details.players.iter().any(|p| {
                p.puuid == k.victim && !p.team_id.is_empty() && !p.team_id.eq_ignore_ascii_case(our_team)
            });
        if victim_is_enemy {
            enemy_dead += 1;
        }
        // Remove a fallen teammate from the alive set.
        alive_team.remove(k.victim.as_str());

        if became_alone && k.killer == puuid {
            our_kills_after_alone += 1;
            first_clutch = first_clutch.or(Some(k));
            clinch = Some(k);
        }
        // The instant our side is reduced to exactly us, with enemies remaining,
        // the clutch is on.
        if !became_alone && alive_team.len() == 1 && alive_team.contains(puuid) {
            became_alone = true;
            let enemies_alive = enemy_count.saturating_sub(enemy_dead);
            if enemies_alive == 0 {
                return None; // no one left to clutch against
            }
            // A kill that lands exactly as we become alone (it's what made us
            // alone only if it was a teammate death; our own kill still counts).
            if k.killer == puuid {
                our_kills_after_alone += 1;
                first_clutch = first_clutch.or(Some(k));
                clinch = Some(k);
            }
        }
    }

    let clinch = clinch?;
    if !became_alone || our_kills_after_alone == 0 {
        return None;
    }
    // Span from our first clutch kill to the clinch (lead_in identical in game-
    // and round-time). Falls back to a point if somehow only the clinch is set.
    let lead_in = first_clutch
        .map(|f| clinch.time_since_round_start_millis - f.time_since_round_start_millis)
        .unwrap_or(0);
    Some(GameEvent::span(
        EventKind::Clutch,
        round.round_num,
        clinch.time_since_game_start_millis,
        clinch.time_since_round_start_millis,
        lead_in,
    ))
}

/// The match Victory event: emitted when our team is flagged `won`, anchored at
/// the match's final recorded action (the latest kill across all rounds) so the
/// clip lands on the match point. `None` if we didn't win or there are no kills.
fn detect_victory(details: &MatchDetails, our_team: &str) -> Option<GameEvent> {
    if our_team.is_empty() {
        return None;
    }
    let won = details
        .teams
        .iter()
        .find(|t| t.team_id.eq_ignore_ascii_case(our_team))
        .map(|t| t.won)
        .unwrap_or(false);
    if !won {
        return None;
    }
    // The last action of the match across every round.
    let last = details
        .round_results
        .iter()
        .flat_map(|r| r.player_stats.iter().flat_map(|ps| ps.kills.iter()).map(move |k| (r.round_num, k)))
        .max_by_key(|(_, k)| k.time_since_game_start_millis)?;
    Some(GameEvent::point(
        EventKind::Victory,
        last.0,
        last.1.time_since_game_start_millis,
        last.1.time_since_round_start_millis,
    ))
}

/// A round's start wall-clock, logged live when the presence score changes.
#[derive(Debug, Clone, Copy)]
pub struct RoundAnchor {
    pub round: i32,
    pub start_wallclock_ticks: i64,
}

/// Maps wall-clock (100-ns ticks) ↔ session-file PTS. Built by the Mode-B
/// session writer as it muxes packets: each entry pairs a packet's capture
/// timestamp with its PTS. Lookups linearly interpolate and clamp to the ends.
#[derive(Debug, Clone, Default)]
pub struct TimelineIndex {
    /// `(wallclock_ticks, pts)` pairs, kept sorted by wall-clock.
    samples: Vec<(i64, i64)>,
}

impl TimelineIndex {
    pub fn new() -> Self {
        TimelineIndex { samples: Vec::new() }
    }

    /// Record a sample. Kept sorted; out-of-order pushes are inserted in place.
    pub fn push(&mut self, wallclock_ticks: i64, pts: i64) {
        match self.samples.last() {
            Some(&(w, _)) if wallclock_ticks >= w => self.samples.push((wallclock_ticks, pts)),
            _ => {
                let idx = self
                    .samples
                    .partition_point(|&(w, _)| w < wallclock_ticks);
                self.samples.insert(idx, (wallclock_ticks, pts));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// PTS at `wallclock_ticks`, linearly interpolated between bracketing
    /// samples and clamped to the recorded range. `None` if no samples.
    pub fn pts_at(&self, wallclock_ticks: i64) -> Option<i64> {
        if self.samples.is_empty() {
            return None;
        }
        let first = self.samples[0];
        let last = self.samples[self.samples.len() - 1];
        if wallclock_ticks <= first.0 {
            return Some(first.1);
        }
        if wallclock_ticks >= last.0 {
            return Some(last.1);
        }
        let i = self.samples.partition_point(|&(w, _)| w <= wallclock_ticks);
        let (w0, p0) = self.samples[i - 1];
        let (w1, p1) = self.samples[i];
        if w1 == w0 {
            return Some(p0);
        }
        let frac = (wallclock_ticks - w0) as f64 / (w1 - w0) as f64;
        Some(p0 + ((p1 - p0) as f64 * frac).round() as i64)
    }
}

/// Estimate an event's wall-clock (100-ns ticks): prefer the per-round anchor
/// (`round_start + timeSinceRoundStart`), else the game-start anchor
/// (`game_start + timeSinceGameStart`).
pub fn event_wallclock(
    event: &GameEvent,
    anchors: &[RoundAnchor],
    game_start_ticks: Option<i64>,
) -> Option<i64> {
    if let Some(a) = anchors.iter().find(|a| a.round == event.round) {
        return Some(a.start_wallclock_ticks + event.time_since_round_start_millis * TICKS_PER_MS);
    }
    game_start_ticks.map(|g| g + event.time_since_game_start_millis * TICKS_PER_MS)
}

/// Medal-faithful single match-start calibration. For the first event whose
/// round has a known start anchor (from the `ShooterGame.log` round-end + buy
/// phase), derive the match's wall-clock origin:
///
/// `matchStart = roundStart(r) + kill.roundTime − kill.gameTime`
///
/// (since `gameTime − roundTime = roundStart − matchStart` for any kill in r).
/// `None` if no event's round has an anchor — caller falls back to the
/// game-start anchor. Events are assumed sorted by `time_since_game_start_millis`.
pub fn calibrate_match_start(events: &[GameEvent], anchors: &[RoundAnchor]) -> Option<i64> {
    for e in events {
        if let Some(a) = anchors.iter().find(|a| a.round == e.round) {
            return Some(
                a.start_wallclock_ticks
                    + e.time_since_round_start_millis * TICKS_PER_MS
                    - e.time_since_game_start_millis * TICKS_PER_MS,
            );
        }
    }
    None
}

/// Position an event on the wall-clock from a calibrated match start, the way
/// Medal does for every event: `eventWall = matchStart + gameTime`.
pub fn event_wallclock_from_match_start(event: &GameEvent, match_start_ticks: i64) -> i64 {
    match_start_ticks + event.time_since_game_start_millis * TICKS_PER_MS
}

/// Reconcile an event to a session-file PTS via the timeline index.
pub fn reconcile_to_pts(
    event: &GameEvent,
    anchors: &[RoundAnchor],
    game_start_ticks: Option<i64>,
    timeline: &TimelineIndex,
) -> Option<i64> {
    let wall = event_wallclock(event, anchors, game_start_ticks)?;
    timeline.pts_at(wall)
}

/// Reconcile an event to its `[start_pts, end_pts]` clip-window anchors: the
/// window ends at the event's anchor (last action) and starts at the sequence's
/// first action (`anchor − lead_in`). Both endpoints map through the timeline
/// independently, so a frozen gap *between* the first and last action is handled
/// correctly. Prefers the Medal `matchStart` calibration when `match_start` is
/// `Some`, else the per-round / game-start anchor (mirrors [`reconcile_to_pts`]).
/// `None` if the anchor can't be placed on the timeline.
pub fn event_span_pts(
    event: &GameEvent,
    match_start: Option<i64>,
    anchors: &[RoundAnchor],
    game_start_ticks: Option<i64>,
    timeline: &TimelineIndex,
) -> Option<(i64, i64)> {
    let anchor_wall = match match_start {
        Some(ms) => event_wallclock_from_match_start(event, ms),
        None => event_wallclock(event, anchors, game_start_ticks)?,
    };
    let start_wall = anchor_wall - event.lead_in_millis * TICKS_PER_MS;
    let start_pts = timeline.pts_at(start_wall)?;
    let end_pts = timeline.pts_at(anchor_wall)?;
    Some((start_pts, end_pts))
}

/// Clip window `[center − before, center + after]` in PTS units, clamped to ≥ 0.
pub fn clip_window(center_pts: i64, pad_before_secs: u32, pad_after_secs: u32, fps: u32) -> (i64, i64) {
    clip_window_span(center_pts, center_pts, pad_before_secs, pad_after_secs, fps)
}

/// Clip window for a reconciled `[start_pts, end_pts]` span: pad `before` ahead
/// of the start (the sequence's first action) and `after` past the end (its
/// last). `[start − before, end + after]`, clamped to ≥ 0. A single-moment event
/// passes `start == end` (see [`clip_window`]).
pub fn clip_window_span(
    start_pts: i64,
    end_pts: i64,
    pad_before_secs: u32,
    pad_after_secs: u32,
    fps: u32,
) -> (i64, i64) {
    let fps = fps.max(1) as i64;
    let start = (start_pts - pad_before_secs as i64 * fps).max(0);
    let end = end_pts + pad_after_secs as i64 * fps;
    (start, end)
}

/// Merge overlapping/adjacent clip windows into one (multi-kills clustered in
/// time become a single clip). Input order-independent; output sorted.
pub fn merge_windows(windows: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    merge_windows_tol(windows, 0)
}

/// Like [`merge_windows`] but also fuses windows separated by a gap of up to
/// `tol_pts` (in PTS units). Medal's `OverlapMergeGrouper` merges events whose
/// windows are within `EventWindow` (10 s for Valorant) of each other, so two
/// near-but-not-touching highlights become one clip rather than two with
/// overlapping padding.
pub fn merge_windows_tol(mut windows: Vec<(i64, i64)>, tol_pts: i64) -> Vec<(i64, i64)> {
    if windows.is_empty() {
        return windows;
    }
    windows.sort_by_key(|w| w.0);
    let mut merged = vec![windows[0]];
    for &(s, e) in &windows[1..] {
        let last = merged.last_mut().unwrap();
        if s <= last.1 + tol_pts {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::valorant::model::*;

    fn kill(killer: &str, victim: &str, round_ms: i64, game_ms: i64) -> Kill {
        Kill {
            time_since_game_start_millis: game_ms,
            time_since_round_start_millis: round_ms,
            killer: killer.into(),
            victim: victim.into(),
            finishing_damage: FinishingDamage::default(),
            assistants: Vec::new(),
        }
    }

    fn round(num: i32, our: &str, kills: Vec<Kill>) -> RoundResult {
        // All kills attributed under our playerStats for simplicity; derive_events
        // filters by killer/victim so attribution bucket doesn't matter here.
        RoundResult {
            round_num: num,
            player_stats: vec![PlayerRoundStats {
                puuid: our.into(),
                kills,
                damage: Vec::new(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn derives_ace_anchored_at_last_kill_spanning_back_to_first() {
        let me = "ME";
        // Five kills at 1,2,3,4,5 s into the round.
        let r = round(
            0,
            me,
            (0..5)
                .map(|i| kill(me, "victim", 1000 + i * 1000, 1000 + i * 1000))
                .collect(),
        );
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![],
            teams: vec![],
            round_results: vec![r],
        };
        let ev = derive_events(&details, me, &EventToggles::default());
        // One Ace event (default toggles: Ace on), anchored at the last kill...
        let aces: Vec<_> = ev.iter().filter(|e| e.kind == EventKind::Ace).collect();
        assert_eq!(aces.len(), 1);
        assert_eq!(aces[0].time_since_round_start_millis, 5000);
        // ...but reaching back to the first kill (5 s − 1 s = 4 s lead-in) so the
        // window pads outward from the first kill, not just the last.
        assert_eq!(aces[0].lead_in_millis, 4000);
    }

    #[test]
    fn toggles_filter_single_kills_off_by_default() {
        let me = "ME";
        let r = round(0, me, vec![kill(me, "v", 1000, 1000)]);
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![],
            teams: vec![],
            round_results: vec![r],
        };
        // Default: single Kill is OFF → no events.
        assert!(derive_events(&details, me, &EventToggles::default()).is_empty());
        // Turn it on → one Kill event.
        let mut t = EventToggles::default();
        t.kill = true;
        let ev = derive_events(&details, me, &t);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, EventKind::Kill);
    }

    #[test]
    fn detects_knife_death_and_assist() {
        let me = "ME";
        let mut knife = kill(me, "v", 2000, 2000);
        knife.finishing_damage.damage_type = "Melee".into();
        let death = kill("enemy", me, 5000, 5000);
        let mut assist = kill("mate", "v2", 7000, 7000);
        assist.assistants = vec![me.into()];
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![],
            teams: vec![],
            round_results: vec![round(0, me, vec![knife, death, assist])],
        };
        let mut t = EventToggles::default();
        t.death = true;
        t.assist = true;
        let ev = derive_events(&details, me, &t);
        assert!(ev.iter().any(|e| e.kind == EventKind::Knife));
        assert!(ev.iter().any(|e| e.kind == EventKind::Death));
        assert!(ev.iter().any(|e| e.kind == EventKind::Assist));
    }

    #[test]
    fn timeline_interpolates_and_clamps() {
        let mut t = TimelineIndex::new();
        t.push(0, 0);
        t.push(10_000_000, 60); // 1 s → PTS 60 (60 fps)
        assert_eq!(t.pts_at(-100), Some(0)); // clamp low
        assert_eq!(t.pts_at(5_000_000), Some(30)); // midpoint
        assert_eq!(t.pts_at(99_000_000), Some(60)); // clamp high
    }

    #[test]
    fn reconciles_via_round_anchor() {
        let me = "ME";
        let ev = GameEvent::point(EventKind::Ace, 3, 0, 2000); // 2 s into the round
        let anchors = [RoundAnchor {
            round: 3,
            start_wallclock_ticks: 50_000_000, // round started at t=5 s
        }];
        // kill_wall = 50_000_000 + 2000 ms*10_000 = 70_000_000 ticks (7 s).
        let mut tl = TimelineIndex::new();
        tl.push(0, 0);
        tl.push(100_000_000, 600); // 10 s → PTS 600
        let pts = reconcile_to_pts(&ev, &anchors, None, &tl).unwrap();
        assert_eq!(pts, 420); // 7 s → PTS 420
        let _ = me;
    }

    #[test]
    fn calibrates_match_start_medal_formula() {
        // Round 3 starts at wall 50 s. A kill 2 s into round 3 is 120 s into the
        // game ⇒ matchStart = 50s + 2s − 120s = −68s (game began 68 s before the
        // round-3 start anchor). Then any event maps via matchStart + gameTime.
        let ev = vec![GameEvent::point(EventKind::Ace, 3, 120_000, 2_000)];
        let anchors = [RoundAnchor {
            round: 3,
            start_wallclock_ticks: 50 * 10_000_000,
        }];
        let ms = calibrate_match_start(&ev, &anchors).unwrap();
        assert_eq!(ms, (50 + 2 - 120) * 10_000_000);
        // The event is 2 s into round 3 ⇒ its wall-clock = roundStart + roundTime
        // = 50 s + 2 s = 52 s (matchStart −68 s + gameTime 120 s).
        assert_eq!(
            event_wallclock_from_match_start(&ev[0], ms),
            52 * 10_000_000
        );
    }

    #[test]
    fn calibration_none_without_matching_anchor() {
        let ev = vec![GameEvent::point(EventKind::Ace, 5, 1_000, 100)];
        let anchors = [RoundAnchor {
            round: 2,
            start_wallclock_ticks: 0,
        }];
        assert!(calibrate_match_start(&ev, &anchors).is_none());
    }

    #[test]
    fn windows_clamp_and_merge() {
        let (s, e) = clip_window(60, 8, 4, 60); // center 1 s, −8/+4
        assert_eq!((s, e), (0, 60 + 240)); // start clamped to 0
        let merged = merge_windows(vec![(0, 300), (250, 500), (1000, 1200)]);
        assert_eq!(merged, vec![(0, 500), (1000, 1200)]);
    }

    #[test]
    fn span_window_pads_outward_from_first_and_last() {
        // A multi-kill spanning PTS 600..1200 (10..20 s at 60 fps) pads 8 s before
        // the first action and 4 s after the last → [600−480, 1200+240].
        let (s, e) = clip_window_span(600, 1200, 8, 4, 60);
        assert_eq!((s, e), (600 - 480, 1200 + 240));
        // A zero-width span behaves exactly like the point `clip_window`.
        assert_eq!(clip_window_span(600, 600, 8, 4, 60), clip_window(600, 8, 4, 60));
        // Start still clamps to ≥ 0.
        assert_eq!(clip_window_span(60, 300, 8, 4, 60).0, 0);
    }

    #[test]
    fn event_span_reconciles_first_and_last_action() {
        // Ace anchored at 20 s into the game (game-start fallback, no round anchor),
        // reaching back 6 s to its first kill. Timeline is linear 60 fps from 0.
        let ev = GameEvent::span(EventKind::Ace, 0, 20_000, 20_000, 6_000);
        let mut tl = TimelineIndex::new();
        tl.push(0, 0);
        tl.push(300_000_000, 1800); // 30 s → PTS 1800 (60 fps)
        let (start_pts, end_pts) =
            event_span_pts(&ev, None, &[], Some(0), &tl).expect("span reconciles");
        assert_eq!(end_pts, 1200); // 20 s → PTS 1200
        assert_eq!(start_pts, 840); // 14 s (20 − 6) → PTS 840
    }

    #[test]
    fn tolerance_merge_fuses_near_windows() {
        // Two windows with a 100-unit gap: strict merge keeps them separate, but
        // a tolerance ≥ 100 (Medal's EventWindow) fuses them into one clip.
        let w = vec![(0, 300), (400, 700)];
        assert_eq!(merge_windows_tol(w.clone(), 0), vec![(0, 300), (400, 700)]);
        assert_eq!(merge_windows_tol(w.clone(), 100), vec![(0, 700)]);
        // A gap wider than the tolerance stays split.
        assert_eq!(merge_windows_tol(w, 50), vec![(0, 300), (400, 700)]);
    }

    #[test]
    fn event_kind_labels() {
        assert_eq!(EventKind::Ace.label(), "Ace");
        assert_eq!(EventKind::TripleKill.label(), "Triple Kill");
        assert_eq!(EventKind::Knife.label(), "Knife");
        assert_eq!(EventKind::SpikeDetonated.label(), "Spike Detonated");
        assert_eq!(EventKind::Clutch.label(), "Clutch");
    }

    fn player(puuid: &str, team: &str) -> Player {
        Player {
            puuid: puuid.into(),
            game_name: String::new(),
            tag_line: String::new(),
            team_id: team.into(),
            character_id: String::new(),
            stats: PlayerStats::default(),
        }
    }

    /// All on default toggles, victory/clutch/spike are on.
    #[test]
    fn derives_spike_detonated_when_we_plant_and_detonate() {
        let me = "ME";
        // A kill at round 2000ms / game 92000ms ⇒ round offset 90000ms.
        let r = RoundResult {
            round_num: 4,
            player_stats: vec![PlayerRoundStats {
                puuid: me.into(),
                kills: vec![kill(me, "v", 2000, 92_000)],
                damage: Vec::new(),
            }],
            round_result_code: "Detonate".into(),
            winning_team: "Blue".into(),
            bomb_planter: me.into(),
            plant_round_time: 30_000,
            ..Default::default()
        };
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![player(me, "Blue")],
            teams: vec![],
            round_results: vec![r],
        };
        let ev = derive_events(&details, me, &EventToggles::default());
        let spike = ev.iter().find(|e| e.kind == EventKind::SpikeDetonated).unwrap();
        // Detonation = plant (30s) + 45s fuse = 75s into the round.
        assert_eq!(spike.time_since_round_start_millis, 75_000);
        // game time = offset 90s + 75s = 165s.
        assert_eq!(spike.time_since_game_start_millis, 165_000);
    }

    #[test]
    fn derives_spike_defused_when_we_defuse() {
        let me = "ME";
        let r = RoundResult {
            round_num: 1,
            player_stats: vec![PlayerRoundStats {
                puuid: me.into(),
                kills: vec![kill(me, "v", 1000, 41_000)], // offset 40s
                damage: Vec::new(),
            }],
            round_result_code: "Defuse".into(),
            winning_team: "Blue".into(),
            bomb_defuser: me.into(),
            defuse_round_time: 38_000,
            ..Default::default()
        };
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![player(me, "Blue")],
            teams: vec![],
            round_results: vec![r],
        };
        let ev = derive_events(&details, me, &EventToggles::default());
        let spike = ev.iter().find(|e| e.kind == EventKind::SpikeDefused).unwrap();
        assert_eq!(spike.time_since_round_start_millis, 38_000);
        assert_eq!(spike.time_since_game_start_millis, 78_000);
    }

    #[test]
    fn detects_1v2_clutch() {
        let me = "ME";
        // Blue: me + mate. Red: e1 + e2. Blue wins. Mate dies first, then we take
        // both enemies — a 1v2 clutch closed by our kills.
        let r = RoundResult {
            round_num: 7,
            player_stats: vec![PlayerRoundStats {
                puuid: me.into(),
                kills: vec![
                    kill("e1", "mate", 1000, 1000),
                    kill(me, "e1", 2000, 2000),
                    kill(me, "e2", 3000, 3000),
                ],
                damage: Vec::new(),
            }],
            winning_team: "Blue".into(),
            ..Default::default()
        };
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![
                player(me, "Blue"),
                player("mate", "Blue"),
                player("e1", "Red"),
                player("e2", "Red"),
            ],
            teams: vec![],
            round_results: vec![r],
        };
        let ev = derive_events(&details, me, &EventToggles::default());
        let clutch = ev.iter().find(|e| e.kind == EventKind::Clutch).unwrap();
        // Anchored at our clinching (last) kill (3 s)...
        assert_eq!(clutch.time_since_round_start_millis, 3000);
        // ...spanning back to our first clutch kill (2 s, after mate died at 1 s)
        // so the window covers the whole 1v2, not just the final kill.
        assert_eq!(clutch.lead_in_millis, 1000);
    }

    #[test]
    fn no_clutch_when_round_lost() {
        let me = "ME";
        let r = RoundResult {
            round_num: 7,
            player_stats: vec![PlayerRoundStats {
                puuid: me.into(),
                kills: vec![kill(me, "e1", 2000, 2000), kill(me, "e2", 3000, 3000)],
                damage: Vec::new(),
            }],
            winning_team: "Red".into(), // we lost
            ..Default::default()
        };
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![
                player(me, "Blue"),
                player("mate", "Blue"),
                player("e1", "Red"),
                player("e2", "Red"),
            ],
            teams: vec![],
            round_results: vec![r],
        };
        let ev = derive_events(&details, me, &EventToggles::default());
        assert!(!ev.iter().any(|e| e.kind == EventKind::Clutch));
    }

    #[test]
    fn derives_victory_at_last_action_when_team_won() {
        let me = "ME";
        let details = MatchDetails {
            match_info: MatchInfo::default(),
            players: vec![player(me, "Blue")],
            teams: vec![
                Team { team_id: "Blue".into(), won: true },
                Team { team_id: "Red".into(), won: false },
            ],
            round_results: vec![
                round(0, me, vec![kill(me, "v", 1000, 5_000)]),
                round(1, me, vec![kill(me, "v", 1000, 90_000)]),
            ],
        };
        let mut toggles = EventToggles::default();
        toggles.victory = true;
        let ev = derive_events(&details, me, &toggles);
        let v = ev.iter().find(|e| e.kind == EventKind::Victory).unwrap();
        // Anchored at the latest kill across the match (game time 90s).
        assert_eq!(v.time_since_game_start_millis, 90_000);
        assert_eq!(v.round, 1);
    }
}
