//! Disk-backed rolling buffer of compressed video packets (Mode A, disk variant).
//!
//! The on-disk counterpart to [`crate::core::buffer::PacketRing`]: instead of
//! holding the last N seconds of encoded video in RAM (~100–400 MB), it spools
//! the compressed [`EncodedPacket`]s to **GOP-aligned segment files** and keeps
//! only the *currently-growing* segment in memory (a `BufWriter` of at most one
//! GOP, ~one second). This trades RAM for steady disk writes — Medal's
//! "Recording buffer: RAM vs Disk" toggle — and is selected per the
//! `buffer_storage` setting (see `settings.rs`).
//!
//! Like the RAM ring, the CPU only ever touches **compressed bytes** (the golden
//! rule): packets are written verbatim to a flat framed log and read back into a
//! `Vec<EncodedPacket>` on save, which then goes through the *same*
//! [`crate::core::mux::write_clip`] stream-copy path — so a disk-buffered clip is
//! byte-identical to a RAM-buffered one.
//!
//! ## Why audio stays in RAM
//! Only video is spooled to disk. The compressed AAC rings ([`crate::core::buffer::AudioRing`])
//! are a few MB even for a long buffer with separate tracks, so the RAM win is
//! entirely about video; keeping audio in RAM avoids a second on-disk format and
//! keeps the save path's per-track tick slicing unchanged.
//!
//! ## Segment files & rotation
//! A new segment file is opened on every keyframe, so **each segment starts with
//! an IDR** — the unit both rotation and eviction work in (mirroring the RAM
//! ring's GOP-aligned eviction). A segment is a flat sequence of records:
//!
//! ```text
//! pts: i64-LE | dts: i64-LE | flags: u8 (bit0 = keyframe) | len: u32-LE | data: [u8; len]
//! ```
//!
//! Eviction deletes whole leading segments while the *next* segment's start
//! keyframe is already older than the retention window — so the oldest retained
//! segment always begins at or before `newest − retention`, and any cut point in
//! the window has a usable preceding IDR. Net effect matches the RAM ring:
//! `retention` seconds **plus up to one extra GOP**.
//!
//! Disk errors are non-fatal: on the first write/rotate failure the ring latches
//! into a `failed` state and silently stops buffering (a save then finds it empty)
//! rather than tearing down live capture.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use crate::core::buffer::BufferStats;
use crate::core::encode::EncodedPacket;

/// Per-record fixed header size: pts(8) + dts(8) + flags(1) + len(4).
const RECORD_HEADER: usize = 8 + 8 + 1 + 4;

/// A finalized segment file on disk (one GOP, starting at a keyframe).
struct Segment {
    path: PathBuf,
    first_pts: i64,
    last_pts: i64,
    bytes: u64,
    packets: u64,
    keyframes: u64,
}

/// The segment currently being written (kept open across pushes).
struct OpenSegment {
    seg: Segment,
    writer: BufWriter<File>,
}

/// A disk-backed ring retaining ~`retention_secs` of compressed video as
/// GOP-aligned segment files under a private directory.
///
/// Single-owner like [`crate::core::buffer::PacketRing`]: the encode thread
/// pushes; a save briefly takes the same lock to [`slice_last`](Self::slice_last)
/// (which flushes the open segment and reads the covering files back).
pub struct DiskPacketRing {
    /// Private directory holding `seg_*.hbuf` files; emptied on creation and
    /// removed on drop.
    dir: PathBuf,
    fps: u32,
    /// Retention window in PTS units (`retention_secs · fps`).
    retention_pts: i64,
    retention_secs: u32,
    /// Monotonic segment counter for unique filenames.
    seq: u64,
    /// Finalized segments, oldest first.
    segments: VecDeque<Segment>,
    /// The currently-growing segment (the newest GOP), if any.
    cur: Option<OpenSegment>,
    /// PTS of the most recent packet pushed (the eviction/slice anchor).
    newest_pts: i64,
    total_bytes: u64,
    total_packets: u64,
    total_keyframes: u64,
    /// Lifetime count of evicted packets (diagnostics).
    dropped: u64,
    /// Latched on the first disk error; the ring then stops buffering.
    failed: bool,
}

