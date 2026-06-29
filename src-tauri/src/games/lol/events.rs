//! Map League's live event feed to Hako's shared [`EventKind`], plus the per-game
//! toggles + clip timings for League.
//!
//! Each event is classified relative to *us* (`me` = our feed name): a kill is a
//! `Kill` only when we're the killer, a `Death` only when we're the victim, an
//! objective only when we secured it, a `Victory` only when our team won. The
//! caller stamps the wall-clock at feed receipt and reconciles to a session PTS
//! the same way Valorant does (the live feed's ~1 s poll jitter is absorbed by
//! the ≥8 s clip padding, exactly like Valorant's 2 s presence poll).

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::games::event::EventKind;
use crate::games::lol::live_client::LiveEvent;

/// Our identity in the live event feed.
///
/// Which name form the feed uses for a player has shifted across patches (the
/// Riot ID migration): older clients used the summoner name, newer ones the Riot
/// ID game name, and some fields carry the full "GameName#TAG". So we keep *every*
/// known form for ourselves and match an event name against any of them,
/// tag-tolerantly (comparing the part before `#` as well). Matching on a single
/// field is the classic reason auto-clips silently capture nothing.
#[derive(Debug, Clone, Default)]
pub struct MeId {
    /// Lowercased, de-duplicated, non-empty name forms.
    aliases: Vec<String>,
}

impl MeId {
    pub fn from_names<I: IntoIterator<Item = String>>(names: I) -> Self {
        let mut aliases: Vec<String> = Vec::new();
        for n in names {
            let n = n.trim().to_ascii_lowercase();
            if !n.is_empty() && !aliases.contains(&n) {
                aliases.push(n);
            }
        }
        MeId { aliases }
    }

    /// A single-name identity (tests / simple callers).
    pub fn single(name: &str) -> Self {
        MeId::from_names([name.to_string()])
    }

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// Number of distinct name forms we recognize ourselves by (diagnostics).
    pub fn alias_count(&self) -> usize {
        self.aliases.len()
    }

    /// Whether `name` (an event's KillerName/VictimName/Assister) is us. Compares
    /// the full string and the pre-`#` base on both sides, case-insensitively.
    pub fn matches(&self, name: &str) -> bool {
        let n = name.trim().to_ascii_lowercase();
        if n.is_empty() {
            return false;
        }
        let n_base = n.split('#').next().unwrap_or(&n);
        self.aliases.iter().any(|a| {
            let a_base = a.split('#').next().unwrap_or(a);
            a == &n || a == n_base || a_base == n || a_base == n_base
        })
    }
}

/// Per-event auto-clip toggles for League. Headline moments default on; noisy
/// per-kill / structure events default off. Additive (`#[serde(default)]`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct LolEventToggles {
    pub kill: bool,
    pub double_kill: bool,
    pub triple_kill: bool,
    pub quadra_kill: bool,
    pub pentakill: bool,
    pub ace: bool,
    pub first_blood: bool,
    pub death: bool,
    pub assist: bool,
    pub dragon: bool,
    pub baron: bool,
    pub herald: bool,
    pub turret: bool,
    pub inhibitor: bool,
    pub victory: bool,
}

impl Default for LolEventToggles {
    fn default() -> Self {
        LolEventToggles {
            kill: false,
            double_kill: false,
            triple_kill: true,
            quadra_kill: true,
            pentakill: true,
            ace: true,
            first_blood: true,
            death: false,
            assist: false,
            dragon: true,
            baron: true,
            herald: true,
            turret: false,
            inhibitor: true,
            victory: true,
        }
    }
}

impl LolEventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Pentakill => self.pentakill,
            EventKind::Ace => self.ace,
            EventKind::FirstBlood => self.first_blood,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            EventKind::DragonKill => self.dragon,
            EventKind::BaronKill => self.baron,
            EventKind::HeraldKill => self.herald,
            EventKind::TurretKilled => self.turret,
            EventKind::InhibKilled => self.inhibitor,
            EventKind::Victory => self.victory,
            _ => false,
        }
    }
}

/// Per-event clip window (seconds before / after the moment) for League.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct LolEventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for LolEventTiming {
    fn default() -> Self {
        LolEventTiming { before: 8, after: 4 }
    }
}

impl LolEventTiming {
    const fn new(before: u32, after: u32) -> Self {
        LolEventTiming { before, after }
    }
}

