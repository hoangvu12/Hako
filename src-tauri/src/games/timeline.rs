//! Wall-clock ↔ session-file PTS mapping, shared across every game integration.
//!
//! Built by the Mode-B session writer ([`crate::core::session`]) as it muxes
//! packets: each entry pairs a packet's capture wall-clock with its PTS. The
//! post-match cut then reconciles an event's wall-clock back to a PTS so a clip
//! can start on the right frame.
//!
//! This is deliberately game-agnostic — Valorant feeds it reconstructed
//! per-round wall-clocks while League feeds it `GameStart`-relative seconds, but
//! both only need "given a wall-clock tick, what PTS was being written then?".

#![allow(dead_code)]

/// 100-ns ticks per millisecond (most game clocks are ms; ours is 100-ns ticks).
pub const TICKS_PER_MS: i64 = 10_000;

/// Maps wall-clock (100-ns ticks) ↔ session-file PTS. Built by the Mode-B
/// session writer as it muxes packets: each entry pairs a packet's capture
/// timestamp with its PTS. Lookups linearly interpolate and clamp to the ends.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TimelineIndex {
    /// `(wallclock_ticks, pts)` pairs, kept sorted by wall-clock.
    samples: Vec<(i64, i64)>,
}

impl TimelineIndex {
    pub fn new() -> Self {
        TimelineIndex {
            samples: Vec::new(),
        }
    }

    /// Record a sample. Kept sorted; out-of-order pushes are inserted in place.
    pub fn push(&mut self, wallclock_ticks: i64, pts: i64) {
        match self.samples.last() {
            Some(&(w, _)) if wallclock_ticks >= w => self.samples.push((wallclock_ticks, pts)),
            _ => {
                let idx = self.samples.partition_point(|&(w, _)| w < wallclock_ticks);
                self.samples.insert(idx, (wallclock_ticks, pts));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// PTS at `wallclock_ticks`, linearly interpolated between bracketing
    /// samples and clamped to the recorded range. `None` if no samples.
    pub fn pts_at(&self, wallclock_ticks: i64) -> Option<i64> {
        if self.samples.is_empty() {
            return None;
        }
        let first = self.samples[0];
        let last = self.samples[self.samples.len() - 1];
        if wallclock_ticks <= first.0 {
            return Some(first.1);
        }
        if wallclock_ticks >= last.0 {
            return Some(last.1);
        }
        let i = self.samples.partition_point(|&(w, _)| w <= wallclock_ticks);
        let (w0, p0) = self.samples[i - 1];
        let (w1, p1) = self.samples[i];
        if w1 == w0 {
            return Some(p0);
        }
        let frac = (wallclock_ticks - w0) as f64 / (w1 - w0) as f64;
        Some(p0 + ((p1 - p0) as f64 * frac).round() as i64)
    }

    /// PTS at `wallclock_ticks`, but only if it lands within the recorded range
    /// (± `tol` ticks of either end). `None` when the moment was never captured —
    /// the recording started *after* it (the game was already in progress when we
    /// began recording) or stopped *before* it. Unlike [`pts_at`], this does NOT
    /// clamp a far-out-of-range timestamp onto a file end, so events that predate
    /// a mid-game recording start are dropped instead of piling onto PTS 0.
    pub fn pts_at_within(&self, wallclock_ticks: i64, tol: i64) -> Option<i64> {
        let first = *self.samples.first()?;
        let last = *self.samples.last()?;
        if wallclock_ticks < first.0 - tol || wallclock_ticks > last.0 + tol {
            return None;
        }
        self.pts_at(wallclock_ticks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_interpolates_and_clamps() {
        let mut t = TimelineIndex::new();
        t.push(0, 0);
        t.push(10_000_000, 60); // 1 s → PTS 60 (60 fps)
        assert_eq!(t.pts_at(-100), Some(0)); // clamp low
        assert_eq!(t.pts_at(5_000_000), Some(30)); // midpoint
        assert_eq!(t.pts_at(99_000_000), Some(60)); // clamp high
    }

    #[test]
    fn pts_at_within_drops_out_of_range() {
        let mut t = TimelineIndex::new();
        t.push(0, 0);
        t.push(10_000_000, 60); // recorded wall 0..1 s → PTS 0..60
        let tol = 2_000_000; // 0.2 s
                             // Inside the recording: interpolated normally.
        assert_eq!(t.pts_at_within(5_000_000, tol), Some(30));
        // Just outside but within tol: clamped to the near end.
        assert_eq!(t.pts_at_within(-1_000_000, tol), Some(0));
        assert_eq!(t.pts_at_within(11_000_000, tol), Some(60));
        // Far before the recording started (app opened mid-game): dropped.
        assert_eq!(t.pts_at_within(-60_000_000, tol), None);
        // Far after the recording stopped: dropped.
        assert_eq!(t.pts_at_within(60_000_000, tol), None);
    }
}