impl DiskPacketRing {
    /// New disk ring spooling to `dir` (created/emptied here), retaining
    /// ~`retention_secs` of video encoded at `fps`. Errors only if the directory
    /// can't be prepared — the caller then falls back to a RAM ring.
    pub fn new(dir: PathBuf, fps: u32, retention_secs: u32) -> Result<Self, String> {
        let fps = fps.clamp(1, 480);
        let retention_secs = retention_secs.max(1);
        // Start from a clean directory so a previous session's segments (e.g. after
        // a crash) don't count against the buffer or leak disk.
        if dir.exists() {
            clear_dir(&dir);
        }
        fs::create_dir_all(&dir).map_err(|e| format!("create disk-buffer dir: {e}"))?;
        Ok(DiskPacketRing {
            dir,
            fps,
            retention_pts: retention_secs as i64 * fps as i64,
            retention_secs,
            seq: 0,
            segments: VecDeque::new(),
            cur: None,
            newest_pts: 0,
            total_bytes: 0,
            total_packets: 0,
            total_keyframes: 0,
            dropped: 0,
            failed: false,
        })
    }

    /// Append one freshly encoded packet: rotate to a new segment on a keyframe,
    /// write the record, then evict segments that fell out of the retention window.
    pub fn push(&mut self, pkt: EncodedPacket) {
        if self.failed {
            return;
        }
        self.newest_pts = pkt.pts;
        // Start a fresh segment on each keyframe (so every segment opens on an IDR),
        // and at the very first packet if the encoder hasn't emitted one yet.
        if pkt.keyframe || self.cur.is_none() {
            if let Err(e) = self.roll() {
                self.fail("rotate segment", e);
                return;
            }
        }
        if let Err(e) = self.write_record(&pkt) {
            self.fail("write packet", e);
            return;
        }
        self.evict();
    }

    /// Copy out the most recent `secs` seconds as a contiguous packet run that
    /// starts on a keyframe — ready for [`crate::core::mux::write_clip`] (no
    /// re-encode). Returns packets in encode/presentation order; empty if nothing
    /// has been buffered yet (or the ring has failed).
    ///
    /// Mirrors [`PacketRing::slice_last`](crate::core::buffer::PacketRing::slice_last):
    /// starts at the latest segment whose opening IDR has `pts ≤ (newest − secs·fps)`,
    /// or the earliest segment when the buffer is shorter than the request.
    pub fn slice_last(&mut self, secs: u32) -> Vec<EncodedPacket> {
        if self.failed {
            return Vec::new();
        }
        // Flush the open segment so its bytes are readable from disk.
        if let Some(open) = self.cur.as_mut() {
            if open.writer.flush().is_err() {
                // Best-effort: a flush failure just means the tail may be short.
            }
        }
        // Ordered list of every segment holding data: finalized first, then the
        // still-open one.
        let mut metas: Vec<(PathBuf, i64)> = self
            .segments
            .iter()
            .map(|s| (s.path.clone(), s.first_pts))
            .collect();
        if let Some(open) = self.cur.as_ref() {
            if open.seg.packets > 0 {
                metas.push((open.seg.path.clone(), open.seg.first_pts));
            }
        }
        if metas.is_empty() {
            return Vec::new();
        }

        let want_start = self.newest_pts - (secs as i64).saturating_mul(self.fps as i64);
        // Latest segment whose opening keyframe is at or before the cut point;
        // fall back to the earliest segment if the buffer is shorter than asked.
        let mut chosen = 0usize;
        for (i, (_, first)) in metas.iter().enumerate() {
            if *first <= want_start {
                chosen = i;
            }
        }

        let mut out = Vec::new();
        for (path, _) in &metas[chosen..] {
            if let Err(e) = read_segment_into(path, &mut out) {
                tracing::warn!("disk buffer: reading segment {} failed: {e}", path.display());
            }
        }
        out
    }

    /// Drop everything (e.g. on capture restart): discard the open segment and
    /// delete all segment files.
    pub fn clear(&mut self) {
        self.cur = None;
        self.segments.clear();
        clear_dir(&self.dir);
        self.total_bytes = 0;
        self.total_packets = 0;
        self.total_keyframes = 0;
    }

