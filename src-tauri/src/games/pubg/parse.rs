//! PUBG replay-directory parsing: the `PUBG.replayinfo` header plus the
//! `events/` + `data/` JSON sidecars, mirroring Medal's `PUBGParser`.
//!
//! A finished match's replay dir looks like:
//! ```text
//! <demo dir>/
//!   PUBG.replayinfo        header: record-user nickname, match-start Unix ms, length
//!   events/ kill* groggy* Etc0*   { "time1": <ms since match start>, ... }
//!   data/   kill* groggy* Etc0*   the who/what for the matching event file
//! ```
//! An `events/<name>` file pairs with the `data/<name>` file of the **same file
//! name**. `data/kill*` and `data/groggy*` are plain JSON; `data/Etc0*` files are
//! lightly obfuscated "UE4 strings" (4-byte LE length prefix, every byte +1) and
//! are decoded before parsing. Each event's absolute wall-clock is
//! `replayinfo.timestamp + time1` (Unix ms); the integration anchors that onto the
//! capture clock to reconcile against the recorded session.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::games::event::EventKind;

/// One derived PUBG highlight, at an absolute wall-clock (Unix milliseconds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubgEvent {
    pub kind: EventKind,
    /// Absolute event time in Unix milliseconds (`replayinfo.timestamp + time1`).
    pub unix_ms: i64,
}

/// A fully parsed, finished match replay.
#[derive(Debug, Clone)]
pub struct ParsedDemo {
    /// The local (recording) player's in-game nickname.
    pub user: String,
    /// Match start, Unix milliseconds (`replayinfo.timestamp`).
    pub start_unix_ms: i64,
    /// Derived highlights (may be empty — e.g. a quick death with no kills).
    pub events: Vec<PubgEvent>,
}

/// `PUBG.replayinfo` (the subset we consume).
#[derive(Debug, Clone, Default, Deserialize)]
struct ReplayInfo {
    #[serde(rename = "recordUserNickName", default)]
    record_user_nick_name: String,
    /// Match start, Unix milliseconds.
    #[serde(default)]
    timestamp: i64,
    /// Match length in ms — non-zero only once the replay is *finalized*, so we
    /// use it as the "match complete" gate.
    #[serde(rename = "lengthInMs", default)]
    length_in_ms: i64,
}

/// An `events/<kind>*` file: just the event's ms offset since match start.
#[derive(Debug, Clone, Default, Deserialize)]
struct EventMeta {
    #[serde(default)]
    time1: i64,
}

/// A decoded `data/Etc0*` file (the only UE4-encoded data sidecar we read).
#[derive(Debug, Clone, Default, Deserialize)]
struct EtcData {
    #[serde(rename = "etceteraEventCode", default)]
    etcetera_event_code: String,
    #[serde(rename = "targetName", default)]
    target_name: String,
}

/// Parse a finished match's replay directory, or `None` if it isn't a
/// finalized match yet (header missing / unparseable / `lengthInMs == 0`, i.e.
/// the replay is still being written). A returned demo may have zero `events`.
pub fn parse_demo(dir: &Path) -> Option<ParsedDemo> {
    let info = parse_replay_info(dir)?;
    // Gate on a finalized replay: `lengthInMs` is 0 until PUBG closes the match.
    if info.length_in_ms <= 0 || info.record_user_nick_name.is_empty() {
        return None;
    }
    let user = info.record_user_nick_name;
    let start = info.timestamp;

    let mut events = Vec::new();
    collect_combat(dir, "kill", &user, start, EventKind::Kill, EventKind::Death, &mut events);
    collect_combat(
        dir,
        "groggy",
        &user,
        start,
        EventKind::Knockdown,
        EventKind::Knockdown,
        &mut events,
    );
    collect_chicken_dinner(dir, &user, start, &mut events);
    events.sort_by_key(|e| e.unix_ms);
    Some(ParsedDemo {
        user,
        start_unix_ms: start,
        events,
    })
}

/// Read + parse `PUBG.replayinfo` (stripping any control-character wrapping).
fn parse_replay_info(dir: &Path) -> Option<ReplayInfo> {
    let raw = std::fs::read_to_string(dir.join("PUBG.replayinfo")).ok()?;
    serde_json::from_str(strip_to_object(&raw)?).ok()
}

