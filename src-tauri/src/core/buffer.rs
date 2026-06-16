//! RAM ring buffer of compressed packets + IDR index (Mode A).
//!
//! Holds the last N seconds of encoded video as compressed [`EncodedPacket`]s
//! (~100–400 MB, never raw frames — golden rule). Tracks keyframe (IDR)
//! positions so a saved clip can start on a keyframe at or before the cut point;
//! the encoder runs keyint = 1s so cut granularity is ~1s. Always-on
//! while capturing, feeding instant hotkey clips (`mux.rs`).
//!
//! ## Why PTS is a valid time axis here
//! The encoder runs with `max_b_frames = 0`, so packets are emitted in
//! presentation order and `pts == dts` — no reordering to reason about. PTS is
//! computed in `capture.rs` as `(system_relative_time − base) · fps / 1e7`, i.e.
//! it is in units of `1/fps` *but tracks wall-clock* (it's derived from the
//! capture clock, not a frame counter). So `(newest_pts − oldest_pts) / fps`
//! equals elapsed seconds even when capture delivers fewer than `fps` frames
//! (DWM composition cap). Retention measured in PTS is therefore
//! correct regardless of the actual delivered rate.
//!
//! ## Eviction is GOP-aligned
//! We never strand frames without a leading IDR. We drop a whole leading GOP
//! only once the *next* GOP's keyframe is itself older than the retention
//! window — so the oldest retained keyframe is always at or before
//! `newest − retention`, and any cut point inside the window has a usable IDR.
//! Net effect: the ring holds `retention` seconds **plus up to one extra GOP**.

#![allow(dead_code)]

use std::collections::VecDeque;

use serde::Serialize;

use crate::core::encode::EncodedPacket;

/// Default RAM-ring depth. 120 s at ~20 Mbps ≈ 300 MB (~100–400 MB).
/// Must comfortably exceed the longest hotkey clip (default "last 30 s").
pub const DEFAULT_RETENTION_SECS: u32 = 120;

/// Snapshot of ring health, for the `buffer-stats` event / dashboard.
#[derive(Debug, Clone, Serialize)]
pub struct BufferStats {
    /// Compressed packets currently retained.
    pub packets: usize,
    /// Retained keyframes (IDRs) — the count of valid clip start points.
    pub keyframes: usize,
    /// Bytes of compressed payload currently retained.
    pub bytes: usize,
    /// Wall-clock span currently retained (newest − oldest PTS, in seconds).
    pub duration_secs: f64,
    /// Configured retention target.
    pub retention_secs: u32,
    /// Packets evicted since start (lifetime counter).
    pub dropped: u64,
}

/// A fixed-duration RAM ring of compressed video packets with a keyframe index.
///
/// Single-owner / single-thread by design: the encode thread pushes; a save
/// (hotkey) path locks it briefly to [`slice_last`](Self::slice_last). Pushing
/// is O(GOP length) worst case (eviction scans at most one trailing GOP);
/// slicing is O(n) but only runs on an explicit save.
pub struct PacketRing {
    packets: VecDeque<EncodedPacket>,
    fps: u32,
    /// Retention window expressed in PTS units (`retention_secs · fps`).
    retention_pts: i64,
    retention_secs: u32,
    /// Live byte total of `packets` (maintained incrementally; avoids O(n) sums).
    bytes: usize,
    /// Live keyframe count of `packets`.
    keyframes: usize,
    /// Lifetime count of evicted packets (diagnostics).
    dropped: u64,
}

impl PacketRing {
    /// New ring retaining ~`retention_secs` of video encoded at `fps`.
    pub fn new(fps: u32, retention_secs: u32) -> Self {
        let fps = fps.clamp(1, 480);
        let retention_secs = retention_secs.max(1);
        PacketRing {
            packets: VecDeque::new(),
            fps,
            retention_pts: retention_secs as i64 * fps as i64,
            retention_secs,
            bytes: 0,
            keyframes: 0,
            dropped: 0,
        }
    }

    /// Append one freshly encoded packet, then evict GOPs that fell out of the
    /// retention window.
    pub fn push(&mut self, pkt: EncodedPacket) {
        if pkt.keyframe {
            self.keyframes += 1;
        }
        self.bytes += pkt.data.len();
        self.packets.push_back(pkt);
        self.evict();
    }

    /// Drop whole leading GOPs while the *next* GOP's keyframe is already older
    /// than the retention window (see module docs — keeps one GOP of slack so
    /// the window always has a preceding IDR).
    fn evict(&mut self) {
        let newest = match self.packets.back() {
            Some(p) => p.pts,
            None => return,
        };
        loop {
            // Index of the first keyframe *after* the front = start of GOP #2.
            let second_kf = self
                .packets
                .iter()
                .enumerate()
                .skip(1)
                .find(|(_, p)| p.keyframe)
                .map(|(i, _)| i);
            let Some(idx) = second_kf else { break };
            if newest - self.packets[idx].pts < self.retention_pts {
                break;
            }
            // GOP #2 is itself old enough → the entire leading GOP is droppable.
            for _ in 0..idx {
                if let Some(p) = self.packets.pop_front() {
                    self.bytes -= p.data.len();
                    if p.keyframe {
                        self.keyframes -= 1;
                    }
                    self.dropped += 1;
                }
            }
        }
    }

