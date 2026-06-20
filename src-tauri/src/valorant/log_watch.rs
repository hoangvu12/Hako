//! `ShooterGame.log` tailer — precise round-boundary anchors.
//!
//! Ported from Medal's `LogFileListener` + `ValorantRoundHandler`. Riot has no
//! real-time kill event, but it *does* log round boundaries, and those give us
//! exact round-start wall-clocks to anchor kill→PTS reconciliation against
//! (`reconcile::calibrate_match_start`).
//!
//! Mechanics:
//! - Tail the log incrementally: seek to end on open, poll ~1 s, read new lines.
//! - On `AShooterGameState::OnRoundEnded for round 'N'`: stamp the **read-time**
//!   wall-clock (QPC, same 100-ns domain as WGC `SystemRelativeTime` / packet
//!   timestamps) as round N's end, and seed round N+1's start with a coarse
//!   `end + buy phase` **fallback** (30 s, or 20 s for Spike Rush).
//! - On `Gameplay started ... (server time > 0)`: the round actually went live
//!   (barriers dropped — the `roundTime = 0` reference for every kill). Stamp its
//!   read-time as the precise round start, overriding the fallback. This is what
//!   makes the 45 s half-start / overtime rounds anchor correctly: a fixed 30 s
//!   guess lands their `roundTime = 0` 15 s early, dragging every reconciled
//!   seek-bar marker in the match off by the same 15 s.
//!   (We tail within ~1 s, so read-time stands in for the log's `[...]` stamp.)
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

/// Marker Valorant logs when a round goes live (barriers drop) and when its buy
/// phase begins — both as `Gameplay started at local time X (server time Y)`.
const GAMEPLAY_STARTED_MARKER: &str = "Gameplay started at local time";

/// The `server time` seconds from a `Gameplay started at local time X (server
/// time Y)` line, else `None`. Valorant logs this twice per round: once at the
/// **buy-phase start** with `server time 0.000000`, and once when the round goes
/// **live** (barriers drop, the `roundTime = 0` reference for every kill) with a
/// non-zero server time — `~30 s`, or `~45 s` on the half-start / overtime rounds.
/// Verified live (release-12.11): the value is exactly `30.292187` / `45.292187`.
pub fn parse_gameplay_started_server_secs(line: &str) -> Option<f64> {
    if !line.contains(GAMEPLAY_STARTED_MARKER) {
        return None;
    }
    let after = line.split("server time ").nth(1)?;
    let num: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    num.parse().ok()
}

/// Whether `line` is the round-goes-live ("Gameplay started", non-zero server
/// time) log line — the exact `roundTime = 0` instant we anchor reconciliation
/// on. Its buy-phase-start twin (`server time 0.000000`) returns false. Using
/// this line directly is buy-phase-agnostic, so the 45 s half-start / overtime
/// rounds anchor correctly instead of landing 15 s early off a fixed 30 s guess.
pub fn is_round_live(line: &str) -> bool {
    parse_gameplay_started_server_secs(line).is_some_and(|s| s > GAMEPLAY_LIVE_MIN_SECS)
}

/// Lower bound separating the live barrier-drop server time (~30 s / ~45 s) from
/// the buy-phase-start twin (exactly `0.0`).
const GAMEPLAY_LIVE_MIN_SECS: f64 = 1.0;

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

/// Largest age (ms) we'll back-date a log line by. We read the log on a ~2 s
/// poll, so a real line is at most a few seconds old; anything older means the
/// embedded timestamp can't be trusted in the QPC domain (system-clock step, a
/// timezone surprise, or a stale line re-read after a log rotation), so we fall
/// back to read-time stamping rather than yank an anchor seconds/hours away.
const MAX_BACKDATE_MS: i64 = 15_000;

