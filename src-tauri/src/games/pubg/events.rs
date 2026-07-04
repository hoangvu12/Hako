//! PUBG per-game auto-clip toggles + clip timings.
//!
//! Event *derivation* lives in [`super::parse`] (it reads the replay sidecars);
//! this module only holds the user-facing toggle/timing config, mirroring the
//! other games' `events` modules so `settings` and the UI wire up identically.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::games::event::EventKind;

/// Per-event auto-clip toggles for PUBG. The headline moments (win, kill,
/// knockdown) default on; deaths default off (noisy). Additive (`serde(default)`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PubgEventToggles {
    pub victory: bool,
    pub kill: bool,
    pub knockdown: bool,
    pub death: bool,
}

impl Default for PubgEventToggles {
    fn default() -> Self {
        PubgEventToggles {
            victory: true,
            kill: true,
            knockdown: true,
            death: false,
        }
    }
}

impl PubgEventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Victory => self.victory,
            EventKind::Kill => self.kill,
            EventKind::Knockdown => self.knockdown,
            EventKind::Death => self.death,
            _ => false,
        }
    }
}

/// Per-event clip window (seconds before / after the moment) for PUBG.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PubgEventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for PubgEventTiming {
    fn default() -> Self {
        // Medal: EventWindow 10s / Padding 5s.
        PubgEventTiming {
            before: 8,
            after: 6,
        }
    }
}

impl PubgEventTiming {
    const fn new(before: u32, after: u32) -> Self {
        PubgEventTiming { before, after }
    }
}

/// Per-event clip windows for PUBG.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct PubgEventTimings {
    pub victory: PubgEventTiming,
    pub kill: PubgEventTiming,
    pub knockdown: PubgEventTiming,
    pub death: PubgEventTiming,
}

impl Default for PubgEventTimings {
    fn default() -> Self {
        PubgEventTimings {
            victory: PubgEventTiming::new(12, 8),
            kill: PubgEventTiming::new(8, 6),
            knockdown: PubgEventTiming::new(8, 5),
            death: PubgEventTiming::new(8, 5),
        }
    }
}

impl PubgEventTimings {
    pub fn for_kind(&self, kind: EventKind) -> PubgEventTiming {
        match kind {
            EventKind::Victory => self.victory,
            EventKind::Kill => self.kill,
            EventKind::Knockdown => self.knockdown,
            EventKind::Death => self.death,
            _ => PubgEventTiming::default(),
        }
    }

    /// Widest after-pad across all *enabled* kinds (sizes the merge tolerance).
    pub fn max_after(&self, toggles: &PubgEventToggles) -> u32 {
        ALL_KINDS
            .iter()
            .filter(|k| toggles.enabled(**k))
            .map(|k| self.for_kind(*k).after)
            .max()
            .unwrap_or(6)
    }
}

const ALL_KINDS: [EventKind; 4] = [
    EventKind::Victory,
    EventKind::Kill,
    EventKind::Knockdown,
    EventKind::Death,
];
