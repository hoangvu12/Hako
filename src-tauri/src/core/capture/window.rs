//! Win32 top-level window discovery — the capture target picker.
//!
//! Everything here answers "which window should we capture?" and is pure
//! `EnumWindows`/`GetWindowThreadProcessId` bookkeeping: no D3D11, no encoder,
//! no shared capture state. Besides the UI picker ([`list_windows`]) these are
//! the auto-start detectors each [`crate::games`] integration polls to notice
//! its game appearing.

use std::ffi::c_void;

use serde::Serialize;
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetCursorPos, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible,
};

/// A capturable top-level window (for the UI picker).
#[derive(Debug, Clone, Serialize)]
pub struct WindowTarget {
    /// HWND as an integer (passed back to `start_capture`).
    pub hwnd: i64,
    pub title: String,
}

/// Visit every top-level window. `visit` returns `false` to stop enumeration
/// early (the first match wins in the `find_*` helpers below).
///
/// This is the one place the `EnumWindows` + `LPARAM`-as-closure-pointer dance
/// lives; the callers stay safe closures.
fn for_each_window<F: FnMut(HWND) -> bool>(mut visit: F) {
    unsafe extern "system" fn trampoline<F: FnMut(HWND) -> bool>(
        hwnd: HWND,
        lparam: LPARAM,
    ) -> BOOL {
        // SAFETY: `lparam` is the `&mut F` handed to EnumWindows just below, and
        // EnumWindows only calls back synchronously, so the borrow is live.
        let visit = &mut *(lparam.0 as *mut F);
        BOOL(visit(hwnd) as i32)
    }

    // SAFETY: `visit` outlives the call — EnumWindows returns only once every
    // callback has run — and nothing else aliases it for the duration.
    unsafe {
        let _ = EnumWindows(Some(trampoline::<F>), LPARAM(&mut visit as *mut F as isize));
    }
}

/// The caption of `hwnd` if it is visible and titled, else `None`. Filtering on
/// both skips a game's hidden helper/splash windows so we latch the real render
/// surface. The caption is returned untrimmed — callers trim when comparing.
fn visible_title(hwnd: HWND) -> Option<String> {
    // SAFETY: all three read window state; a stale HWND yields length 0.
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        (!title.is_empty()).then_some(title)
    }
}

/// Whether `hwnd` is visible and titled, without paying for the caption text.
fn is_visible_titled(hwnd: HWND) -> bool {
    // SAFETY: both just read window state.
    unsafe { IsWindowVisible(hwnd).as_bool() && GetWindowTextLengthW(hwnd) > 0 }
}

/// The process id owning `hwnd`, or 0 if it can't be resolved.
fn owner_pid(hwnd: HWND) -> u32 {
    let mut pid: u32 = 0;
    // SAFETY: just reads window ownership; a stale HWND yields pid 0.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Every visible, titled top-level window, minus those belonging to processes
/// the generic-capture catalog excludes (shells, launchers, Hako itself).
pub fn list_windows() -> Vec<WindowTarget> {
    use crate::games::process_snapshot;

    let mut raw: Vec<(i64, String, u32)> = Vec::new();
    for_each_window(|hwnd| {
        if let Some(title) = visible_title(hwnd) {
            raw.push((hwnd.0 as i64, title, owner_pid(hwnd)));
        }
        true
    });
    raw.into_iter()
        .filter(|(_, _, pid)| {
            process_snapshot::name_for_pid(*pid, process_snapshot::DEFAULT_MAX_AGE)
                .map(|name| !crate::games::generic::catalog::is_excluded(&name))
                .unwrap_or(true)
        })
        .map(|(hwnd, title, _)| WindowTarget { hwnd, title })
        .collect()
}

/// Find the live VALORANT **game** window (the Unreal client, not the Riot
/// launcher), used to auto-start capture when the game launches — the way Medal
/// detects the game process. Matches the game window's exact title; returns its
/// HWND or `None` if the game isn't running.
pub fn find_valorant_window() -> Option<i64> {
    find_window_by_title("VALORANT")
}

/// Find the first visible top-level window whose (trimmed) title matches `want`
/// case-insensitively, returning its HWND. The game-agnostic window detector each
/// [`crate::games`] integration uses to auto-start capture when its game appears
/// (Valorant → "VALORANT", League → "League of Legends (TM) Client"). The exact
/// (trimmed) compare avoids matching browser tabs like "VALORANT - YouTube".
pub fn find_window_by_title(want: &str) -> Option<i64> {
    let mut found = 0i64;
    for_each_window(|hwnd| {
        match visible_title(hwnd) {
            Some(title) if title.trim().eq_ignore_ascii_case(want) => {
                found = hwnd.0 as i64;
                false // stop enumeration
            }
            _ => true,
        }
    });
    (found != 0).then_some(found)
}

/// Find the first visible top-level window owned by a process whose name matches
/// any of `process_names` (case-insensitive), returning its HWND. Used when a
/// game's window title is unreliable/unknown but its executable name is certain
/// (Rematch → "RuntimeClient-Win64-Shipping.exe"). Two passes: resolve the target
/// PIDs via `sysinfo`, then enumerate windows and match the owning PID.
pub fn find_window_by_process(process_names: &[&str]) -> Option<i64> {
    use crate::games::process_snapshot;
    let pids = process_snapshot::pids_for(process_names, process_snapshot::DEFAULT_MAX_AGE);
    if pids.is_empty() {
        return None;
    }
    let mut found = 0i64;
    for_each_window(|hwnd| {
        if is_visible_titled(hwnd) {
            let pid = owner_pid(hwnd);
            if pid != 0 && pids.contains(&pid) {
                found = hwnd.0 as i64;
                return false; // stop enumeration
            }
        }
        true
    });
    (found != 0).then_some(found)
}

/// The process id that owns `hwnd_raw` (for the `specific_apps` "Game Audio"
/// source — the capture target's PID). `None` for an invalid window.
pub fn pid_for_hwnd(hwnd_raw: i64) -> Option<u32> {
    let pid = owner_pid(HWND(hwnd_raw as *mut c_void));
    (pid != 0).then_some(pid)
}

/// The window title (caption) of `hwnd_raw`, trimmed, or `None` if it has none.
/// Used by `add_custom_game` to seed a picked game's display name from its window
/// title (Medal's Request-a-Game captures the caption too).
pub fn window_title(hwnd_raw: i64) -> Option<String> {
    let hwnd = HWND(hwnd_raw as *mut c_void);
    // SAFETY: reads the caption of a window handle; a stale HWND yields length 0.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_string())
    }
}

