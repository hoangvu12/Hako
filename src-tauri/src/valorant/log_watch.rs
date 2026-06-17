//! `ShooterGame.log` tailer — precise round-boundary anchors.
//!
//! Ported from Medal's `LogFileListener` + `ValorantRoundHandler`. Riot has no
//! real-time kill event, but it *does* log round boundaries, and those give us
//! exact round-start wall-clocks to anchor kill→PTS reconciliation against
//! (`reconcile::calibrate_match_start`).
//!
//! Mechanics, matching Medal:
//! - Tail the log incrementally: seek to end on open, poll ~1 s, read new lines.
//! - On `AShooterGameState::OnRoundEnded for round 'N'`: stamp the **read-time**
//!   wall-clock (QPC, same 100-ns domain as WGC `SystemRelativeTime` / packet
//!   timestamps) as round N's end, and round N+1's start = end + buy phase
//!   (30 s, or 20 s for Spike Rush). Medal parses the log's `[...]` timestamp but
//!   falls back to "now"; since we tail within ~1 s we use "now" directly.
//!
//! The pure parsing + [`RoundTracker`] are unit-tested; [`LogTail`] is the thin
//! IO layer the orchestrator drives.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use crate::core::clock::TICKS_PER_SECOND;
use crate::valorant::reconcile::RoundAnchor;

/// Marker Medal keys round ends off of.
const ROUND_ENDED_MARKER: &str = "AShooterGameState::OnRoundEnded";

/// Medal's poll cadence / read batch.
pub const POLL_INTERVAL_MS: u64 = 1000;
const READ_BATCH_BYTES: u64 = 204_800;

/// Resolve `ShooterGame.log`, trying Medal's three known locations in order.
pub fn log_path() -> Option<PathBuf> {
    // 1) %LOCALAPPDATA%\VALORANT\Saved\Logs\ShooterGame.log  (the live one)
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(&local)
            .join("VALORANT")
            .join("Saved")
            .join("Logs")
            .join("ShooterGame.log");
        if p.exists() {
            return Some(p);
        }
    }
    // 2) %PROGRAMDATA%\Riot Games\Logs\VALORANT\ShooterGame.log
    if let Some(common) = std::env::var_os("PROGRAMDATA") {
        let p = PathBuf::from(&common)
            .join("Riot Games")
            .join("Logs")
            .join("VALORANT")
            .join("ShooterGame.log");
        if p.exists() {
            return Some(p);
        }
    }
    // 3) %USERPROFILE%\Documents\Riot Games\VALORANT\Logs\ShooterGame.log
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let p = PathBuf::from(&profile)
            .join("Documents")
            .join("Riot Games")
            .join("VALORANT")
            .join("Logs")
            .join("ShooterGame.log");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Marker Medal reads the live client version off of (primary source, ahead of
/// the public version API).
const CLIENT_VERSION_MARKER: &str = "CI server version:";

/// Extract the client release version from a `... CI server version: <ver>`
/// line, else `None`. Case-insensitive on the marker (Medal uses
/// `OrdinalIgnoreCase`); returns the trimmed remainder.
pub fn parse_ci_server_version(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let marker = CLIENT_VERSION_MARKER.to_ascii_lowercase();
    let idx = lower.find(&marker)?;
    let after = line[idx + marker.len()..].trim();
    (!after.is_empty()).then(|| after.to_string())
}

/// Scan `ShooterGame.log` for the `CI server version:` line and return the
/// client release version, or `None` if the log is missing or hasn't logged it
/// yet. Medal's primary client-version source (falls back to valorant-api.com).
pub fn client_version_from_log() -> Option<String> {
    use std::io::{BufRead, BufReader};
    let path = log_path()?;
    // Share-read: Valorant keeps the log open for writing.
    let file = std::fs::OpenOptions::new().read(true).open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Some(v) = parse_ci_server_version(&line) {
            return Some(v);
        }
    }
    None
}

