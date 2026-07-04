//! Rematch's auto-clip event taxonomy — a single highlight, **Goal**, matching
//! Medal's lone "Goal Scored" event for the game.
//!
//! Kept in the same toggle/timing shape as League ([`crate::games::lol::events`])
//! so the settings UI and the shared windowed cut treat every game uniformly,
//! even though Rematch only has one event today.

#![allow(dead_code)]

use crate::games::event::EventKind;
use crate::games::event_config::event_config;

event_config! {
    toggles: RematchEventToggles,
    timing: RematchEventTiming,
    timings: RematchEventTimings,
    // A goal cue (the `PostGoal` celebration transition) logs a few seconds
    // *after* the ball crosses, so the `before` pad reaches back past that lag to
    // the actual shot; most of the window is `before`, holding through the
    // celebration on the tail.
    default_window: (12, 6),
    merge_fallback_after: 4,
    events: {
        goal => EventKind::Goal, on: true, window: (12, 6),
    },
}
