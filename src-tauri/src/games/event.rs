//! The shared highlight-event vocabulary used by every game integration.
//!
//! One [`EventKind`] enum spans all games so the library (clip tags, seek-bar
//! markers), the cut/window machinery, and the settings UI stay uniform. Each
//! game only ever produces the subset that makes sense for it (Valorant emits
//! `SpikeDetonated`, League emits `DragonKill`, neither emits the other), but
//! both flow through the same windowed-cut tail in [`crate::games::recording`].
//!
//! `EventKind` serializes as its variant name and is persisted as a string label
//! in the clip DB, so adding a variant is always additive and forward-compatible.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Every highlight kind Hako can auto-clip, across all games. Variant names are
/// stable (serialized to settings + the clip DB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    // ── Shared combat (both games) ──────────────────────────────────────────
    Kill,
    DoubleKill,
    TripleKill,
    QuadraKill,
    /// Five kills in one round (Valorant) — distinct from League's `Pentakill`.
    Ace,
    Death,
    Assist,

    // ── Counter-Strike 2 ────────────────────────────────────────────────────
    /// A headshot kill (CS2's `round_killhs`). Kill/Double/Triple/Quadra/Ace/
    /// Death/Assist are reused from the shared combat set above.
    Headshot,

    // ── Valorant-specific ───────────────────────────────────────────────────
    Knife,
    /// We won the match (anchored at the final round's last action).
    Victory,
    /// We won a round as the last player alive on our team (a 1vX clutch).
    Clutch,
    /// A spike we planted detonated (round won by detonation).
    SpikeDetonated,
    /// We defused the spike.
    SpikeDefused,

    // ── League of Legends ───────────────────────────────────────────────────
    /// First champion takedown of the game (we were involved).
    FirstBlood,
    /// Five-kill streak by us (League's headline multi-kill).
    Pentakill,
    /// We killed (or stole) a dragon.
    DragonKill,
    /// We killed (or stole) Baron Nashor.
    BaronKill,
    /// We killed (or stole) the Rift Herald.
    HeraldKill,
    /// We destroyed an enemy turret.
    TurretKilled,
    /// We destroyed an enemy inhibitor.
    InhibKilled,

    // ── Rematch ─────────────────────────────────────────────────────────────
    /// A goal was scored in the match by either team (matching Medal's "Goal
    /// Scored").
    Goal,
    /// A goal scored by the local player (the `Stat Goals` achievement increment
    /// follows the goal cue in `Runtime.log`).
    MyGoal,
    /// The local player assisted a teammate's goal (`Stat Assists` increment).
    MyAssist,

    // ── Dota 2 ──────────────────────────────────────────────────────────────
    /// Four kills inside the multi-kill window (Dota's "Ultra Kill").
    UltraKill,
    /// Five kills inside the multi-kill window (Dota's "Rampage" — the headline).
    Rampage,

    // ── War Thunder ─────────────────────────────────────────────────────────
    /// We crashed our own vehicle (WT's "has crashed." hud message). Ranked above
    /// a plain kill (Medal slots crashes above kills). Kill/Death are reused.
    Crash,

    // ── PUBG ────────────────────────────────────────────────────────────────
    /// A knockdown involving us — we downed an enemy ("Groggy") or were downed
    /// ("Down"). Below a kill, above a death. Kill/Death/Victory are reused.
    Knockdown,
}

impl EventKind {
    /// Human label for clip titles / library tags (e.g. "Triple Kill", "Ace").
    pub fn label(self) -> &'static str {
        match self {
            EventKind::Kill => "Kill",
            EventKind::DoubleKill => "Double Kill",
            EventKind::TripleKill => "Triple Kill",
            EventKind::QuadraKill => "Quadra Kill",
            EventKind::Ace => "Ace",
            EventKind::Death => "Death",
            EventKind::Assist => "Assist",
            EventKind::Headshot => "Headshot",
            EventKind::Knife => "Knife",
            EventKind::Victory => "Victory",
            EventKind::Clutch => "Clutch",
            EventKind::SpikeDetonated => "Spike Detonated",
            EventKind::SpikeDefused => "Spike Defused",
            EventKind::FirstBlood => "First Blood",
            EventKind::Pentakill => "Pentakill",
            EventKind::DragonKill => "Dragon",
            EventKind::BaronKill => "Baron",
            EventKind::HeraldKill => "Herald",
            EventKind::TurretKilled => "Turret",
            EventKind::InhibKilled => "Inhibitor",
            EventKind::Goal => "Goal",
            EventKind::MyGoal => "My Goal",
            EventKind::MyAssist => "My Assist",
            EventKind::UltraKill => "Ultra Kill",
            EventKind::Rampage => "Rampage",
            EventKind::Crash => "Crash",
            EventKind::Knockdown => "Knockdown",
        }
    }

    /// The Valorant multi-kill tier for `n` kills in a single round
    /// (n≥1; 5+ ⇒ Ace).
    pub fn for_multikill(n: usize) -> EventKind {
        match n {
            2 => EventKind::DoubleKill,
            3 => EventKind::TripleKill,
            4 => EventKind::QuadraKill,
            n if n >= 5 => EventKind::Ace,
            _ => EventKind::Kill, // 0 or 1
        }
    }

    /// The League multi-kill tier for an `n`-kill streak (n≥1; 5+ ⇒ Pentakill).
    pub fn for_lol_multikill(n: usize) -> EventKind {
        match n {
            2 => EventKind::DoubleKill,
            3 => EventKind::TripleKill,
            4 => EventKind::QuadraKill,
            n if n >= 5 => EventKind::Pentakill,
            _ => EventKind::Kill,
        }
    }

    /// The Dota 2 multi-kill tier for `n` kills within the multi-kill window
    /// (n≥1; 4 ⇒ Ultra Kill, 5+ ⇒ Rampage).
    pub fn for_dota_multikill(n: usize) -> EventKind {
        match n {
            2 => EventKind::DoubleKill,
            3 => EventKind::TripleKill,
            4 => EventKind::UltraKill,
            n if n >= 5 => EventKind::Rampage,
            _ => EventKind::Kill,
        }
    }

    /// Tag priority: the headline moments outrank multi-kills, which outrank
    /// single kills, objectives, deaths, and assists. Used to pick the dominant
    /// label of a merged clip and to dedup overlapping seek-bar markers.
    pub fn priority(self) -> u8 {
        match self {
            EventKind::Victory => 30,
            // Rematch: the player's own goal/assist headline over a generic goal
            // (a merged clip spanning both is about *their* moment).
            EventKind::MyGoal => 27,
            EventKind::MyAssist => 26,
            EventKind::Goal => 25,
            // Dota's headline multi-kills (Rampage = 5, Ultra = 4).
            EventKind::Rampage => 24,
            EventKind::UltraKill => 23,
            EventKind::Pentakill => 22,
            EventKind::Ace => 21,
            EventKind::Clutch => 20,
            EventKind::BaronKill => 19,
            EventKind::QuadraKill => 18,
            EventKind::DragonKill => 17,
            EventKind::HeraldKill => 16,
            EventKind::TripleKill => 15,
            EventKind::Knife => 14,
            EventKind::InhibKilled => 13,
            EventKind::TurretKilled => 12,
            EventKind::DoubleKill => 11,
            EventKind::SpikeDefused => 10,
            EventKind::SpikeDetonated => 9,
            EventKind::FirstBlood => 8,
            // War Thunder crash: Medal ranks a crash above a kill (the headline of
            // a WT clip that has one). Above Headshot; below every multi-kill.
            EventKind::Crash => 4,
            // CS2 headshot: outranks a plain kill, below every multi-kill (Medal
            // slots Headshot between Kill and 2K).
            EventKind::Headshot => 3,
            EventKind::Kill => 2,
            EventKind::Assist => 1,
            // PUBG knockdown: below a kill, above a death. Shares the Assist rank
            // (PUBG never emits Assist, so there's no in-clip collision).
            EventKind::Knockdown => 1,
            EventKind::Death => 0,
        }
    }
}