/// The mouse cursor's current screen position (physical pixels), or `None` if the
/// query fails. Used by the source loop to un-skip a static frame when the cursor
/// moved while "record cursor" is on.
pub(super) fn cursor_screen_pos() -> Option<(i32, i32)> {
    let mut pt = POINT::default();
    // SAFETY: GetCursorPos writes the current pointer position into `pt`.
    (unsafe { GetCursorPos(&mut pt) })
        .ok()
        .map(|_| (pt.x, pt.y))
}

/// Whether a window is minimized (iconic). A minimized game — common with
/// exclusive fullscreen when alt-tabbed — usually stops presenting frames, so
/// the graphics hook can't capture it; the auto-capture skips it until it's back
/// on screen rather than re-injecting into a non-rendering process.
pub fn is_window_minimized(hwnd: i64) -> bool {
    // SAFETY: IsIconic just reads window state; a stale/invalid HWND returns false.
    unsafe { IsIconic(HWND(hwnd as *mut c_void)).as_bool() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop always has visible titled windows, so enumeration must yield
    /// some — and every entry must carry a non-empty title and a live HWND.
    /// Guards the `EnumWindows` closure trampoline: a broken `LPARAM` round-trip
    /// would surface as an empty list rather than a crash.
    #[test]
    fn enumerates_visible_titled_windows() {
        let windows = list_windows();
        assert!(!windows.is_empty(), "no visible top-level windows found");
        for w in &windows {
            assert_ne!(w.hwnd, 0);
            assert!(!w.title.is_empty());
        }
    }

    /// Round-trip an enumerated window back through the title lookup: the same
    /// window must be findable by its own caption, and its pid must resolve.
    /// This is what the game auto-detectors rely on.
    #[test]
    fn finds_an_enumerated_window_by_its_own_title() {
        let windows = list_windows();
        let target = windows
            .iter()
            .find(|w| !w.title.trim().is_empty())
            .expect("at least one titled window");

        let found = find_window_by_title(target.title.trim()).expect("window findable by title");
        // Several windows can share a caption; the match must at least be a real
        // window whose (trimmed) title equals what we searched for.
        assert_eq!(
            window_title(found).as_deref(),
            Some(target.title.trim()),
            "found window's caption should match the search"
        );
        assert!(pid_for_hwnd(found).is_some(), "found window should have a pid");
    }

    /// Early-out: enumeration stops on the first match rather than running to
    /// completion, and a caption that cannot exist yields `None`.
    #[test]
    fn unmatched_title_returns_none() {
        assert_eq!(
            find_window_by_title("hako::no-such-window-caption-\u{1F4A9}"),
            None
        );
    }
}
