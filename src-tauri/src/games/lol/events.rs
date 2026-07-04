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

use crate::games::event::EventKind;
use crate::games::event_config::event_config;
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

event_config! {
    toggles: LolEventToggles,
    timing: LolEventTiming,
    timings: LolEventTimings,
    default_window: (8, 4),
    merge_fallback_after: 4,
    // Outplayed-style: headline moments default on; noisy per-kill / structure
    // events default off. Objectives lead in further since the fight that secures
    // them precedes the kill event.
    events: {
        kill        => EventKind::Kill,         on: false, window: (8, 4),
        double_kill => EventKind::DoubleKill,   on: false, window: (9, 4),
        triple_kill => EventKind::TripleKill,   on: true,  window: (11, 5),
        quadra_kill => EventKind::QuadraKill,   on: true,  window: (13, 5),
        pentakill   => EventKind::Pentakill,    on: true,  window: (15, 6),
        ace         => EventKind::Ace,          on: true,  window: (12, 6),
        first_blood => EventKind::FirstBlood,   on: true,  window: (8, 4),
        death       => EventKind::Death,        on: false, window: (8, 4),
        assist      => EventKind::Assist,       on: false, window: (8, 4),
        dragon      => EventKind::DragonKill,   on: true,  window: (14, 6),
        baron       => EventKind::BaronKill,    on: true,  window: (16, 6),
        herald      => EventKind::HeraldKill,   on: true,  window: (12, 5),
        turret      => EventKind::TurretKilled, on: false, window: (8, 4),
        inhibitor   => EventKind::InhibKilled,  on: true,  window: (8, 4),
        victory     => EventKind::Victory,      on: true,  window: (10, 6),
    },
}

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
        "FirstBlood" if is_me(&ev.recipient) || is_me(&ev.killer_name) => {
            Some(EventKind::FirstBlood)
        }
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

    // The on-disk (`settings.json`) field names for League are deliberately
    // *not* the `EventKind` variant names (`dragon` ↔ `DragonKill`, `inhibitor`
    // ↔ `InhibKilled`, …). These golden tests pin that mapping and the defaults
    // so a future edit that renames a field — silently resetting every existing
    // user's toggle on their next launch — fails the build instead.

    #[test]
    fn toggles_default_matches_kind_mapping() {
        let t = LolEventToggles::default();
        // Field ↔ EventKind: the non-uniform names must route to the right kind.
        assert_eq!(t.dragon, t.enabled(EventKind::DragonKill));
        assert_eq!(t.baron, t.enabled(EventKind::BaronKill));
        assert_eq!(t.herald, t.enabled(EventKind::HeraldKill));
        assert_eq!(t.turret, t.enabled(EventKind::TurretKilled));
        assert_eq!(t.inhibitor, t.enabled(EventKind::InhibKilled));
        // A representative slice of the default on/off state.
        assert!(!t.enabled(EventKind::Kill));
        assert!(t.enabled(EventKind::TripleKill));
        assert!(t.enabled(EventKind::DragonKill));
        assert!(!t.enabled(EventKind::TurretKilled));
        assert!(t.enabled(EventKind::Victory));
    }

    #[test]
    fn toggles_deserialize_from_legacy_field_names() {
        // A config saved by an older build: snake_case keys, `dragon`/`baron`/
        // `herald`/`turret`/`inhibitor` (not the kind names). Must round-trip to
        // the same in-memory state, not reset to defaults.
        let json = r#"{
            "kill": true, "double_kill": true, "triple_kill": false,
            "quadra_kill": false, "pentakill": false, "ace": false,
            "first_blood": false, "death": true, "assist": true,
            "dragon": false, "baron": false, "herald": false,
            "turret": true, "inhibitor": false, "victory": false
        }"#;
        let t: LolEventToggles = serde_json::from_str(json).unwrap();
        assert!(t.enabled(EventKind::Kill));
        assert!(t.enabled(EventKind::Death));
        assert!(!t.enabled(EventKind::DragonKill));
        assert!(t.enabled(EventKind::TurretKilled));
        assert!(!t.enabled(EventKind::Victory));
    }

    #[test]
    fn toggles_are_additive_over_missing_fields() {
        // `#[serde(default)]`: a partial config keeps stored keys and fills the
        // rest from `Default` (so a newly added event isn't force-off).
        let json = r#"{ "kill": true }"#;
        let t: LolEventToggles = serde_json::from_str(json).unwrap();
        assert!(t.enabled(EventKind::Kill)); // from JSON
        assert!(t.enabled(EventKind::TripleKill)); // from Default (on)
        assert!(!t.enabled(EventKind::TurretKilled)); // from Default (off)
    }

    #[test]
    fn timings_default_and_partial_window() {
        let ti = LolEventTimings::default();
        assert_eq!(ti.for_kind(EventKind::BaronKill).before, 16);
        assert_eq!(ti.for_kind(EventKind::BaronKill).after, 6);
        // A window object missing `after` fills it from `LolEventTiming::default`.
        let json = r#"{ "dragon": { "before": 20 } }"#;
        let ti: LolEventTimings = serde_json::from_str(json).unwrap();
        assert_eq!(ti.for_kind(EventKind::DragonKill).before, 20); // from JSON
        assert_eq!(ti.for_kind(EventKind::DragonKill).after, 4); // Timing default
        // An untouched field keeps the per-kind default, not the bare default.
        assert_eq!(ti.for_kind(EventKind::BaronKill).before, 16);
    }

    #[test]
    fn classifies_our_kill_and_death() {
        let mut k = ev("ChampionKill");
        k.killer_name = "Me".into();
        k.victim_name = "Enemy".into();
        assert_eq!(
            classify(&k, &MeId::single("Me"), "ORDER"),
            Some(EventKind::Kill)
        );

        let mut d = ev("ChampionKill");
        d.killer_name = "Enemy".into();
        d.victim_name = "Me".into();
        assert_eq!(
            classify(&d, &MeId::single("Me"), "ORDER"),
            Some(EventKind::Death)
        );

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
        assert_eq!(
            classify(&m, &MeId::single("Me"), "ORDER"),
            Some(EventKind::Pentakill)
        );
        m.kill_streak = 3;
        assert_eq!(
            classify(&m, &MeId::single("Me"), "ORDER"),
            Some(EventKind::TripleKill)
        );
    }

    #[test]
    fn classifies_objectives_and_victory() {
        let mut dragon = ev("DragonKill");
        dragon.killer_name = "Me".into();
        assert_eq!(
            classify(&dragon, &MeId::single("Me"), "ORDER"),
            Some(EventKind::DragonKill)
        );

        let mut win = ev("GameEnd");
        win.result = "Win".into();
        assert_eq!(
            classify(&win, &MeId::single("Me"), "ORDER"),
            Some(EventKind::Victory)
        );
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
        assert_eq!(
            classify(&a, &MeId::single("Me"), "ORDER"),
            Some(EventKind::Ace)
        );
        assert_eq!(classify(&a, &MeId::single("Me"), "CHAOS"), None);
    }
}
