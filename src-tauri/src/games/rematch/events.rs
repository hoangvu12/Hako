//! Rematch's auto-clip event taxonomy — a single highlight, **Goal**, matching
//! Medal's lone "Goal Scored" event for the game.
//!
//! Kept in the same toggle/timing shape as League ([`crate::games::lol::events`])
//! so the settings UI and the shared windowed cut treat every game uniformly,
//! even though Rematch only has one event today.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

use crate::games::event::EventKind;

/// Per-event auto-clip toggles for Rematch. Additive (`#[serde(default)]`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RematchEventToggles {
    /// A goal was scored in the match.
    pub goal: bool,
}

impl Default for RematchEventToggles {
    fn default() -> Self {
        RematchEventToggles { goal: true }
    }
}

impl RematchEventToggles {
    pub fn enabled(&self, kind: EventKind) -> bool {
        match kind {
            EventKind::Goal => self.goal,
            _ => false,
        }
    }
}

/// Per-event clip window (seconds before / after the moment) for Rematch.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RematchEventTiming {
    pub before: u32,
    pub after: u32,
}

impl Default for RematchEventTiming {
    fn default() -> Self {
        // A goal: lead in far enough to catch the build-up / shot, hold through the
        // celebration. The goal cue (the `PostGoal` celebration transition) logs a
        // few seconds *after* the ball crosses, so the `before` pad is sized to
        // reach back past that lag to the actual shot; most of the window is `before`.
        RematchEventTiming {
            before: 12,
            after: 6,
        }
    }
}

/// Per-event clip windows for Rematch (one entry, mirrors League's struct shape).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct RematchEventTimings {
    pub goal: RematchEventTiming,
}

impl Default for RematchEventTimings {
    fn default() -> Self {
        RematchEventTimings {
            goal: RematchEventTiming::default(),
        }
    }
}

impl RematchEventTimings {
    pub fn for_kind(&self, kind: EventKind) -> RematchEventTiming {
        match kind {
            EventKind::Goal => self.goal,
            _ => RematchEventTiming::default(),
        }
    }

    /// Widest after-pad across all *enabled* kinds (sizes the merge tolerance).
    pub fn max_after(&self, toggles: &RematchEventToggles) -> u32 {
        if toggles.goal {
            self.goal.after
        } else {
            4
        }
    }
}
