//! CS2 event derivation from GSI payload diffs, plus the per-game toggles,
//! clip timings, and match context.
//!
//! GSI has no discrete "kill" event — it streams cumulative stats. So we diff
//! successive [`ValidPayload`]s (mirroring Medal's `CounterStrike2Handler`):
//! a rise in `match_stats.kills` is a Kill, `deaths` a Death, `assists` an
//! Assist, and `player.state.round_killhs` a Headshot. A **per-round running
//! kill count** relabels the Nth kill into its multi-kill tier
//! (Kill→2K→3K→4K→Ace), exactly like Valorant. The caller stamps each emitted
//! event with the capture-clock wall-clock at receipt and reconciles to session
//! PTS at match end (League's live-feed shape).

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::games::cs2::payload::ValidPayload;
use crate::games::event::EventKind;
use crate::library::db::NewClip;

/// What one payload produced: any clippable events, plus the two lifecycle
/// signals the integration reacts to (new match / match over).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FeedResult {
    /// Events derived from this payload's deltas, in emission order.
    pub events: Vec<EventKind>,
    /// The map/mode changed → a new match began (finalize the previous one).
    pub new_match: bool,
    /// The match reached the final scoreboard (`map.phase == "gameover"`).
    pub game_over: bool,
}

/// Rolling diff state across GSI payloads for one CS2 game session. Cumulative
/// match stats only reset on a new match (map/mode change); `round_killhs` and
/// the per-round kill counter reset each round.
#[derive(Debug, Default)]
pub struct Cs2Tracker {
    prev_kills: i32,
    prev_deaths: i32,
    prev_assists: i32,
    prev_headshots: i32,
    round_num: i32,
    between_rounds: bool,
    /// Kills so far *this round* — drives the multi-kill tier of the next kill.
    round_kills: usize,
    map_name: String,
    mode_name: String,
    ctx: Cs2Context,
}

impl Cs2Tracker {
    pub fn new() -> Self {
        Cs2Tracker::default()
    }

    /// The latest known match context (map / mode / player), for clip tagging.
    pub fn context(&self) -> &Cs2Context {
        &self.ctx
    }

    /// Fold one validated payload into the running state, returning the events
    /// and lifecycle signals it produced.
    pub fn feed(&mut self, p: &ValidPayload) -> FeedResult {
        let mut out = FeedResult::default();
        self.ctx.player = p.player_name.clone();

        // Map or mode change ⇒ a fresh match: reset all round/diff state and
        // seed the cumulative baselines from this payload so we don't backfill a
        // burst of phantom kills when we join a match already in progress.
        if p.map_name != self.map_name || p.map_mode != self.mode_name {
            self.map_name = p.map_name.clone();
            self.mode_name = p.map_mode.clone();
            // Seed the round to wherever we joined (CS2's `map.round` isn't 0 when
            // we attach mid-match), so the first same-round payloads don't look
            // like a round change and reset the multi-kill counter.
            self.round_num = p.map_round;
            self.between_rounds = false;
            self.round_kills = 0;
            self.prev_kills = p.kills;
            self.prev_deaths = p.deaths;
            self.prev_assists = p.assists;
            self.prev_headshots = p.round_killhs;
            self.ctx.map = translate_map(&p.map_name);
            self.ctx.mode = translate_mode(&p.map_mode);
            out.new_match = true;
            out.game_over = p.is_gameover();
            return out;
        }

        // Deltas off the cumulative match stats + per-round headshot count.
        let d_kills = (p.kills - self.prev_kills).max(0);
        let d_deaths = (p.deaths - self.prev_deaths).max(0);
        let d_assists = (p.assists - self.prev_assists).max(0);
        let d_headshots = (p.round_killhs - self.prev_headshots).max(0);

        if p.map_round != self.round_num {
            self.between_rounds = true;
        }

        self.prev_kills = p.kills;
        self.prev_deaths = p.deaths;
        self.prev_assists = p.assists;
        self.prev_headshots = p.round_killhs;

        // Emit in Medal's order: assists, death, kills (multi-kill tiered),
        // headshots. Same-instant events merge into one clip downstream.
        for _ in 0..d_assists {
            out.events.push(EventKind::Assist);
        }
        if d_deaths > 0 {
            out.events.push(EventKind::Death);
        }
        for _ in 0..d_kills {
            self.round_kills += 1;
            out.events.push(EventKind::for_multikill(self.round_kills));
        }
        for _ in 0..d_headshots {
            out.events.push(EventKind::Headshot);
        }

        // A new round has actually begun once the round number advanced *and*
        // the per-round headshot counter is back to 0 (the freeze/buy phase):
        // commit the round and reset the per-round multi-kill counter.
        if self.between_rounds && p.round_killhs == 0 {
            self.between_rounds = false;
            self.round_num = p.map_round;
            self.round_kills = 0;
        }

        out.game_over = p.is_gameover();
        out
    }
}