    /// Copy out the most recent `secs` seconds as a contiguous packet run that
    /// starts on a keyframe — ready for a stream-copy mux (`mux.rs`, no
    /// re-encode). Returns the packets in encode/presentation order.
    ///
    /// The start is the latest IDR with `pts ≤ (newest − secs·fps)`. If the ring
    /// holds less than `secs` (just started), it falls back to the earliest IDR,
    /// returning everything available. Empty if nothing has been buffered yet.
    pub fn slice_last(&self, secs: u32) -> Vec<EncodedPacket> {
        let newest = match self.packets.back() {
            Some(p) => p.pts,
            None => return Vec::new(),
        };
        let want_start = newest - (secs as i64).saturating_mul(self.fps as i64);

        // Latest keyframe at or before the cut point.
        let mut chosen: Option<usize> = None;
        for (i, p) in self.packets.iter().enumerate() {
            if p.keyframe && p.pts <= want_start {
                chosen = Some(i);
            }
        }
        // Buffer shorter than the request → start at the earliest IDR we have.
        if chosen.is_none() {
            chosen = self.packets.iter().position(|p| p.keyframe);
        }

        match chosen {
            Some(i) => self.packets.iter().skip(i).cloned().collect(),
            None => Vec::new(), // no keyframe buffered yet (shouldn't happen post-IDR)
        }
    }

    /// Wall-clock span currently retained, in seconds.
    pub fn duration_secs(&self) -> f64 {
        match (self.packets.front(), self.packets.back()) {
            (Some(f), Some(b)) => (b.pts - f.pts) as f64 / self.fps as f64,
            _ => 0.0,
        }
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn keyframe_count(&self) -> usize {
        self.keyframes
    }

    pub fn byte_size(&self) -> usize {
        self.bytes
    }

    pub fn retention_secs(&self) -> u32 {
        self.retention_secs
    }

    /// Drop everything (e.g. on capture restart).
    pub fn clear(&mut self) {
        self.packets.clear();
        self.bytes = 0;
        self.keyframes = 0;
    }

    pub fn stats(&self) -> BufferStats {
        BufferStats {
            packets: self.packets.len(),
            keyframes: self.keyframes,
            bytes: self.bytes,
            duration_secs: self.duration_secs(),
            retention_secs: self.retention_secs,
            dropped: self.dropped,
        }
    }
}

/// RAM ring of compressed **audio** (AAC) packets, retained by wall-clock.
///
/// Unlike [`PacketRing`], audio packets carry their PTS as **absolute 100 ns
/// QPC ticks** (the same clock as the video master clock — see
/// [`crate::core::audio`]), every packet is independently decodable (no GOP),
/// and there's no keyframe alignment to worry about. A clip slices the audio
/// covering its video window by tick range ([`slice_ticks`](Self::slice_ticks)).
pub struct AudioRing {
    packets: VecDeque<EncodedPacket>,
    retention_ticks: i64,
}

/// 100 ns ticks per second (mirror of `clock::TICKS_PER_SECOND`, kept local so
/// buffer.rs stays dependency-light).
const TICKS_PER_SECOND: i64 = 10_000_000;

impl AudioRing {
    pub fn new(retention_secs: u32) -> AudioRing {
        AudioRing {
            packets: VecDeque::new(),
            retention_ticks: retention_secs.max(1) as i64 * TICKS_PER_SECOND,
        }
    }

    /// Append one AAC packet (PTS in absolute 100 ns ticks) and evict anything
    /// older than the retention window.
    pub fn push(&mut self, pkt: EncodedPacket) {
        self.packets.push_back(pkt);
        let newest = self.packets.back().map(|p| p.pts).unwrap_or(0);
        while let Some(front) = self.packets.front() {
            if newest - front.pts > self.retention_ticks {
                self.packets.pop_front();
            } else {
                break;
            }
        }
    }