/// Fold a `kill`/`groggy` event family into `out`: pair each `events/<prefix>*`
/// file with its `data/<prefix>*` twin (same file name), attribute it to the
/// local player, and classify it as `we_did` (we're the actor) or `to_us` (we're
/// the victim). Rows that don't involve us are skipped.
fn collect_combat(
    dir: &Path,
    prefix: &str,
    user: &str,
    start_unix_ms: i64,
    we_did: EventKind,
    to_us: EventKind,
    out: &mut Vec<PubgEvent>,
) {
    let metas = read_event_metas(dir, prefix);
    let data = read_string_maps(dir, prefix);
    for (name, meta) in &metas {
        let Some(fields) = data.get(name) else {
            continue;
        };
        let kind = if fields.get("victimName").is_some_and(|v| v == user) {
            to_us
        } else if fields.values().any(|v| v == user) {
            we_did
        } else {
            continue;
        };
        out.push(PubgEvent {
            kind,
            unix_ms: start_unix_ms + meta.time1,
        });
    }
}

/// Fold the "last survivor" (Chicken Dinner ⇒ [`EventKind::Victory`]) event, if
/// the local player won. Its `data/Etc0*` twin is a UE4-encoded string.
fn collect_chicken_dinner(dir: &Path, user: &str, start_unix_ms: i64, out: &mut Vec<PubgEvent>) {
    let metas = read_event_metas(dir, "Etc0");
    let data_dir = dir.join("data");
    for (name, meta) in &metas {
        let path = data_dir.join(name);
        let Some(decoded) = decode_ue4_file(&path) else {
            continue;
        };
        let Some(obj) = strip_to_object(&decoded) else {
            continue;
        };
        let Ok(etc) = serde_json::from_str::<EtcData>(obj) else {
            continue;
        };
        if etc.etcetera_event_code == "LastSurvivor" && etc.target_name == user {
            out.push(PubgEvent {
                kind: EventKind::Victory,
                unix_ms: start_unix_ms + meta.time1,
            });
        }
    }
}

/// Read every `events/<prefix>*` file into `file name → EventMeta`.
fn read_event_metas(dir: &Path, prefix: &str) -> HashMap<String, EventMeta> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir.join("events")) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(obj) = strip_to_object(&raw) {
            if let Ok(meta) = serde_json::from_str::<EventMeta>(obj) {
                out.insert(name, meta);
            }
        }
    }
    out
}

/// Read every plain-JSON `data/<prefix>*` file into `file name → {field: value}`.
fn read_string_maps(dir: &Path, prefix: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir.join("data")) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(obj) = strip_to_object(&raw) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(obj) {
                out.insert(name, map);
            }
        }
    }
    out
}

/// Slice a string to its outermost `{ … }` (PUBG sidecars can carry leading /
/// trailing control bytes). `None` if there's no balanced-looking object.
fn strip_to_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&s[start..=end])
}

/// Decode a UE4-encoded string file: a 4-byte little-endian length prefix
/// followed by `len` bytes, each stored **minus one** (0 bytes pass through), and
/// a possible trailing NUL. Mirrors Medal's `DecodeUE4String(path, 1)`. `None` on
/// any read / length error.
fn decode_ue4_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(decode_ue4_bytes(&bytes))
}