/// What we know about the current CS2 match, for tagging its clips.
#[derive(Debug, Clone, Default)]
pub struct Cs2Context {
    /// Local player display name.
    pub player: String,
    /// Friendly map name ("Dust II", "Mirage", …).
    pub map: String,
    /// Friendly mode name ("Competitive", "Wingman", …).
    pub mode: String,
}

impl Cs2Context {
    pub fn clip_context(&self) -> NewClip {
        NewClip {
            map: (!self.map.is_empty()).then(|| self.map.clone()),
            mode: (!self.mode.is_empty()).then(|| self.mode.clone()),
            game: Some("cs2".to_string()),
            ..Default::default()
        }
    }

    pub fn title_suffix(&self) -> String {
        if !self.map.is_empty() {
            self.map.clone()
        } else {
            self.mode.clone()
        }
    }
}

/// Friendly display name for a CS2 internal map id (common maps only; unknown
/// ids pass through unchanged). Subset of Medal's `GameData` table.
fn translate_map(internal: &str) -> String {
    let name = match internal {
        "de_dust2" => "Dust II",
        "de_mirage" => "Mirage",
        "de_nuke" => "Nuke",
        "de_inferno" => "Inferno",
        "de_overpass" => "Overpass",
        "de_vertigo" => "Vertigo",
        "de_anubis" => "Anubis",
        "de_ancient" => "Ancient",
        "de_train" => "Train",
        "de_cache" => "Cache",
        "cs_office" => "Office",
        "cs_italy" => "Italy",
        "cs_agency" => "Agency",
        other => other,
    };
    name.to_string()
}

/// Friendly display name for a CS2 internal game-mode id.
fn translate_mode(internal: &str) -> String {
    let name = match internal {
        "competitive" => "Competitive",
        "scrimcomp2v2" => "Wingman",
        "gungameprogressive" => "Arms Race",
        "gungametrbomb" => "Demolition",
        "deathmatch" => "Deathmatch",
        "cooperative" => "Guardian",
        "survival" => "Danger Zone",
        "casual" => "Casual",
        other => other,
    };
    name.to_string()
}

/// Per-event auto-clip toggles for CS2. Multi-kills + headshots default on;
/// noisy single kills / deaths / assists default off. Additive (`serde(default)`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Cs2EventToggles {
    pub kill: bool,
    pub headshot: bool,
    pub double_kill: bool,
    pub triple_kill: bool,
    pub quadra_kill: bool,
    pub ace: bool,
    pub death: bool,
    pub assist: bool,
}

impl Default for Cs2EventToggles {
    fn default() -> Self {
        Cs2EventToggles {
            kill: false,
            headshot: true,
            double_kill: true,
            triple_kill: true,
            quadra_kill: true,
            ace: true,
            death: false,
            assist: false,
        }
    }
}

impl Cs2EventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::Headshot => self.headshot,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Ace => self.ace,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            _ => false,
        }
    }
}

/// Per-event clip window (seconds before / after the moment) for CS2.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Cs2EventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for Cs2EventTiming {
    fn default() -> Self {
        Cs2EventTiming {
            before: 6,
            after: 5,
        }
    }
}

impl Cs2EventTiming {
    const fn new(before: u32, after: u32) -> Self {
        Cs2EventTiming { before, after }
    }
}

/// Per-event clip windows for CS2 (Medal: EventWindow 5s, Padding 5s).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct Cs2EventTimings {
    pub kill: Cs2EventTiming,
    pub headshot: Cs2EventTiming,
    pub double_kill: Cs2EventTiming,
    pub triple_kill: Cs2EventTiming,
    pub quadra_kill: Cs2EventTiming,
    pub ace: Cs2EventTiming,
    pub death: Cs2EventTiming,
    pub assist: Cs2EventTiming,
}

impl Default for Cs2EventTimings {
    fn default() -> Self {
        Cs2EventTimings {
            kill: Cs2EventTiming::new(6, 5),
            headshot: Cs2EventTiming::new(6, 5),
            double_kill: Cs2EventTiming::new(7, 5),
            triple_kill: Cs2EventTiming::new(8, 5),
            quadra_kill: Cs2EventTiming::new(9, 6),
            ace: Cs2EventTiming::new(10, 6),
            death: Cs2EventTiming::new(6, 4),
            assist: Cs2EventTiming::new(6, 4),
        }
    }
}

