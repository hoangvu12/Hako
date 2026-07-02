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
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// Default freshness for the per-second detection callers: refresh at most once a
/// second across all of them.
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(1);

/// The shared table + when it was last refreshed. `None` until first use (`System`
/// construction isn't `const`, so it can't be built in the static initializer).
static SNAPSHOT: Mutex<Option<(System, Instant)>> = Mutex::new(None);

/// Run `f` over a process snapshot no older than `max_age`, refreshing the shared
/// table first only if the last refresh is older than that. Serialized across
/// callers by the mutex, so concurrent pollers coalesce onto one refresh.
fn with_processes<T>(max_age: Duration, f: impl FnOnce(&System) -> T) -> T {
    let mut guard = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
    let stale = guard.as_ref().map_or(true, |(_, at)| at.elapsed() >= max_age);
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