/// Parse the leading `[YYYY.MM.DD-HH.MM.SS:mmm]` **UTC** timestamp Valorant
/// stamps on every log line into milliseconds since the Unix epoch. `None` if the
/// line doesn't start with that bracketed stamp. (Verified live, release-12.11:
/// the bracket is UTC — a match-end line at `[..11.52.30..]` matched our own
/// `11:52:31Z` processing time, not the `18:52` local wall clock.)
pub fn parse_log_timestamp_unix_ms(line: &str) -> Option<i64> {
    let inner = line.strip_prefix('[')?;
    let ts = &inner[..inner.find(']')?]; // "2026.06.20-10.20.47:174"
    let (date, time) = ts.split_once('-')?;
    let mut d = date.split('.');
    let year: i64 = d.next()?.parse().ok()?;
    let month: i64 = d.next()?.parse().ok()?;
    let day: i64 = d.next()?.parse().ok()?;
    let (hms, millis) = time.split_once(':')?;
    let mut t = hms.split('.');
    let hh: i64 = t.next()?.parse().ok()?;
    let mm: i64 = t.next()?.parse().ok()?;
    let ss: i64 = t.next()?.parse().ok()?;
    let millis: i64 = millis.parse().ok()?;
    let secs = days_from_civil(year, month, day) * 86_400 + hh * 3_600 + mm * 60 + ss;
    Some(secs * 1000 + millis)
}

/// Days since 1970-01-01 for a proleptic-Gregorian (y, m, d). Howard Hinnant's
/// `days_from_civil` — exact integer arithmetic, no date dependency.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// System wall clock in Unix ms (UTC), for measuring a log line's age against its
/// embedded timestamp.
fn system_now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The true event wall-clock (QPC 100-ns ticks) for a log line, back-dated from
/// read time to the line's own `[UTC]` timestamp.
///
/// We drain the log on the orchestrator's ~2 s presence poll, so stamping a round
/// boundary with `now_ticks()` at read time lands it **up to 2 s late** — and
/// since one round anchor calibrates the whole match, that drags every reconciled
/// seek-bar marker the same ~2 s late (measured: kills sat ~1.8 s before their
/// markers). The line carries the moment it was actually written, so we recover
/// the real event time as `now_ticks − (system_now − log_time)`. `now_ticks` and
/// `system_now` are sampled together, so their difference is just the line's age;
/// QPC and the wall clock both advance in real time, so subtracting the age in the
/// QPC domain is exact. Falls back to read time for an unparseable / implausibly
/// old line (see [`MAX_BACKDATE_MS`]).
pub fn line_event_ticks(line: &str) -> i64 {
    let now_ticks = now_ticks();
    let Some(log_ms) = parse_log_timestamp_unix_ms(line) else {
        return now_ticks;
    };
    backdate_ticks(now_ticks, system_now_unix_ms(), log_ms)
}

/// Pure core of [`line_event_ticks`]: back-date `now_ticks` by the line's age
/// (`now_unix_ms − log_unix_ms`), or return it unchanged when the age is negative
/// or beyond [`MAX_BACKDATE_MS`].
fn backdate_ticks(now_ticks: i64, now_unix_ms: i64, log_unix_ms: i64) -> i64 {
    let age_ms = now_unix_ms - log_unix_ms;
    if age_ms < 0 || age_ms > MAX_BACKDATE_MS {
        return now_ticks;
    }
    now_ticks - age_ms * (TICKS_PER_SECOND / 1000)
}

