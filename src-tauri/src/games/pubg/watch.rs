//! Locating PUBG replay directories under `%LOCALAPPDATA%\TslGame\Saved\Demos\`.
//!
//! No `FileSystemWatcher` dependency (Medal's approach): the integration polls
//! [`demo_dirs`] each tick and parses any directory it hasn't handled yet. A
//! "demo directory" is simply any folder that contains a `PUBG.replayinfo`
//! header; PUBG nests replays a level or two under `Demos\`, so we walk a small
//! bounded depth.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The header file that marks a directory as a match replay.
const REPLAY_INFO: &str = "PUBG.replayinfo";

/// How deep below `Demos\` to search for replay dirs (PUBG nests shallowly; this
/// bounds the walk so a stray deep tree can't blow up a poll tick).
const MAX_DEPTH: usize = 3;

/// The `Demos` root (`%LOCALAPPDATA%\TslGame\Saved\Demos`), or `None` when
/// `LOCALAPPDATA` is unset / the folder doesn't exist yet (no replays recorded).
pub fn demos_root() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let root = Path::new(&local)
        .join("TslGame")
        .join("Saved")
        .join("Demos");
    root.is_dir().then_some(root)
}

/// Every replay directory currently under `Demos\`, newest first (by the
/// `PUBG.replayinfo` modification time). Cheap enough to call each poll tick.
pub fn demo_dirs() -> Vec<PathBuf> {
    let Some(root) = demos_root() else {
        return Vec::new();
    };
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    collect(&root, 0, &mut found);
    // Newest replay first, so the integration handles the latest finished match.
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Recursively collect directories containing a `PUBG.replayinfo`, up to
/// [`MAX_DEPTH`]. A dir that *is* a replay isn't descended into further.
fn collect(dir: &Path, depth: usize, out: &mut Vec<(SystemTime, PathBuf)>) {
    let info = dir.join(REPLAY_INFO);
    if info.is_file() {
        let mtime = std::fs::metadata(&info)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        out.push((mtime, dir.to_path_buf()));
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            collect(&entry.path(), depth + 1, out);
        }
    }
}