/// Per-event clip windows for League (Outplayed-style). Objectives lead in
/// further since the fight that secures them precedes the kill event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct LolEventTimings {
    pub kill: LolEventTiming,
    pub double_kill: LolEventTiming,
    pub triple_kill: LolEventTiming,
    pub quadra_kill: LolEventTiming,
    pub pentakill: LolEventTiming,
    pub ace: LolEventTiming,
    pub first_blood: LolEventTiming,
    pub death: LolEventTiming,
    pub assist: LolEventTiming,
    pub dragon: LolEventTiming,
    pub baron: LolEventTiming,
    pub herald: LolEventTiming,
    pub turret: LolEventTiming,
    pub inhibitor: LolEventTiming,
    pub victory: LolEventTiming,
}

impl Default for LolEventTimings {
    fn default() -> Self {
        LolEventTimings {
            kill: LolEventTiming::new(8, 4),
            double_kill: LolEventTiming::new(9, 4),
            triple_kill: LolEventTiming::new(11, 5),
            quadra_kill: LolEventTiming::new(13, 5),
            pentakill: LolEventTiming::new(15, 6),
            ace: LolEventTiming::new(12, 6),
            first_blood: LolEventTiming::new(8, 4),
            death: LolEventTiming::new(8, 4),
            assist: LolEventTiming::new(8, 4),
            dragon: LolEventTiming::new(14, 6),
            baron: LolEventTiming::new(16, 6),
            herald: LolEventTiming::new(12, 5),
            turret: LolEventTiming::new(8, 4),
            inhibitor: LolEventTiming::new(8, 4),
            victory: LolEventTiming::new(10, 6),
        }
    }
}

impl LolEventTimings {
    pub fn for_kind(&self, kind: EventKind) -> LolEventTiming {
        match kind {
            EventKind::Kill => self.kill,
            EventKind::DoubleKill => self.double_kill,
            EventKind::TripleKill => self.triple_kill,
            EventKind::QuadraKill => self.quadra_kill,
            EventKind::Pentakill => self.pentakill,
            EventKind::Ace => self.ace,
            EventKind::FirstBlood => self.first_blood,
            EventKind::Death => self.death,
            EventKind::Assist => self.assist,
            EventKind::DragonKill => self.dragon,
            EventKind::BaronKill => self.baron,
            EventKind::HeraldKill => self.herald,
            EventKind::TurretKilled => self.turret,
            EventKind::InhibKilled => self.inhibitor,
            EventKind::Victory => self.victory,
            _ => LolEventTiming::default(),
        }
    }

    /// Widest after-pad across all *enabled* kinds (sizes the merge tolerance).
    pub fn max_after(&self, toggles: &LolEventToggles) -> u32 {
        ALL_KINDS
            .iter()
            .filter(|k| toggles.enabled(**k))
            .map(|k| self.for_kind(*k).after)
            .max()
            .unwrap_or(4)
    }
}

const ALL_KINDS: [EventKind; 15] = [
    EventKind::Kill,
    EventKind::DoubleKill,
    EventKind::TripleKill,
    EventKind::QuadraKill,
    EventKind::Pentakill,
    EventKind::Ace,
    EventKind::FirstBlood,
    EventKind::Death,
    EventKind::Assist,
    EventKind::DragonKill,
    EventKind::BaronKill,
    EventKind::HeraldKill,
    EventKind::TurretKilled,
    EventKind::InhibKilled,
    EventKind::Victory,
];

/// Whether an event name is one we attribute to a specific player (i.e. one
/// `classify` can only keep if it's *ours*). Used purely for diagnostics — to
/// tell "you got no clippable moments" apart from "we failed to recognize you".
pub fn is_owned_combat(event_name: &str) -> bool {
    matches!(
        event_name,
        "ChampionKill"
            | "Multikill"
            | "FirstBlood"
            | "DragonKill"
            | "BaronKill"
            | "HeraldKill"
            | "TurretKilled"
            | "InhibKilled"
    )
}