    /// Snapshot of buffer health, matching [`PacketRing::stats`](crate::core::buffer::PacketRing::stats).
    pub fn stats(&self) -> BufferStats {
        let oldest = self
            .segments
            .front()
            .map(|s| s.first_pts)
            .or_else(|| {
                self.cur
                    .as_ref()
                    .filter(|o| o.seg.packets > 0)
                    .map(|o| o.seg.first_pts)
            })
            .unwrap_or(self.newest_pts);
        let duration_secs = (self.newest_pts - oldest).max(0) as f64 / self.fps.max(1) as f64;
        BufferStats {
            packets: self.total_packets as usize,
            keyframes: self.total_keyframes as usize,
            bytes: self.total_bytes as usize,
            duration_secs,
            retention_secs: self.retention_secs,
            dropped: self.dropped,
        }
    }

    // --- internals ---------------------------------------------------------

    /// Finalize the current segment and open a fresh one for the next GOP.
    fn roll(&mut self) -> std::io::Result<()> {
        self.finalize_current()?;
        let path = self.dir.join(format!("seg_{:08}.hbuf", self.seq));
        self.seq += 1;
        let file = File::create(&path)?;
        self.cur = Some(OpenSegment {
            seg: Segment {
                path,
                first_pts: i64::MAX,
                last_pts: i64::MIN,
                bytes: 0,
                packets: 0,
                keyframes: 0,
            },
            writer: BufWriter::new(file),
        });
        Ok(())
    }

    /// Flush + move the open segment into the finalized deque (or delete it if it
    /// never received a packet).
    fn finalize_current(&mut self) -> std::io::Result<()> {
        if let Some(mut open) = self.cur.take() {
            open.writer.flush()?;
            if open.seg.packets > 0 {
                self.segments.push_back(open.seg);
            } else {
                let _ = fs::remove_file(&open.seg.path);
            }
        }
        Ok(())
    }

    fn write_record(&mut self, pkt: &EncodedPacket) -> std::io::Result<()> {
        let open = self
            .cur
            .as_mut()
            .expect("write_record called with no open segment");
        let w = &mut open.writer;
        w.write_all(&pkt.pts.to_le_bytes())?;
        w.write_all(&pkt.dts.to_le_bytes())?;
        w.write_all(&[pkt.keyframe as u8])?;
        w.write_all(&(pkt.data.len() as u32).to_le_bytes())?;
        w.write_all(&pkt.data)?;

        let s = &mut open.seg;
        if s.first_pts == i64::MAX {
            s.first_pts = pkt.pts;
        }
        s.last_pts = pkt.pts;
        s.bytes += pkt.data.len() as u64;
        s.packets += 1;
        if pkt.keyframe {
            s.keyframes += 1;
        }
        self.total_bytes += pkt.data.len() as u64;
        self.total_packets += 1;
        if pkt.keyframe {
            self.total_keyframes += 1;
        }
        Ok(())
    }

    /// Delete whole leading segments while the *next* segment's opening keyframe is
    /// already older than the retention window (keeps one GOP of slack, exactly
    /// like the RAM ring). The open segment is the newest GOP and is never evicted.
    fn evict(&mut self) {
        while self.segments.len() >= 2 {
            let second_start = self.segments[1].first_pts;
            if self.newest_pts - second_start < self.retention_pts {
                break;
            }
            let seg = self.segments.pop_front().unwrap();
            self.total_bytes = self.total_bytes.saturating_sub(seg.bytes);
            self.total_packets = self.total_packets.saturating_sub(seg.packets);
            self.total_keyframes = self.total_keyframes.saturating_sub(seg.keyframes);
            self.dropped += seg.packets;
            let _ = fs::remove_file(&seg.path);
        }
    }

    /// Latch a disk failure and log it once, so a slow/full disk degrades the
    /// buffer instead of crashing capture.
    fn fail(&mut self, what: &str, e: std::io::Error) {
        if !self.failed {
            tracing::warn!("disk buffer disabled ({what} failed: {e}); clips will be empty until restart");
            self.failed = true;
        }
    }
}