/// Tracks round end/start wall-clocks from log lines. Pure; the IO layer feeds
/// it round-ended + round-live markers stamped with `now_ticks`.
///
/// A round's start is the **barrier-drop** (`roundTime = 0`, what every kill's
/// `roundTime` counts from): we anchor it on the `Gameplay started` (server time
/// > 0) line via [`on_round_live`](Self::on_round_live). The `OnRoundEnded` line
/// only sets a coarse `end + buy phase` *fallback* (a fixed 30 s guess that is
/// 15 s short on the 45 s half-start / overtime rounds), overridden by the precise
/// barrier-drop whenever we see it.
#[derive(Debug, Default)]
pub struct RoundTracker {
    /// Set once the match is known to be live; markers before it are ignored.
    match_found_ticks: Option<i64>,
    buy_phase_ticks: i64,
    /// The round whose live start ("Gameplay started", server time > 0) we expect
    /// next: `Some(N + 1)` once round N has ended. `None` until then, so a
    /// barrier-drop line seen before we know the round number (app opened mid-game)
    /// is ignored rather than mis-anchored onto round 0.
    next_round: Option<i32>,
    /// Rounds anchored from a precise barrier-drop line — so the coarse fallback
    /// never clobbers a precise start.
    precise: std::collections::BTreeSet<i32>,
    starts: BTreeMap<i32, i64>,
    ends: BTreeMap<i32, i64>,
}

impl RoundTracker {
    pub fn new(buy_phase_ticks: i64) -> Self {
        RoundTracker {
            match_found_ticks: None,
            buy_phase_ticks,
            next_round: None,
            precise: std::collections::BTreeSet::new(),
            starts: BTreeMap::new(),
            ends: BTreeMap::new(),
        }
    }

    /// Mark the match as live (presence entered INGAME). Markers seen before this
    /// are ignored, matching Medal's `MatchFoundTime` gate.
    pub fn set_match_found(&mut self, ticks: i64) {
        self.match_found_ticks = Some(ticks);
    }

    /// Record that round `round` ended at `end_ticks`. Sets round `round`'s end,
    /// seeds round `round + 1`'s start with the coarse `end + buy phase` fallback
    /// (unless a precise barrier-drop already anchored it), and notes that the next
    /// barrier-drop line belongs to round `round + 1`.
    pub fn on_round_ended(&mut self, round: i32, end_ticks: i64) {
        if let Some(found) = self.match_found_ticks {
            if end_ticks < found {
                return; // belongs to a previous match
            }
        }
        self.ends.insert(round, end_ticks);
        if !self.precise.contains(&(round + 1)) {
            self.starts.insert(round + 1, end_ticks + self.buy_phase_ticks);
        }
        self.next_round = Some(round + 1);
    }

