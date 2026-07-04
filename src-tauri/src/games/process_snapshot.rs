//! Shared, rate-limited process-table snapshot.
//!
//! Every per-game detection loop used to build its own `sysinfo::System` and run a
//! full `refresh_processes_specifics(All, …)` — three loops × a few Hz each meant
//! constant process-table walks + kernel churn while a game runs. This consolidates
//! them onto one shared table that refreshes **at most once per `max_age`** no
//! matter how many callers poll, so N callers at 1 Hz cost one scan per second
//! total instead of N.
//!
//! Process-name only (`ProcessRefreshKind::nothing()`), matching what every ported
//! caller already requested — we only ever read `Process::name()` here.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Default freshness for the per-second detection callers: refresh at most once a
/// second across all of them.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(1);

/// Freshness for the exe-path table ([`processes_with_paths`]). Path enumeration
/// is heavier than the name-only walk and Steam detection doesn't need 1 Hz, so
/// it refreshes far less often — and on its own table, so name-only callers never
/// pay for it.
pub const PATHS_MAX_AGE: Duration = Duration::from_secs(5);

/// The shared table + when it was last refreshed. `None` until first use (`System`
/// construction isn't `const`, so it can't be built in the static initializer).
static SNAPSHOT: Mutex<Option<(System, Instant)>> = Mutex::new(None);

/// The exe-path table, separate from [`SNAPSHOT`] so `any_running`/`pids_for`/
/// `name_for_pid` stay name-only and cheap. Populated with full exe paths.
static PATHS: Mutex<Option<(System, Instant)>> = Mutex::new(None);

/// Run `f` over a process snapshot no older than `max_age`, refreshing the shared
/// table first only if the last refresh is older than that. Serialized across
/// callers by the mutex, so concurrent pollers coalesce onto one refresh.
fn with_processes<T>(max_age: Duration, f: impl FnOnce(&System) -> T) -> T {
    let mut guard = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let stale = guard
        .as_ref()
        .map_or(true, |(_, at)| at.elapsed() >= max_age);
    if stale {
        match guard.as_mut() {
            Some((sys, at)) => {
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing(),
                );
                *at = Instant::now();
            }
            None => {
                let mut sys = System::new();
                sys.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    ProcessRefreshKind::nothing(),
                );
                *guard = Some((sys, Instant::now()));
            }
        }
    }
    let (sys, _) = guard.as_ref().expect("snapshot populated above");
    f(sys)
}

/// Whether any running process's name matches one of `names` (case-insensitive).
pub fn any_running(names: &[&str], max_age: Duration) -> bool {
    with_processes(max_age, |sys| {
        sys.processes().values().any(|p| {
            p.name()
                .to_str()
                .map(|n| names.iter().any(|w| n.eq_ignore_ascii_case(w)))
                .unwrap_or(false)
        })
    })
}

/// The lowercase exe file name of the process with pid `pid`, from the shared
/// name-only snapshot (no path refresh — Steam's full-path detection is a
/// separate, coarser table added in Phase 2). `None` if the pid isn't in the
/// table or its name isn't valid UTF-8. Used by `add_custom_game` to resolve a
/// picked window's owning exe.
pub fn name_for_pid(pid: u32, max_age: Duration) -> Option<String> {
    with_processes(max_age, |sys| {
        sys.process(sysinfo::Pid::from_u32(pid))
            .and_then(|p| p.name().to_str())
            .map(|n| n.to_ascii_lowercase())
    })
}

/// PIDs of all running processes whose name matches one of `names`
/// (case-insensitive). Empty when none match.
pub fn pids_for(names: &[&str], max_age: Duration) -> HashSet<u32> {
    with_processes(max_age, |sys| {
        sys.processes()
            .iter()
            .filter(|(_, p)| {
                p.name()
                    .to_str()
                    .map(|n| names.iter().any(|w| n.eq_ignore_ascii_case(w)))
                    .unwrap_or(false)
            })
            .map(|(pid, _)| pid.as_u32())
            .collect()
    })
}

/// Lowercase names of all running processes, from the shared name-only snapshot
/// (deduped only insofar as the OS reports them — repeats are fine for callers
/// that look each name up in a set). Used by the generic curated scan to match
/// running exes against the bundled `games.json` in one pass.
pub fn running_names(max_age: Duration) -> Vec<String> {
    with_processes(max_age, |sys| {
        sys.processes()
            .values()
            .filter_map(|p| p.name().to_str().map(|n| n.to_ascii_lowercase()))
            .collect()
    })
}

/// Run `f` over the exe-path snapshot no older than `max_age`. Mirrors
/// [`with_processes`] but on the separate [`PATHS`] table, refreshed with exe
/// paths (`ProcessRefreshKind::with_exe`) so `Process::exe()` is populated.
fn with_paths<T>(max_age: Duration, f: impl FnOnce(&System) -> T) -> T {
    let mut guard = PATHS.lock().unwrap_or_else(|e| e.into_inner());
    let stale = guard
        .as_ref()
        .map_or(true, |(_, at)| at.elapsed() >= max_age);
    if stale {
        let kind = ProcessRefreshKind::nothing().with_exe(UpdateKind::Always);
        match guard.as_mut() {
            Some((sys, at)) => {
                sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
                *at = Instant::now();
            }
            None => {
                let mut sys = System::new();
                sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
                *guard = Some((sys, Instant::now()));
            }
        }
    }
    let (sys, _) = guard.as_ref().expect("paths snapshot populated above");
    f(sys)
}

/// `(pid, lowercase exe file name, full exe path)` for every process whose exe
/// path is readable, from the coarser-cadence [`PATHS`] table (refreshed at most
/// once per `max_age`). Processes with no accessible exe path are skipped. Used by
/// the generic Steam scan to spot `steamapps\common\` installs.
pub fn processes_with_paths(max_age: Duration) -> Vec<(u32, String, PathBuf)> {
    with_paths(max_age, |sys| {
        sys.processes()
            .iter()
            .filter_map(|(pid, p)| {
                let exe = p.exe()?;
                let name = exe.file_name()?.to_str()?.to_ascii_lowercase();
                Some((pid.as_u32(), name, exe.to_path_buf()))
            })
            .collect()
    })
}