impl Drop for DiskPacketRing {
    fn drop(&mut self) {
        // Release the open file handle, then remove the whole private directory so
        // the buffer never outlives the capture session on disk.
        self.cur = None;
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Remove every file in `dir` (best-effort; leaves the directory itself).
fn clear_dir(dir: &std::path::Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Read all framed records from a segment file, appending decoded packets to `out`.
fn read_segment_into(path: &std::path::Path, out: &mut Vec<EncodedPacket>) -> std::io::Result<()> {
    let bytes = fs::read(path)?;
    let mut off = 0usize;
    while off + RECORD_HEADER <= bytes.len() {
        let pts = i64::from_le_bytes(bytes[off..off + 8].try_into().unwrap());
        let dts = i64::from_le_bytes(bytes[off + 8..off + 16].try_into().unwrap());
        let keyframe = bytes[off + 16] & 1 != 0;
        let len = u32::from_le_bytes(bytes[off + 17..off + 21].try_into().unwrap()) as usize;
        off += RECORD_HEADER;
        if off + len > bytes.len() {
            // Truncated trailing record (e.g. a torn write) — stop cleanly.
            break;
        }
        out.push(EncodedPacket {
            data: bytes[off..off + len].to_vec(),
            pts,
            dts,
            keyframe,
        });
        off += len;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disk ring in a per-test private dir (`name` keeps concurrently-run tests
    /// from sharing a directory and clobbering each other's segments).
    fn ring(name: &str, fps: u32, retention: u32) -> DiskPacketRing {
        let base = std::env::temp_dir().join(format!("hako_disk_buf_test_{name}"));
        let _ = fs::remove_dir_all(&base);
        DiskPacketRing::new(base, fps, retention).expect("create disk ring")
    }

    fn pkt(pts: i64, keyframe: bool) -> EncodedPacket {
        EncodedPacket {
            data: vec![pts as u8; if keyframe { 100 } else { 10 }],
            pts,
            dts: pts,
            keyframe,
        }
    }

    fn fill(r: &mut DiskPacketRing, n: i64, gop: i64) {
        for i in 0..n {
            r.push(pkt(i, i % gop == 0));
        }
    }

    #[test]
    fn empty_ring_is_inert() {
        let mut r = ring("empty", 60, 30);
        assert!(r.slice_last(30).is_empty());
        assert_eq!(r.stats().bytes, 0);
        assert_eq!(r.stats().packets, 0);
    }

    #[test]
    fn roundtrips_packets_through_disk() {
        let mut r = ring("roundtrip", 10, 100); // big retention: nothing evicted
        fill(&mut r, 100, 10); // pts 0..=99, keyframes at 0,10,...,90

        // Last 1 s ⇒ want_start = 99 − 10 = 89 ⇒ latest opening IDR ≤ 89 is the
        // segment that starts at pts 80.
        let s = r.slice_last(1);
        assert!(s.first().unwrap().keyframe, "clip must start on a keyframe");
        assert_eq!(s.first().unwrap().pts, 80);
        assert_eq!(s.last().unwrap().pts, 99);
        assert_eq!(s.len(), 20);
        // Payload survived the disk round-trip intact.
        assert_eq!(s.first().unwrap().data, vec![80u8; 100]);
    }

    #[test]
    fn slice_longer_than_buffer_returns_from_earliest_segment() {
        let mut r = ring("slice_long", 10, 2); // retains ~3 GOPs of slack
        fill(&mut r, 100, 10);

        let s = r.slice_last(5);
        assert!(s.first().unwrap().keyframe);
        // Earliest retained segment starts at the kept-slack keyframe (pts 70).
        assert_eq!(s.first().unwrap().pts, 70);
        assert_eq!(s.last().unwrap().pts, 99);
    }

    #[test]
    fn evicts_gop_aligned_and_keeps_one_gop_of_slack() {
        let mut r = ring("evict", 10, 2); // retention = 20 PTS, GOP = 10 frames
        fill(&mut r, 100, 10);

        // Same worked example as the RAM ring: front lands on the keyframe at 70,
        // retained span 70..99.
        let s = r.slice_last(1_000_000);
        assert_eq!(s.first().unwrap().pts, 70);
        let stats = r.stats();
        assert!((stats.duration_secs - 2.9).abs() < 1e-6, "got {}", stats.duration_secs);
        assert_eq!(stats.keyframes, 3); // kf at 70, 80, 90
        assert!(stats.dropped >= 70, "expected ~70 evicted, got {}", stats.dropped);
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut r = ring("clear", 10, 100);
        fill(&mut r, 50, 10);
        r.clear();
        assert!(r.slice_last(30).is_empty());
        assert_eq!(r.stats().packets, 0);
    }
}
