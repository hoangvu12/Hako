//! Dota 2 event derivation from GSI payload diffs, plus the per-game toggles,
//! clip timings, and match context.
//!
//! Like CS2, Dota's GSI streams cumulative stats, so we diff successive payloads
//! (mirroring Medal's `Dota2Handler`): a rise in `player.kills` is a kill,
//! `deaths` a death, `assists` an assist. Multi-kills are grouped by an **18-second
//! sliding window** over recent kill wall-clocks — chaining kills within 18s of
//! each other into Double / Triple / Ultra / Rampage. A new match is detected
//! when `map.game_time` drops (the clock resets). The caller stamps each event
//! with the capture-clock wall-clock at receipt and reconciles at match end.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::core::clock::TICKS_PER_SECOND;
use crate::games::dota2::payload::ValidPayload;
use crate::games::event::EventKind;
use crate::library::db::NewClip;

/// Kills within this window of each other chain into a higher multi-kill tier.
const KILL_WINDOW_TICKS: i64 = 18 * TICKS_PER_SECOND;

/// What one payload produced: any events plus whether a new match began.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FeedResult {
    pub events: Vec<EventKind>,
    /// `map.game_time` dropped → a fresh match (finalize the previous one).
    pub new_match: bool,
}

/// Rolling diff state across GSI payloads for one Dota 2 game. Cumulative stats
/// only reset on a new match (clock drop); kill wall-clocks feed the multi-kill
/// window.
#[derive(Debug)]
pub struct Dota2Tracker {
    game_time: i32,
    prev_kills: i32,
    prev_deaths: i32,
    prev_assists: i32,
    /// Wall-clock (100-ns ticks) of recent kills, for the multi-kill window.
    kill_ticks: Vec<i64>,
    ctx: Dota2Context,
}

impl Default for Dota2Tracker {
    fn default() -> Self {
        Dota2Tracker {
            // Start at MAX so the first payload's game_time is always lower →
            // treated as a new match (mirrors Medal's `int.MaxValue` seed).
            game_time: i32::MAX,
            prev_kills: 0,
            prev_deaths: 0,
            prev_assists: 0,
            kill_ticks: Vec::new(),
            ctx: Dota2Context::default(),
        }
    }
}

impl Dota2Tracker {
    pub fn new() -> Self {
        Dota2Tracker::default()
    }

    pub fn context(&self) -> &Dota2Context {
        &self.ctx
    }

    /// Fold one validated payload into the running state. `now` is the
    /// capture-clock wall-clock at receipt (also what the caller stamps the
    /// returned events with), used for the multi-kill window.
    pub fn feed(&mut self, p: &ValidPayload, now: i64) -> FeedResult {
        let mut out = FeedResult::default();
        self.ctx.hero = translate_hero(&p.hero);
        self.ctx.player = p.player_name.clone();

        // The clock going backward means a new game started: reset diff state and
        // seed the cumulative baselines from this payload (so joining a match in
        // progress doesn't backfill a burst of phantom kills — an improvement on
        // Medal, which reseeds to 0 and re-emits the whole score).
        if p.game_time < self.game_time {
            self.game_time = p.game_time;
            self.prev_kills = p.kills;
            self.prev_deaths = p.deaths;
            self.prev_assists = p.assists;
            self.kill_ticks.clear();
            out.new_match = true;
            return out;
        }
        self.game_time = p.game_time;

        let d_kills = p.kills - self.prev_kills;
        let d_deaths = p.deaths - self.prev_deaths;
        let d_assists = p.assists - self.prev_assists;
        self.prev_kills = p.kills;
        self.prev_deaths = p.deaths;
        self.prev_assists = p.assists;

        // Medal's emission order: assists, deaths, then the kill tier.
        if d_assists > 0 {
            out.events.push(EventKind::Assist);
        }
        if d_deaths > 0 {
            out.events.push(EventKind::Death);
        }
        if d_kills > 0 {
            out.events.push(self.kill_tier(d_kills, now));
        }
        out
    }