/// One seek-bar marker within an event: its own label + match-relative time. A
/// single-action event (a lone kill, a death, a spike) carries exactly one; a
/// multi-kill carries one *per kill* (cumulative tiers — Kill, Double Kill, …)
/// and a clutch one per clutch kill, so the bar shows every moment rather than
/// just the clip's anchor. The reconciler maps each to a session PTS on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventMoment {
    /// Label for this marker (e.g. the running multi-kill tier of this kill).
    pub kind: EventKind,
    /// ms since the game started (anchor for the match-start calibration path).
    pub game_millis: i64,
    /// ms since this round started (anchor for the per-round path).
    pub round_millis: i64,
}

/// A derived in-match highlight, positioned in match-relative time. The
/// per-game reconciler later maps these to session-file PTS (Valorant via round
/// anchors + calibration; League via the live wall-clock at event receipt).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameEvent {
    pub kind: EventKind,
    /// Round index this event belongs to (`roundNum`). League is roundless (0).
    pub round: i32,
    /// Anchor time for reconciliation: ms since the game started. For a
    /// multi-action event (e.g. a multi-kill) this is the *last* action — the
    /// clip window ends here (+ padding).
    pub time_since_game_start_millis: i64,
    /// ms since this round started (finer anchor when round starts are known).
    pub time_since_round_start_millis: i64,
    /// How long *before* the anchor this event's sequence began, in ms (0 for a
    /// single-moment event). The clip window opens `lead_in_millis` before the
    /// anchor (then minus the before-pad) so the whole sequence is covered.
    pub lead_in_millis: i64,
    /// Per-action seek-bar markers. Defaults to a single marker at the anchor;
    /// multi-action events ([`Self::with_marks`]) carry one per constituent kill.
    pub marks: Vec<EventMoment>,
}

impl GameEvent {
    /// A single-moment event (one kill, death, spike, …): the window pads
    /// symmetrically around this instant (`lead_in_millis == 0`).
    pub fn point(kind: EventKind, round: i32, game_millis: i64, round_millis: i64) -> Self {
        GameEvent {
            kind,
            round,
            time_since_game_start_millis: game_millis,
            time_since_round_start_millis: round_millis,
            lead_in_millis: 0,
            marks: vec![EventMoment {
                kind,
                game_millis,
                round_millis,
            }],
        }
    }

    /// A multi-action event anchored at its *last* action (`game_millis` /
    /// `round_millis`), whose window reaches back `lead_in_millis` to the first
    /// action so the entire sequence (every kill of a multi-kill) is captured.
    pub fn span(
        kind: EventKind,
        round: i32,
        game_millis: i64,
        round_millis: i64,
        lead_in_millis: i64,
    ) -> Self {
        GameEvent {
            kind,
            round,
            time_since_game_start_millis: game_millis,
            time_since_round_start_millis: round_millis,
            lead_in_millis: lead_in_millis.max(0),
            marks: vec![EventMoment {
                kind,
                game_millis,
                round_millis,
            }],
        }
    }

    /// Replace the default single-anchor marker with explicit per-action moments
    /// — e.g. one marker per kill of a multi-kill. An empty list is ignored so
    /// every event keeps at least its anchor marker.
    pub fn with_marks(mut self, marks: Vec<EventMoment>) -> Self {
        if !marks.is_empty() {
            self.marks = marks;
        }
        self
    }
}
