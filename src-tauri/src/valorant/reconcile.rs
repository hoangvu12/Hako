//! Kill timestamp → session-file PTS reconciliation + event derivation.
//!
//! Two pure stages, both unit-tested without a live game:
//!
//! 1. **Derive** ([`derive_events`]) — from post-match `MatchDetails`, compute
//!    our highlight events: one multi-kill tier per round (Kill/2K/3K/4K/Ace at
//!    the *last* kill of the round so the clip covers the whole sequence), plus
//!    Knife / Death / Assist. Filtered by [`EventToggles`].
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
/// single Kill / 2K / Death / Assist off.
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
        }
    }
}

/// Derive our highlight events from a finished match, keeping only enabled kinds.
/// Events come back sorted by `time_since_game_start_millis`.
pub fn derive_events(details: &MatchDetails, puuid: &str, toggles: &EventToggles) -> Vec<GameEvent> {
    let mut events = Vec::new();

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

        // One multi-kill tier event per round, anchored at the LAST kill so the
        // clip captures the full sequence (padding extends backwards).
        if let Some(last) = our_kills.last() {
            events.push(GameEvent {
                kind: EventKind::for_multikill(our_kills.len()),
                round: round.round_num,
                time_since_game_start_millis: last.time_since_game_start_millis,
                time_since_round_start_millis: last.time_since_round_start_millis,
            });
        }

        // Knife kills are their own highlight regardless of the round's tier.
        for k in our_kills.iter().filter(|k| k.is_knife()) {
            events.push(GameEvent {
                kind: EventKind::Knife,
                round: round.round_num,
                time_since_game_start_millis: k.time_since_game_start_millis,
                time_since_round_start_millis: k.time_since_round_start_millis,
            });
        }

        // Deaths (we are the victim) and assists (we assisted someone else).
        for ps in &round.player_stats {
            for k in &ps.kills {
                if k.victim == puuid {
                    events.push(GameEvent {
                        kind: EventKind::Death,
                        round: round.round_num,
                        time_since_game_start_millis: k.time_since_game_start_millis,
                        time_since_round_start_millis: k.time_since_round_start_millis,
                    });
                }
                if k.killer != puuid && k.assistants.iter().any(|a| a == puuid) {
                    events.push(GameEvent {
                        kind: EventKind::Assist,
                        round: round.round_num,
                        time_since_game_start_millis: k.time_since_game_start_millis,
                        time_since_round_start_millis: k.time_since_round_start_millis,
                    });
                }
            }
        }
    }

    events.retain(|e| toggles.enabled(e.kind));
    events.sort_by_key(|e| e.time_since_game_start_millis);
    events
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

/// Clip window `[center − before, center + after]` in PTS units, clamped to ≥ 0.
pub fn clip_window(center_pts: i64, pad_before_secs: u32, pad_after_secs: u32, fps: u32) -> (i64, i64) {
    let fps = fps.max(1) as i64;
    let start = (center_pts - pad_before_secs as i64 * fps).max(0);
    let end = center_pts + pad_after_secs as i64 * fps;
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
        }
    }

    #[test]
    fn derives_ace_as_single_event_at_last_kill() {
        let me = "ME";
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
        // One Ace event (default toggles: Ace on), anchored at the last kill.
        let aces: Vec<_> = ev.iter().filter(|e| e.kind == EventKind::Ace).collect();
        assert_eq!(aces.len(), 1);
        assert_eq!(aces[0].time_since_round_start_millis, 5000);
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
        let ev = GameEvent {
            kind: EventKind::Ace,
            round: 3,
            time_since_game_start_millis: 0,
            time_since_round_start_millis: 2000, // 2 s into the round
        };
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
        let ev = vec![GameEvent {
            kind: EventKind::Ace,
            round: 3,
            time_since_game_start_millis: 120_000,
            time_since_round_start_millis: 2_000,
        }];
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
        let ev = vec![GameEvent {
            kind: EventKind::Ace,
            round: 5,
            time_since_game_start_millis: 1_000,
            time_since_round_start_millis: 100,
        }];
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
    }
}