    /// The multi-kill tier for a kill delta landing at `now`: extend the streak
    /// backward through recent kills that chain within [`KILL_WINDOW_TICKS`].
    fn kill_tier(&mut self, d_kills: i32, now: i64) -> EventKind {
        let mut streak = d_kills.max(1);
        let mut anchor = now;
        for &ts in self.kill_ticks.iter().rev() {
            if anchor - ts < KILL_WINDOW_TICKS {
                streak += 1;
                anchor = ts;
            } else {
                break;
            }
        }
        self.kill_ticks.push(now);
        EventKind::for_dota_multikill(streak as usize)
    }
}

/// What we know about the current Dota 2 match, for tagging its clips.
#[derive(Debug, Clone, Default)]
pub struct Dota2Context {
    /// Local player display name.
    pub player: String,
    /// Friendly hero name ("Anti-Mage", "Juggernaut", …).
    pub hero: String,
}

impl Dota2Context {
    pub fn clip_context(&self) -> NewClip {
        NewClip {
            // Reuse the agent/champion column for the hero.
            agent: (!self.hero.is_empty()).then(|| self.hero.clone()),
            game: Some("dota2".to_string()),
            ..Default::default()
        }
    }

    pub fn title_suffix(&self) -> String {
        self.hero.clone()
    }
}

/// Friendly hero name from a Dota internal id: strip the `npc_dota_hero_` prefix
/// and Title-Case the underscored remainder (`npc_dota_hero_anti_mage` →
/// "Anti Mage"). Good enough without shipping Medal's full 120-hero table.
fn translate_hero(internal: &str) -> String {
    if internal.is_empty() {
        return String::new();
    }
    let stem = internal.strip_prefix("npc_dota_hero_").unwrap_or(internal);
    stem.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Per-event auto-clip toggles for Dota 2. Multi-kills default on; single kills /
/// deaths / assists default off. Additive (`serde(default)`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Dota2EventToggles {
    pub kill: bool,
    pub double_kill: bool,
    pub triple_kill: bool,
    pub ultra_kill: bool,
    pub rampage: bool,
    pub death: bool,
    pub assist: bool,
}

impl Default for Dota2EventToggles {
    fn default() -> Self {
        Dota2EventToggles {
            kill: false,
            double_kill: true,
            triple_kill: true,
            ultra_kill: true,
            rampage: true,
            death: false,
            assist: false,
        }
    }
}

impl Dota2EventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::UltraKill => self.ultra_kill,
            EventKind::Rampage => self.rampage,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            _ => false,
        }
    }
}

/// Per-event clip window (seconds before / after) for Dota 2.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Dota2EventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for Dota2EventTiming {
    fn default() -> Self {
        Dota2EventTiming {
            before: 12,
            after: 8,
        }
    }
}

impl Dota2EventTiming {
    const fn new(before: u32, after: u32) -> Self {
        Dota2EventTiming { before, after }
    }
}

/// Per-event clip windows for Dota 2 (Medal: EventWindow 20s, Padding 10s).
/// Higher tiers lead in further to cover the whole streak.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Dota2EventTimings {
    pub kill: Dota2EventTiming,
    pub double_kill: Dota2EventTiming,
    pub triple_kill: Dota2EventTiming,
    pub ultra_kill: Dota2EventTiming,
    pub rampage: Dota2EventTiming,
    pub death: Dota2EventTiming,
    pub assist: Dota2EventTiming,
}

impl Default for Dota2EventTimings {
    fn default() -> Self {
        Dota2EventTimings {
            kill: Dota2EventTiming::new(10, 6),
            double_kill: Dota2EventTiming::new(14, 8),
            triple_kill: Dota2EventTiming::new(20, 8),
            ultra_kill: Dota2EventTiming::new(28, 10),
            rampage: Dota2EventTiming::new(36, 10),
            death: Dota2EventTiming::new(10, 6),
            assist: Dota2EventTiming::new(10, 6),
        }
    }
}