    /// Record that the expected round went live (barriers dropped) at
    /// `start_ticks` — the precise `roundTime = 0` anchor, replacing the
    /// buy-phase fallback. Ignored until a round has ended (we don't yet know the
    /// round number after a mid-game start).
    pub fn on_round_live(&mut self, start_ticks: i64) {
        if let Some(found) = self.match_found_ticks {
            if start_ticks < found {
                return;
            }
        }
        let Some(round) = self.next_round else {
            return; // round number unknown (no round-ended seen yet) — don't guess
        };
        self.starts.insert(round, start_ticks);
        self.precise.insert(round);
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
    fn parses_gameplay_started_server_time() {
        let live = "[2026.06.20-10.21.17:156][172]LogShooterGameState: Warning: \
                    Gameplay started at local time 30.179688 (server time 30.292187)";
        let buy = "[2026.06.20-10.20.47:174][977]LogShooterGameState: Warning: \
                   Gameplay started at local time 0.000000 (server time 0.000000)";
        assert_eq!(parse_gameplay_started_server_secs(live), Some(30.292187));
        assert_eq!(parse_gameplay_started_server_secs(buy), Some(0.0));
        assert_eq!(parse_gameplay_started_server_secs("unrelated line"), None);
        // Only the non-zero (barriers-dropped) twin counts as the round going live.
        assert!(is_round_live(live));
        assert!(!is_round_live(buy));
        // The 45 s half-start / overtime variant is also "live".
        let ot = "Gameplay started at local time 44.789062 (server time 45.292187)";
        assert!(is_round_live(ot));
    }

    #[test]
    fn tracker_seeds_buy_phase_fallback_on_round_ended() {
        let buy = 30 * TICKS_PER_SECOND;
        let mut t = RoundTracker::new(buy);
        let round0_end = 100 * TICKS_PER_SECOND;
        t.on_round_ended(0, round0_end);
        // Until the barrier-drop line arrives, round 1 carries the coarse fallback.
        let r1 = t.anchors().into_iter().find(|a| a.round == 1).unwrap();
        assert_eq!(r1.start_wallclock_ticks, round0_end + buy);
    }

    #[test]
    fn tracker_overrides_fallback_with_precise_barrier_drop() {
        // A 45 s half-start round: the fixed 30 s fallback is 15 s early, but the
        // barrier-drop line anchors the true round start exactly.
        let buy = 30 * TICKS_PER_SECOND;
        let mut t = RoundTracker::new(buy);
        let round0_end = 100 * TICKS_PER_SECOND;
        t.on_round_ended(0, round0_end); // next_round = 1, fallback = 130 s
        let live = round0_end + 45 * TICKS_PER_SECOND; // real barrier drop
        t.on_round_live(live);
        let r1 = t.anchors().into_iter().find(|a| a.round == 1).unwrap();
        assert_eq!(r1.start_wallclock_ticks, live); // precise, not the 130 s guess
        // A later duplicate round-ended must not clobber the precise start.
        t.on_round_ended(0, round0_end);
        let r1b = t.anchors().into_iter().find(|a| a.round == 1).unwrap();
        assert_eq!(r1b.start_wallclock_ticks, live);
    }

    #[test]
    fn tracker_ignores_round_live_before_any_round_ended() {
        // App opened mid-game: a barrier-drop line whose round number we can't yet
        // know must be dropped, not mis-anchored onto round 0.
        let mut t = RoundTracker::new(30 * TICKS_PER_SECOND);
        t.on_round_live(500 * TICKS_PER_SECOND);
        assert!(t.is_empty());
        // Once a round ends, the next barrier drop anchors the right round.
        t.on_round_ended(4, 600 * TICKS_PER_SECOND);
        t.on_round_live(640 * TICKS_PER_SECOND);
        let r5 = t.anchors().into_iter().find(|a| a.round == 5).unwrap();
        assert_eq!(r5.start_wallclock_ticks, 640 * TICKS_PER_SECOND);
    }

    #[test]
    fn parses_utc_log_timestamp() {
        // The real match-end line, cross-checked live: 11:52:30.218 UTC =
        // 1781956350218 ms (our own clip at 11:52:32.046 = 1781956352046 is 1.8 s
        // later, exactly the poll lag this back-dating removes).
        let line = "[2026.06.20-11.52.30:218][865]LogPlatformSessionManager: \
                    Loopstate changed from INGAME to MENUS";
        assert_eq!(parse_log_timestamp_unix_ms(line), Some(1_781_956_350_218));
        // Epoch sanity + a non-bracketed line.
        assert_eq!(parse_log_timestamp_unix_ms("[1970.01.01-00.00.00:000]x"), Some(0));
        assert_eq!(parse_log_timestamp_unix_ms("no timestamp here"), None);
    }

    #[test]
    fn backdate_recovers_true_event_time() {
        // Line read at QPC 100 s (1e9 ticks), system clock says 10.000 s, the line
        // was stamped at 8.200 s → it's 1.8 s old → its event tick is 1.8 s before
        // read time: 1e9 − 1.8 s·1e7 = 982_000_000.
        assert_eq!(backdate_ticks(1_000_000_000, 10_000, 8_200), 982_000_000);
        // No timestamp drift → no change.
        assert_eq!(backdate_ticks(1_000_000_000, 10_000, 10_000), 1_000_000_000);
        // A negative age (clock skew) or an implausibly old line degrades to read
        // time rather than yanking the anchor.
        assert_eq!(backdate_ticks(1_000_000_000, 10_000, 10_500), 1_000_000_000);
        assert_eq!(backdate_ticks(1_000_000_000, 100_000, 10_000), 1_000_000_000);
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