/// Round number from an `OnRoundEnded ... for round 'N'` line, else `None`.
/// Manual parse (no regex dep), mirroring Medal's `for round '(\d+)'`.
pub fn parse_round_ended(line: &str) -> Option<i32> {
    if !line.contains(ROUND_ENDED_MARKER) {
        return None;
    }
    let after = line.split("for round '").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// `LogPlatformSessionManager: Loopstate changed from ... to INGAME` (match start).
pub fn is_match_start(line: &str) -> bool {
    line.contains("LogPlatformSessionManager: Loopstate changed from") && line.contains("to INGAME")
}

/// `LogPlatformSessionManager: Loopstate changed from INGAME` (match end).
pub fn is_match_end(line: &str) -> bool {
    line.contains("LogPlatformSessionManager: Loopstate changed from INGAME")
}

/// Buy-phase duration in 100-ns ticks. Medal: Spike Rush = 20 s, else 30 s.
pub fn buy_phase_ticks(game_mode: &str) -> i64 {
    let secs = if game_mode == "Spike Rush" { 20 } else { 30 };
    secs * TICKS_PER_SECOND
}

/// `QueryPerformanceCounter` in 100-ns ticks — same domain as WGC
/// `SystemRelativeTime`, so anchors line up with session packet timestamps.
#[cfg(windows)]
pub fn now_ticks() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
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

/// Tracks round end/start wall-clocks from log lines. Pure; the IO layer feeds
/// it `(round, now_ticks)` pairs. Mirrors Medal's `ValorantGameState` round map:
/// round N ended now → round N+1 starts at now + buy phase.
#[derive(Debug, Default)]
pub struct RoundTracker {
    /// Set once the match is known to be live; round ends before it are ignored.
    match_found_ticks: Option<i64>,
    buy_phase_ticks: i64,
    starts: BTreeMap<i32, i64>,
    ends: BTreeMap<i32, i64>,
}

impl RoundTracker {
    pub fn new(buy_phase_ticks: i64) -> Self {
        RoundTracker {
            match_found_ticks: None,
            buy_phase_ticks,
            starts: BTreeMap::new(),
            ends: BTreeMap::new(),
        }
    }

    /// Mark the match as live (presence entered INGAME). Round ends seen before
    /// this are ignored, matching Medal's `MatchFoundTime` gate.
    pub fn set_match_found(&mut self, ticks: i64) {
        self.match_found_ticks = Some(ticks);
    }

    /// Record that round `round` ended at `end_ticks`. Sets round `round`'s end
    /// and round `round + 1`'s start (= end + buy phase).
    pub fn on_round_ended(&mut self, round: i32, end_ticks: i64) {
        if let Some(found) = self.match_found_ticks {
            if end_ticks < found {
                return; // belongs to a previous match
            }
        }
        self.ends.insert(round, end_ticks);
        self.starts.insert(round + 1, end_ticks + self.buy_phase_ticks);
    }

    /// Round-start anchors gathered so far, for `reconcile::calibrate_match_start`.
    pub fn anchors(&self) -> Vec<RoundAnchor> {
        self.starts
            .iter()
            .map(|(&round, &start_wallclock_ticks)| RoundAnchor {
                round,
                start_wallclock_ticks,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }
}

/// Incremental log tailer. Open once; `poll_new_lines` returns lines appended
/// since the last poll. Resets to the start if the file shrinks (rotation).
pub struct LogTail {
    path: PathBuf,
    pos: u64,
    carry: String,
}

impl LogTail {
    /// Open at end-of-file (incremental mode) like Medal's `SeekToEnd`.
    pub fn open_at_end(path: PathBuf) -> std::io::Result<Self> {
        let end = File::open(&path)?.metadata()?.len();
        Ok(LogTail {
            path,
            pos: end,
            carry: String::new(),
        })
    }

    /// Open at the start (full scan) — used by tests / one-shot reads.
    pub fn open_at_start(path: PathBuf) -> Self {
        LogTail {
            path,
            pos: 0,
            carry: String::new(),
        }
    }

    /// Read bytes appended since the last call, split into complete lines. A
    /// partial trailing line is carried over to the next poll.
    pub fn poll_new_lines(&mut self) -> std::io::Result<Vec<String>> {
        let mut file = File::open(&self.path)?;
        let len = file.metadata()?.len();
        if len < self.pos {
            // Truncated/rotated — restart from the top.
            self.pos = 0;
            self.carry.clear();
        }
        let mut lines = Vec::new();
        let mut remaining = len.saturating_sub(self.pos);
        file.seek(SeekFrom::Start(self.pos))?;
        while remaining > 0 {
            let take = remaining.min(READ_BATCH_BYTES) as usize;
            let mut buf = vec![0u8; take];
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            self.pos += n as u64;
            remaining -= n as u64;
            self.carry.push_str(&String::from_utf8_lossy(&buf[..n]));
            while let Some(nl) = self.carry.find('\n') {
                let line: String = self.carry.drain(..=nl).collect();
                lines.push(line.trim_end().to_string());
            }
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_round_ended_number() {
        let line = "[2026.06.17-21.34.56:789][456]LogShooterGameState: \
                    AShooterGameState::OnRoundEnded for round '3'";
        assert_eq!(parse_round_ended(line), Some(3));
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse_round_ended("LogTemp: something for round '9'"), None);
        assert_eq!(parse_round_ended("AShooterGameState::OnRoundEnded no number"), None);
    }

    #[test]
    fn parses_ci_server_version() {
        let line = "[2026.06.17-21.00.00:000][0]LogShooter: Display: \
                    CI server version: 11.02.00.123456";
        assert_eq!(
            parse_ci_server_version(line).as_deref(),
            Some("11.02.00.123456")
        );
        // Case-insensitive marker, trimmed value.
        assert_eq!(
            parse_ci_server_version("ci SERVER version:   9.0.1   ").as_deref(),
            Some("9.0.1")
        );
        assert_eq!(parse_ci_server_version("no version here"), None);
        assert_eq!(parse_ci_server_version("CI server version:   "), None);
    }

    #[test]
    fn detects_match_start_and_end_markers() {
        assert!(is_match_start(
            "LogPlatformSessionManager: Loopstate changed from MENUS to INGAME"
        ));
        assert!(is_match_end(
            "LogPlatformSessionManager: Loopstate changed from INGAME to MENUS"
        ));
        assert!(!is_match_start(
            "LogPlatformSessionManager: Loopstate changed from INGAME to MENUS"
        ));
    }

    #[test]
    fn buy_phase_matches_medal() {
        assert_eq!(buy_phase_ticks("Spike Rush"), 20 * TICKS_PER_SECOND);
        assert_eq!(buy_phase_ticks("Standard"), 30 * TICKS_PER_SECOND);
        assert_eq!(buy_phase_ticks(""), 30 * TICKS_PER_SECOND);
    }

    #[test]
    fn tracker_sets_next_round_start_after_buy_phase() {
        let buy = 30 * TICKS_PER_SECOND;
        let mut t = RoundTracker::new(buy);
        let round0_end = 100 * TICKS_PER_SECOND;
        t.on_round_ended(0, round0_end);
        let anchors = t.anchors();
        // Round 1 starts buy-phase after round 0 ended.
        let r1 = anchors.iter().find(|a| a.round == 1).unwrap();
        assert_eq!(r1.start_wallclock_ticks, round0_end + buy);
    }

    #[test]
    fn tracker_ignores_round_ends_before_match_found() {
        let mut t = RoundTracker::new(30 * TICKS_PER_SECOND);
        t.set_match_found(1_000 * TICKS_PER_SECOND);
        t.on_round_ended(0, 500 * TICKS_PER_SECOND); // stale, pre-match
        assert!(t.is_empty());
        t.on_round_ended(0, 1_100 * TICKS_PER_SECOND); // this match
        assert!(!t.is_empty());
    }
}