impl Dota2EventTimings {
    pub fn for_kind(&self, kind: EventKind) -> Dota2EventTiming {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::UltraKill => self.ultra_kill,
            EventKind::Rampage => self.rampage,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            _ => Dota2EventTiming::default(),
        }
    }

    /// Widest after-pad across all *enabled* kinds (sizes the merge tolerance).
    pub fn max_after(&self, toggles: &Dota2EventToggles) -> u32 {
        ALL_KINDS
            .iter()
            .filter(|k| toggles.enabled(**k))
            .map(|k| self.for_kind(*k).after)
            .max()
            .unwrap_or(6)
    }
}

const ALL_KINDS: [EventKind; 7] = [
    EventKind::Kill,
    EventKind::DoubleKill,
    EventKind::TripleKill,
    EventKind::UltraKill,
    EventKind::Rampage,
    EventKind::Death,
    EventKind::Assist,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(kills: i32, deaths: i32, assists: i32, game_time: i32) -> ValidPayload {
        ValidPayload {
            game_time,
            match_id: "1".into(),
            hero: "npc_dota_hero_anti_mage".into(),
            player_name: "me".into(),
            kills,
            deaths,
            assists,
        }
    }

    #[test]
    fn first_payload_is_new_match_and_seeds() {
        let mut t = Dota2Tracker::new();
        let r = t.feed(&payload(4, 1, 2, 100), 0);
        assert!(r.new_match);
        assert!(r.events.is_empty());
        assert_eq!(t.context().hero, "Anti Mage");
    }

    #[test]
    fn multikill_chains_within_window() {
        let mut t = Dota2Tracker::new();
        t.feed(&payload(0, 0, 0, 0), 0); // seed
        let s = TICKS_PER_SECOND;
        assert_eq!(t.feed(&payload(1, 0, 0, 10), 5 * s).events, vec![EventKind::Kill]);
        assert_eq!(
            t.feed(&payload(2, 0, 0, 20), 15 * s).events,
            vec![EventKind::DoubleKill]
        );
        assert_eq!(
            t.feed(&payload(3, 0, 0, 30), 25 * s).events,
            vec![EventKind::TripleKill]
        );
        assert_eq!(
            t.feed(&payload(4, 0, 0, 40), 35 * s).events,
            vec![EventKind::UltraKill]
        );
        assert_eq!(
            t.feed(&payload(5, 0, 0, 50), 45 * s).events,
            vec![EventKind::Rampage]
        );
    }

    #[test]
    fn kills_outside_window_do_not_chain() {
        let mut t = Dota2Tracker::new();
        t.feed(&payload(0, 0, 0, 0), 0);
        let s = TICKS_PER_SECOND;
        assert_eq!(t.feed(&payload(1, 0, 0, 10), 10 * s).events, vec![EventKind::Kill]);
        // 30s later — beyond the 18s window → a fresh single Kill, not a Double.
        assert_eq!(t.feed(&payload(2, 0, 0, 40), 40 * s).events, vec![EventKind::Kill]);
    }

    #[test]
    fn clock_drop_is_new_match() {
        let mut t = Dota2Tracker::new();
        t.feed(&payload(0, 0, 0, 100), 0);
        t.feed(&payload(3, 0, 0, 200), TICKS_PER_SECOND);
        // A new game: clock resets to a lower value.
        let r = t.feed(&payload(0, 0, 0, -90), 2 * TICKS_PER_SECOND);
        assert!(r.new_match);
        assert!(r.events.is_empty());
    }

    #[test]
    fn death_and_assist() {
        let mut t = Dota2Tracker::new();
        t.feed(&payload(0, 0, 0, 0), 0);
        assert_eq!(t.feed(&payload(0, 1, 0, 10), TICKS_PER_SECOND).events, vec![EventKind::Death]);
        assert_eq!(
            t.feed(&payload(0, 1, 1, 20), 2 * TICKS_PER_SECOND).events,
            vec![EventKind::Assist]
        );
    }

    #[test]
    fn translate_hero_titlecases() {
        assert_eq!(translate_hero("npc_dota_hero_anti_mage"), "Anti Mage");
        assert_eq!(translate_hero("npc_dota_hero_juggernaut"), "Juggernaut");
        assert_eq!(translate_hero(""), "");
    }
}
