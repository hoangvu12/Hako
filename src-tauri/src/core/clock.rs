//! Master clock + PTS mapping.
//!
//! Anchored on WGC `SystemRelativeTime` (100-ns ticks, the same unit as Windows
//! `QueryPerformanceCounter` normalized to FILETIME). This is the single source
//! of truth for converting a capture timestamp into the encoder's presentation
//! timestamp, so capture, buffer, and Valorant round boundaries all live on
//! one timeline.

#![allow(dead_code)]

/// 100-nanosecond ticks per second — the unit of WGC `SystemRelativeTime`.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

/// Current capture clock reading in 100-ns ticks — `QueryPerformanceCounter`
/// normalized to the same domain as WGC `SystemRelativeTime`, so a wall-clock
/// stamped here lines up with session packet timestamps. Used by Valorant's log
/// anchors and by League's live-event receipt timestamps to reconcile against the
/// session [`crate::games::timeline::TimelineIndex`].
#[cfg(windows)]
pub fn now_ticks() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    // SAFETY: both calls just read counters into locals.
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0;
    }
    // ticks(100ns) = qpc / freq * 1e7, in i128 to avoid overflow.
    (c as i128 * TICKS_PER_SECOND as i128 / f as i128) as i64
}

#[cfg(not(windows))]
pub fn now_ticks() -> i64 {
    0
}

/// Map a capture timestamp (100-ns ticks) to a presentation timestamp in
/// `1/fps` units relative to `base_ticks`.
///
/// PTS is in encoder time-base units (`1/fps`) but tracks wall-clock, because it
/// is derived from the capture clock rather than a frame counter — so elapsed
/// seconds = `pts / fps` even when frames are dropped (DWM composition cap).
/// The buffer relies on this to measure retention by PTS.
#[inline]
pub fn ticks_to_pts(ticks: i64, base_ticks: i64, fps: u32) -> i64 {
    ((ticks - base_ticks) * fps as i64) / TICKS_PER_SECOND
}

/// Master capture clock: anchors PTS on the first frame seen, then converts each
/// subsequent capture timestamp to a `1/fps` PTS off that anchor.
///
/// The wall-clock anchor (`base_ticks`) is also what lines Riot
/// round boundaries up against buffer positions.
pub struct MasterClock {
    base_ticks: Option<i64>,
    fps: u32,
}

impl MasterClock {
    pub fn new(fps: u32) -> Self {
        Self {
            base_ticks: None,
            fps: fps.clamp(1, 480),
        }
    }

    /// PTS for a frame captured at `ticks`. The first call sets the zero anchor.
    pub fn pts(&mut self, ticks: i64) -> i64 {
        let base = *self.base_ticks.get_or_insert(ticks);
        ticks_to_pts(ticks, base, self.fps)
    }

    pub fn base_ticks(&self) -> Option<i64> {
        self.base_ticks
    }

    pub fn fps(&self) -> u32 {
        self.fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pts_is_zero_anchored_and_scales_with_fps() {
        let mut c = MasterClock::new(60);
        // First frame at an arbitrary tick offset anchors PTS 0.
        let base = 123_456_789i64;
        assert_eq!(c.pts(base), 0);
        // One second later (1e7 ticks) → 60 PTS at 60 fps.
        assert_eq!(c.pts(base + TICKS_PER_SECOND), 60);
        // Half a second after the anchor → 30 PTS.
        assert_eq!(c.pts(base + TICKS_PER_SECOND / 2), 30);
        assert_eq!(c.base_ticks(), Some(base));
    }

    #[test]
    fn ticks_to_pts_matches_formula() {
        assert_eq!(ticks_to_pts(TICKS_PER_SECOND, 0, 30), 30);
        assert_eq!(ticks_to_pts(0, 0, 60), 0);
    }
}