impl Cs2EventTimings {
    pub fn for_kind(&self, kind: EventKind) -> Cs2EventTiming {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::Headshot => self.headshot,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Ace => self.ace,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            _ => Cs2EventTiming::default(),
        }
    }

    /// Widest after-pad across all *enabled* kinds (sizes the merge tolerance).
    pub fn max_after(&self, toggles: &Cs2EventToggles) -> u32 {
        ALL_KINDS
            .iter()
            .filter(|k| toggles.enabled(**k))
            .map(|k| self.for_kind(*k).after)
            .max()
            .unwrap_or(4)
    }
}

const ALL_KINDS: [EventKind; 8] = [
    EventKind::Kill,
    EventKind::Headshot,
    EventKind::DoubleKill,
    EventKind::TripleKill,
    EventKind::QuadraKill,
    EventKind::Ace,
    EventKind::Death,
    EventKind::Assist,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(kills: i32, deaths: i32, assists: i32, hs: i32, round: i32) -> ValidPayload {
        ValidPayload {
            map_name: "de_dust2".into(),
            map_mode: "competitive".into(),
            map_round: round,
            map_phase: "live".into(),
            player_name: "me".into(),
            team: "CT".into(),
            kills,
            deaths,
            assists,
            round_killhs: hs,
            round_phase: "live".into(),
            bomb: String::new(),
        }
    }

    #[test]
    fn first_payload_is_new_match_and_emits_nothing() {
        let mut t = Cs2Tracker::new();
        let r = t.feed(&payload(5, 2, 1, 0, 1));
        assert!(r.new_match);
        assert!(r.events.is_empty());
        assert_eq!(t.context().map, "Dust II");
        assert_eq!(t.context().mode, "Competitive");
    }

    #[test]
    fn multikill_tiers_within_a_round() {
        let mut t = Cs2Tracker::new();
        t.feed(&payload(0, 0, 0, 0, 1)); // seed
        assert_eq!(t.feed(&payload(1, 0, 0, 0, 1)).events, vec![EventKind::Kill]);
        assert_eq!(
            t.feed(&payload(2, 0, 0, 0, 1)).events,
            vec![EventKind::DoubleKill]
        );
        assert_eq!(
            t.feed(&payload(3, 0, 0, 0, 1)).events,
            vec![EventKind::TripleKill]
        );
        assert_eq!(
            t.feed(&payload(4, 0, 0, 0, 1)).events,
            vec![EventKind::QuadraKill]
        );
        assert_eq!(t.feed(&payload(5, 0, 0, 0, 1)).events, vec![EventKind::Ace]);
    }

    #[test]
    fn round_reset_restarts_multikill_counter() {
        let mut t = Cs2Tracker::new();
        t.feed(&payload(0, 0, 0, 0, 1)); // seed round 1
        t.feed(&payload(1, 0, 0, 0, 1)); // Kill (round_kills=1)
        t.feed(&payload(2, 0, 0, 0, 1)); // DoubleKill (round_kills=2)
                                         // Round advances; freeze phase (round_killhs back to 0) commits it.
        t.feed(&payload(2, 0, 0, 0, 2));
        // Next kill in the new round is a single Kill again, not a TripleKill.
        assert_eq!(t.feed(&payload(3, 0, 0, 0, 2)).events, vec![EventKind::Kill]);
    }

    #[test]
    fn headshot_and_kill_both_emit() {
        let mut t = Cs2Tracker::new();
        t.feed(&payload(0, 0, 0, 0, 1));
        // A headshot kill bumps both kills and round_killhs.
        let r = t.feed(&payload(1, 0, 0, 1, 1));
        assert_eq!(r.events, vec![EventKind::Kill, EventKind::Headshot]);
    }

    #[test]
    fn death_and_assist_deltas() {
        let mut t = Cs2Tracker::new();
        t.feed(&payload(0, 0, 0, 0, 1));
        assert_eq!(t.feed(&payload(0, 1, 0, 0, 1)).events, vec![EventKind::Death]);
        assert_eq!(
            t.feed(&payload(0, 1, 2, 0, 1)).events,
            vec![EventKind::Assist, EventKind::Assist]
        );
    }

    #[test]
    fn map_change_finalizes_and_reseeds() {
        let mut t = Cs2Tracker::new();
        t.feed(&payload(0, 0, 0, 0, 1));
        t.feed(&payload(3, 0, 0, 0, 1));
        let mut next = payload(0, 0, 0, 0, 1);
        next.map_name = "de_mirage".into();
        let r = t.feed(&next);
        assert!(r.new_match);
        assert!(r.events.is_empty());
        assert_eq!(t.context().map, "Mirage");
    }
}