    /// Copy out the packets whose PTS falls in `[start_ticks, end_ticks]`, plus
    /// the one packet immediately before `start_ticks` so the decoder has audio
    /// covering the very start of the clip. Returned in capture order.
    pub fn slice_ticks(&self, start_ticks: i64, end_ticks: i64) -> Vec<EncodedPacket> {
        let mut out = Vec::new();
        let mut prior: Option<&EncodedPacket> = None;
        for p in &self.packets {
            if p.pts < start_ticks {
                prior = Some(p);
            } else if p.pts <= end_ticks {
                if out.is_empty() {
                    if let Some(pp) = prior {
                        out.push(pp.clone());
                    }
                }
                out.push(p.clone());
            } else {
                break;
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn clear(&mut self) {
        self.packets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a packet at `pts`; `keyframe` marks GOP starts. Payload size is
    /// arbitrary but distinct enough to exercise byte accounting.
    fn pkt(pts: i64, keyframe: bool) -> EncodedPacket {
        EncodedPacket {
            data: vec![0u8; if keyframe { 100 } else { 10 }],
            pts,
            dts: pts,
            keyframe,
        }
    }

    /// Feed `n` frames at 1 PTS/frame with a keyframe every `gop` frames.
    fn fill(ring: &mut PacketRing, n: i64, gop: i64) {
        for i in 0..n {
            ring.push(pkt(i, i % gop == 0));
        }
    }

    #[test]
    fn empty_ring_is_inert() {
        let r = PacketRing::new(60, 30);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert_eq!(r.duration_secs(), 0.0);
        assert!(r.slice_last(30).is_empty());
        assert_eq!(r.stats().bytes, 0);
    }

    #[test]
    fn evicts_gop_aligned_and_keeps_one_gop_of_slack() {
        // fps=10, retain 2 s ⇒ retention = 20 PTS. GOP = 10 frames.
        let mut r = PacketRing::new(10, 2);
        fill(&mut r, 100, 10); // pts 0..=99, keyframes at 0,10,20,...

        // Worked example from the design: drop leading GOPs while the *second*
        // GOP keyframe is ≥ 20 older than newest(99) ⇒ stop once 2nd kf ≥ 80,
        // i.e. front lands on the keyframe at pts 70. Span 70..99 = 2.9 s.
        let front = r.slice_last(1_000_000).first().cloned().unwrap();
        assert!(front.keyframe, "ring must start on a keyframe");
        assert_eq!(front.pts, 70);
        assert!((r.duration_secs() - 2.9).abs() < 1e-6, "got {}", r.duration_secs());
        // Byte accounting stays consistent with what's retained.
        let expect_bytes: usize = (70..100).map(|i| if i % 10 == 0 { 100 } else { 10 }).sum();
        assert_eq!(r.byte_size(), expect_bytes);
        assert_eq!(r.keyframe_count(), 3); // kf at 70, 80, 90
    }

    #[test]
    fn slice_last_starts_on_idr_at_or_before_cut() {
        let mut r = PacketRing::new(10, 100); // big retention: nothing evicted
        fill(&mut r, 100, 10); // pts 0..=99

        // Last 1 s ⇒ want_start = 99 − 10 = 89 ⇒ latest IDR ≤ 89 is pts 80.
        let s = r.slice_last(1);
        assert!(s.first().unwrap().keyframe);
        assert_eq!(s.first().unwrap().pts, 80);
        assert_eq!(s.last().unwrap().pts, 99);
        assert_eq!(s.len(), 20);
    }

    #[test]
    fn slice_longer_than_buffer_returns_from_earliest_idr() {
        let mut r = PacketRing::new(10, 2); // retains ~3 GOPs (pts 70..99)
        fill(&mut r, 100, 10);

        // Asking for 5 s when only ~3 s is held ⇒ start at earliest IDR (70).
        let s = r.slice_last(5);
        assert_eq!(s.first().unwrap().pts, 70);
        assert!(s.first().unwrap().keyframe);
        assert_eq!(s.len(), 30); // 70..=99
    }

    #[test]
    fn dropped_counter_tracks_evictions() {
        let mut r = PacketRing::new(10, 2);
        fill(&mut r, 100, 10);
        // Retained 70..99 = 30 packets ⇒ 70 evicted.
        assert_eq!(r.stats().dropped, 70);
        assert_eq!(r.len(), 30);
    }

    /// Audio packet at absolute tick `t` (100 ns units).
    fn apkt(t: i64) -> EncodedPacket {
        EncodedPacket {
            data: vec![0u8; 8],
            pts: t,
            dts: t,
            keyframe: true,
        }
    }

    #[test]
    fn audio_ring_evicts_by_wallclock() {
        // 2 s retention = 2e7 ticks. Push packets ~21ms apart (213_333 ticks).
        let mut r = AudioRing::new(2);
        for i in 0..200i64 {
            r.push(apkt(i * 213_333));
        }
        let newest = 199 * 213_333;
        // Oldest retained must be within the 2 s window of the newest.
        let span = newest - r.slice_ticks(0, i64::MAX).first().unwrap().pts;
        assert!(span <= 2 * TICKS_PER_SECOND, "retained span {span} exceeds 2s");
        assert!(!r.is_empty());
    }

    #[test]
    fn audio_slice_includes_one_packet_before_start() {
        let mut r = AudioRing::new(60);
        for i in 0..10i64 {
            r.push(apkt(i * 1000)); // ticks 0,1000,...,9000
        }
        // Window [2500, 5500] covers ticks 3000,4000,5000; plus the one before
        // (2000) so audio covers the very start of the clip.
        let s = r.slice_ticks(2500, 5500);
        let got: Vec<i64> = s.iter().map(|p| p.pts).collect();
        assert_eq!(got, vec![2000, 3000, 4000, 5000]);
    }
}