/// Classify a live event relative to us. Returns the [`EventKind`] to clip (when
/// it concerns us), or `None` if it's someone else's event / not clippable.
/// `my_team` is `"ORDER"` / `"CHAOS"`; used for team-wide events (Ace).
pub fn classify(ev: &LiveEvent, me: &MeId, my_team: &str) -> Option<EventKind> {
    let is_me = |n: &str| me.matches(n);
    match ev.event_name.as_str() {
        "ChampionKill" => {
            if is_me(&ev.killer_name) {
                Some(EventKind::Kill)
            } else if is_me(&ev.victim_name) {
                Some(EventKind::Death)
            } else if ev.assisters.iter().any(|a| is_me(a)) {
                Some(EventKind::Assist)
            } else {
                None
            }
        }
        "Multikill" if is_me(&ev.killer_name) => {
            Some(EventKind::for_lol_multikill(ev.kill_streak.max(2) as usize))
        }
        "FirstBlood" if is_me(&ev.recipient) || is_me(&ev.killer_name) => Some(EventKind::FirstBlood),
        "Ace" => {
            let mine = is_me(&ev.acer)
                || (!my_team.is_empty() && ev.acing_team.eq_ignore_ascii_case(my_team));
            mine.then_some(EventKind::Ace)
        }
        "DragonKill" if is_me(&ev.killer_name) => Some(EventKind::DragonKill),
        "BaronKill" if is_me(&ev.killer_name) => Some(EventKind::BaronKill),
        "HeraldKill" if is_me(&ev.killer_name) => Some(EventKind::HeraldKill),
        "TurretKilled" if is_me(&ev.killer_name) => Some(EventKind::TurretKilled),
        "InhibKilled" if is_me(&ev.killer_name) => Some(EventKind::InhibKilled),
        "GameEnd" if ev.result.eq_ignore_ascii_case("Win") => Some(EventKind::Victory),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(name: &str) -> LiveEvent {
        LiveEvent {
            event_name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn classifies_our_kill_and_death() {
        let mut k = ev("ChampionKill");
        k.killer_name = "Me".into();
        k.victim_name = "Enemy".into();
        assert_eq!(classify(&k, &MeId::single("Me"), "ORDER"), Some(EventKind::Kill));

        let mut d = ev("ChampionKill");
        d.killer_name = "Enemy".into();
        d.victim_name = "Me".into();
        assert_eq!(classify(&d, &MeId::single("Me"), "ORDER"), Some(EventKind::Death));

        // Someone else's kill we weren't part of → ignored.
        let mut other = ev("ChampionKill");
        other.killer_name = "A".into();
        other.victim_name = "B".into();
        assert_eq!(classify(&other, &MeId::single("Me"), "ORDER"), None);
    }

    #[test]
    fn classifies_multikill_tiers() {
        let mut m = ev("Multikill");
        m.killer_name = "Me".into();
        m.kill_streak = 5;
        assert_eq!(classify(&m, &MeId::single("Me"), "ORDER"), Some(EventKind::Pentakill));
        m.kill_streak = 3;
        assert_eq!(classify(&m, &MeId::single("Me"), "ORDER"), Some(EventKind::TripleKill));
    }

    #[test]
    fn classifies_objectives_and_victory() {
        let mut dragon = ev("DragonKill");
        dragon.killer_name = "Me".into();
        assert_eq!(classify(&dragon, &MeId::single("Me"), "ORDER"), Some(EventKind::DragonKill));

        let mut win = ev("GameEnd");
        win.result = "Win".into();
        assert_eq!(classify(&win, &MeId::single("Me"), "ORDER"), Some(EventKind::Victory));
        let mut lose = ev("GameEnd");
        lose.result = "Lose".into();
        assert_eq!(classify(&lose, &MeId::single("Me"), "ORDER"), None);
    }

    #[test]
    fn me_id_matches_across_name_forms() {
        // Feed uses our Riot ID game name even though our summoner name differs.
        let me = MeId::from_names(["LegacySummoner".into(), "Faker".into()]);
        let mut k = ev("ChampionKill");
        k.killer_name = "Faker".into();
        k.victim_name = "Enemy".into();
        assert_eq!(classify(&k, &me, "ORDER"), Some(EventKind::Kill));

        // Tag-tolerant: event carries the full "GameName#TAG".
        let mut k2 = ev("ChampionKill");
        k2.killer_name = "Faker#KR1".into();
        k2.victim_name = "Enemy".into();
        assert_eq!(classify(&k2, &me, "ORDER"), Some(EventKind::Kill));

        // Still not us.
        let mut other = ev("ChampionKill");
        other.killer_name = "SomeoneElse".into();
        other.victim_name = "Enemy".into();
        assert_eq!(classify(&other, &me, "ORDER"), None);
    }

    #[test]
    fn ace_matches_team() {
        let mut a = ev("Ace");
        a.acing_team = "ORDER".into();
        assert_eq!(classify(&a, &MeId::single("Me"), "ORDER"), Some(EventKind::Ace));
        assert_eq!(classify(&a, &MeId::single("Me"), "CHAOS"), None);
    }
}
