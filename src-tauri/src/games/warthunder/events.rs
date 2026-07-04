//! War Thunder event derivation from `/hudmsg` damage lines, plus the per-game
//! toggles, clip timings, and match context.
//!
//! War Thunder has no structured kill feed — the web HUD only exposes the
//! human-readable combat log ("Player (Vehicle) shot down Enemy (Vehicle)"). So
//! we classify each line that mentions the local player's nickname, mirroring
//! Medal's `HudMessageHandler` but collapsed to a flat Kill / Death / Crash set
//! (per the plan — the per-vehicle-class split is unnecessary for auto-clipping):
//!
//! - `"... has crashed."` → **Crash** (we wrecked our own vehicle).
//! - `"shot down"` / `"destroyed"` → **Kill** or **Death** by *name position*:
//!   the killer is named first, so our nickname near the start (byte index < 3)
//!   means we got the kill; otherwise we're the victim.
//! - Special case: an aircraft is always "shot down", never "destroyed", so a
//!   `"destroyed"` line while we're flying isn't about us — Medal returns None,
//!   and so do we.
//!
//! English strings only for v1 (Medal ships 21 localizations); acceptable per the
//! plan. Classification is pure and unit-tested; the integration stamps each
//! emitted event with the capture-clock wall-clock at receipt.

#![allow(dead_code)]

use crate::games::event::EventKind;
use crate::games::event_config::event_config;
use crate::games::warthunder::api::Vehicle;
use crate::library::db::NewClip;

/// A nickname found at or before this byte index in a combat line marks the
/// *killer* (who is always named first). Medal uses the same "is our name at the
/// front?" heuristic; the small slack (< 3) tolerates a leading quote/space.
const KILLER_NAME_MAX_INDEX: usize = 3;

/// Classify one `/hudmsg` damage line for the local player, or `None` if the line
/// doesn't concern us / isn't a clippable moment.
///
/// `nickname` is the player's in-game name (from settings); an empty nickname
/// means we can't attribute anything, so every line is `None`. `vehicle` is the
/// current local vehicle class, used only for the aircraft "destroyed" exception.
pub fn classify(msg: &str, nickname: &str, vehicle: Vehicle) -> Option<EventKind> {
    if nickname.trim().is_empty() {
        return None;
    }
    let lower = msg.to_ascii_lowercase();
    let nick_lower = nickname.to_ascii_lowercase();
    // Only lines that mention us are ours to classify.
    let pos = lower.find(&nick_lower)?;

    // Our own vehicle wreck — the headline of a WT clip, ranked above a kill.
    if lower.contains("has crashed") {
        return Some(EventKind::Crash);
    }

    let shot_down = lower.contains("shot down");
    let destroyed = lower.contains("destroyed");
    if !shot_down && !destroyed {
        return None;
    }

    // Aircraft are "shot down", never "destroyed": a bare "destroyed" line while
    // we're flying is about ground/naval targets, not us. Medal returns None.
    if vehicle == Vehicle::Air && destroyed && !shot_down {
        return None;
    }

    // The killer is named first; our name at the front ⇒ we got the kill.
    if pos <= KILLER_NAME_MAX_INDEX {
        Some(EventKind::Kill)
    } else {
        Some(EventKind::Death)
    }
}

/// What we know about the current War Thunder battle, for tagging its clips.
/// War Thunder exposes no map/mode over the HUD API, so this is deliberately
/// light — just the local vehicle class for a friendly clip subtitle.
#[derive(Debug, Clone, Default)]
pub struct WarThunderContext {
    /// The vehicle class we last saw ("Air" / "Ground" / "Naval"), for the clip
    /// subtitle. Empty until `/indicators` first reports a valid vehicle.
    pub vehicle: String,
}

impl WarThunderContext {
    /// Update the stored vehicle label from a fresh `/indicators` reading (a
    /// [`Vehicle::Unknown`] reading leaves the last known class in place).
    pub fn observe(&mut self, vehicle: Vehicle) {
        if let Some(label) = vehicle_label(vehicle) {
            self.vehicle = label.to_string();
        }
    }

    pub fn clip_context(&self) -> NewClip {
        NewClip {
            mode: (!self.vehicle.is_empty()).then(|| self.vehicle.clone()),
            game: Some("warthunder".to_string()),
            ..Default::default()
        }
    }

    pub fn title_suffix(&self) -> String {
        self.vehicle.clone()
    }
}

/// Friendly label for a vehicle class, or `None` for [`Vehicle::Unknown`].
fn vehicle_label(vehicle: Vehicle) -> Option<&'static str> {
    match vehicle {
        Vehicle::Air => Some("Air"),
        Vehicle::Ground => Some("Ground"),
        Vehicle::Naval => Some("Naval"),
        Vehicle::Unknown => None,
    }
}

event_config! {
    toggles: WarThunderEventToggles,
    timing: WarThunderEventTiming,
    timings: WarThunderEventTimings,
    // Medal: Padding 15s / EventWindow 15s. WT engagements build up slowly, so
    // the lead-in is generous; the tail is a little shorter.
    default_window: (15, 10),
    merge_fallback_after: 8,
    // Kills + crashes default on; deaths default off (noisy).
    events: {
        kill  => EventKind::Kill,  on: true,  window: (15, 10),
        crash => EventKind::Crash, on: true,  window: (15, 10),
        death => EventKind::Death, on: false, window: (12, 8),
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_when_our_name_is_first() {
        let msg = "Me (Bf 109) shot down Enemy (Spitfire)";
        assert_eq!(classify(msg, "Me", Vehicle::Air), Some(EventKind::Kill));
    }

    #[test]
    fn death_when_our_name_is_the_victim() {
        let msg = "Enemy (Spitfire) shot down Me (Bf 109)";
        assert_eq!(classify(msg, "Me", Vehicle::Air), Some(EventKind::Death));
    }

    #[test]
    fn ground_destroyed_is_attributed() {
        let killer = "Me (Tiger) destroyed Enemy (T-34)";
        assert_eq!(classify(killer, "Me", Vehicle::Ground), Some(EventKind::Kill));
        let victim = "Enemy (T-34) destroyed Me (Tiger)";
        assert_eq!(classify(victim, "Me", Vehicle::Ground), Some(EventKind::Death));
    }

    #[test]
    fn air_destroyed_is_ignored() {
        // Planes are "shot down", not "destroyed": a "destroyed" line while flying
        // isn't about us even though our name matches.
        let msg = "Me (Bf 109) destroyed Enemy (Pillbox)";
        assert_eq!(classify(msg, "Me", Vehicle::Air), None);
    }

    #[test]
    fn crash_detected_over_kill_death() {
        let msg = "Me (Bf 109) has crashed.";
        assert_eq!(classify(msg, "Me", Vehicle::Air), Some(EventKind::Crash));
    }

    #[test]
    fn unrelated_line_is_none() {
        let msg = "Alice (Spitfire) shot down Bob (Bf 109)";
        assert_eq!(classify(msg, "Me", Vehicle::Air), None);
    }

    #[test]
    fn blank_nickname_classifies_nothing() {
        let msg = "Me (Bf 109) shot down Enemy (Spitfire)";
        assert_eq!(classify(msg, "", Vehicle::Air), None);
        assert_eq!(classify(msg, "   ", Vehicle::Air), None);
    }

    #[test]
    fn attribution_is_case_insensitive() {
        let msg = "mE (Bf 109) shot down Enemy (Spitfire)";
        assert_eq!(classify(msg, "Me", Vehicle::Air), Some(EventKind::Kill));
    }
}
