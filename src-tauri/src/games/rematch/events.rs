//! Rematch's auto-clip event taxonomy — **Goal** (any goal, matching Medal's
//! "Goal Scored") plus the attributed **My Goal** / **My Assist** variants
//! (derived from the local player's achievement-stat increments; see
//! [`crate::games::rematch::log_watch`]).
//!
//! Each goal produces exactly one event: the most specific enabled kind (my
//! goal > my assist > goal). Defaults clip only the player's own moments — the
//! all-goals `goal` toggle is opt-in for people who want every goal by either
//! team.
//!
//! Kept in the same toggle/timing shape as League ([`crate::games::lol::events`])
//! so the settings UI and the shared windowed cut treat every game uniformly.

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
    // celebration on the tail. The `my_*` events anchor on the same goal cue, so
    // they share its window.
    default_window: (12, 6),
    merge_fallback_after: 4,
    events: {
        goal => EventKind::Goal, on: false, window: (12, 6),
        my_goal => EventKind::MyGoal, on: true, window: (12, 6),
        my_assist => EventKind::MyAssist, on: true, window: (12, 6),
    },
}