/// Decode the UE4-obfuscated payload from raw file bytes (see [`decode_ue4_file`]).
fn decode_ue4_bytes(bytes: &[u8]) -> String {
    if bytes.len() < 4 {
        return String::new();
    }
    let len = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]).max(0) as usize;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        // Only `len` bytes follow the prefix; tolerate a short file.
        let b = bytes.get(4 + i).copied().unwrap_or(0);
        // Encoded bytes are stored +1 (Medal's `encoded_offset = 1`); a genuine 0
        // byte is left as-is rather than wrapping to 255.
        out.push(if b > 0 { b.wrapping_add(1) } else { 0 });
    }
    // Drop a single trailing NUL terminator if present.
    if out.last() == Some(&0) {
        out.pop();
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip helper: encode a plaintext string the way PUBG's UE4 writer does
    /// (4-byte LE length prefix, every byte −1, trailing NUL), so the decoder can
    /// be tested against a value we control.
    fn encode_ue4(plain: &str) -> Vec<u8> {
        let mut body: Vec<u8> = plain.bytes().map(|b| b.wrapping_sub(1)).collect();
        body.push(0); // trailing NUL
        let mut out = (body.len() as i32).to_le_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn ue4_decode_round_trips() {
        let json = r#"{"etceteraEventCode":"LastSurvivor","targetName":"Me"}"#;
        let encoded = encode_ue4(json);
        assert_eq!(decode_ue4_bytes(&encoded), json);
    }

    #[test]
    fn ue4_decode_tolerates_short_and_empty() {
        assert_eq!(decode_ue4_bytes(&[]), "");
        // Prefix claims 3 bytes but the body is missing: decodes to NUL padding,
        // which carries no JSON object — harmless (skipped) downstream.
        assert!(strip_to_object(&decode_ue4_bytes(&[3, 0, 0, 0])).is_none());
    }

    #[test]
    fn strip_to_object_trims_wrapping() {
        assert_eq!(strip_to_object("\u{1}{\"a\":1}\u{0}"), Some("{\"a\":1}"));
        assert_eq!(strip_to_object("no braces"), None);
    }

    #[test]
    fn parse_demo_reads_a_finished_match() {
        let dir = tempdir();
        write(&dir.join("PUBG.replayinfo"), br#"{"recordUserNickName":"Me","timestamp":1000000,"lengthInMs":600000}"#);
        std::fs::create_dir_all(dir.join("events")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();

        // A kill by us at +5s.
        write(&dir.join("events/kill_0"), br#"{"time1":5000}"#);
        write(&dir.join("data/kill_0"), br#"{"killerName":"Me","victimName":"Enemy"}"#);
        // A death (we're the victim) at +10s.
        write(&dir.join("events/kill_1"), br#"{"time1":10000}"#);
        write(&dir.join("data/kill_1"), br#"{"killerName":"Enemy","victimName":"Me"}"#);
        // We knock someone at +3s.
        write(&dir.join("events/groggy_0"), br#"{"time1":3000}"#);
        write(&dir.join("data/groggy_0"), br#"{"attackerName":"Me","victimName":"Enemy"}"#);
        // A kill not involving us — ignored.
        write(&dir.join("events/kill_2"), br#"{"time1":7000}"#);
        write(&dir.join("data/kill_2"), br#"{"killerName":"A","victimName":"B"}"#);
        // Chicken Dinner (UE4-encoded data).
        write(&dir.join("events/Etc0_0"), br#"{"time1":600000}"#);
        std::fs::write(
            dir.join("data/Etc0_0"),
            encode_ue4(r#"{"etceteraEventCode":"LastSurvivor","targetName":"Me"}"#),
        )
        .unwrap();

        let demo = parse_demo(&dir).expect("finished match parses");
        assert_eq!(demo.user, "Me");
        let kinds: Vec<EventKind> = demo.events.iter().map(|e| e.kind).collect();
        // Sorted by time: groggy(3s) Knockdown, kill(5s) Kill, death(10s) Death,
        // chicken(600s) Victory. The A-vs-B kill is absent.
        assert_eq!(
            kinds,
            vec![
                EventKind::Knockdown,
                EventKind::Kill,
                EventKind::Death,
                EventKind::Victory,
            ]
        );
        // Absolute times = timestamp + time1.
        assert_eq!(demo.events[0].unix_ms, 1_003_000);
        assert_eq!(demo.events[1].unix_ms, 1_005_000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_demo_none_until_finalized() {
        let dir = tempdir();
        // lengthInMs == 0 ⇒ replay still being written ⇒ not ready.
        write(&dir.join("PUBG.replayinfo"), br#"{"recordUserNickName":"Me","timestamp":1,"lengthInMs":0}"#);
        assert!(parse_demo(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unique temp directory for a test (no external tempfile crate needed).
    fn tempdir() -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!(
            "hako_pubg_test_{}_{}",
            std::process::id(),
            TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&base).unwrap();
        base
    }
    static TEST_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }
}
