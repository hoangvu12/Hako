//! A short-TTL cache over one [`catalog`] scan, shared by the integration's
//! detection and the status labeler so a tick does at most one process/window
//! sweep.
//!
//! Unlike [`crate::games::detected_game`] (which can scan on demand via a closure),
//! a generic scan needs the [`AppHandle`] — it reads the custom-games table + the
//! process snapshot. So the integration's `run` loop *pushes* a fresh scan into
//! this cache each tick via [`refresh`], and the app-less readers ([`find_window`],
//! the status snapshot) just read the last result via [`current_generic`]. The
//! loop refreshes ~every 2 s and the read is valid within [`STALE`], so the label
//! stays fresh while the loop runs and goes quiet (not stale-wrong) if it stops.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::AppHandle;

use super::{catalog, DetectedGame};

/// How long a pushed scan stays valid for the app-less readers. Comfortably longer
/// than the integration's poll interval so every read between refreshes hits, but
/// short enough that a stopped loop stops labeling within a couple seconds.
const STALE: Duration = Duration::from_secs(5);

/// The last scan pushed by [`refresh`]: `(when, result)`. `None` until the first
/// refresh.
static CACHE: Mutex<Option<(Instant, Option<DetectedGame>)>> = Mutex::new(None);

/// Re-scan the catalog and store the result. Called once per tick by the generic
/// integration's `run` loop (the only caller with an [`AppHandle`]).
pub fn refresh(app: &AppHandle) -> Option<DetectedGame> {
    let found = catalog::detect_generic_game(app);
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((Instant::now(), found.clone()));
    }
    found
}

/// The last-detected generic game, if a [`refresh`] within [`STALE`] found one.
/// Read by [`super::integration::Integration::find_window`] and the status
/// snapshot; returns `None` once the cache goes stale (loop stopped / game exited).
pub fn current_generic() -> Option<DetectedGame> {
    let cache = CACHE.lock().ok()?;
    match cache.as_ref() {
        Some((at, val)) if at.elapsed() < STALE => val.clone(),
        _ => None,
    }
}
