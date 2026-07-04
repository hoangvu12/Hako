//! PUBG per-game auto-clip toggles + clip timings.
//!
//! Event *derivation* lives in [`super::parse`] (it reads the replay sidecars);
//! this module only holds the user-facing toggle/timing config, mirroring the
//! other games' `events` modules so `settings` and the UI wire up identically.

#![allow(dead_code)]

use crate::games::event::EventKind;
use crate::games::event_config::event_config;

event_config! {
    toggles: PubgEventToggles,
    timing: PubgEventTiming,
    timings: PubgEventTimings,
    // Medal: EventWindow 10s / Padding 5s.
    default_window: (8, 6),
    merge_fallback_after: 6,
    // The headline moments (win, kill, knockdown) default on; deaths default off
    // (noisy).
    events: {
        victory   => EventKind::Victory,   on: true,  window: (12, 8),
        kill      => EventKind::Kill,      on: true,  window: (8, 6),
        knockdown => EventKind::Knockdown, on: true,  window: (8, 5),
        death     => EventKind::Death,     on: false, window: (8, 5),
    },
}
