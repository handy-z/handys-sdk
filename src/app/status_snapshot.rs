//! Pieces of a game's read-only `--status <file>` snapshot.
use std::path::Path;
use std::time::{Duration, Instant};

use super::single_instance::SingleInstance;
use super::updater_thread::{Updater, status_name};

/// A second Updater would race the running app's files: when `mutex_name` is held, writes
/// "<app> is running; close it first" to `out` and returns false.
pub fn snapshot_allowed(mutex_name: &str, app: &str, out: &Path) -> bool {
    if !SingleInstance::is_running(mutex_name) {
        return true;
    }
    let _ = std::fs::write(out, format!("{app} is running; close it first\n"));
    false
}

/// status_name, plus " - <detail>" when there is one, e.g. "Waiting for offsets - retry in 30 s".
pub fn status_line(u: &Updater) -> String {
    let (s, d) = (status_name(u.status()), u.detail());
    if d.is_empty() {
        s.to_owned()
    } else {
        format!("{s} - {d}")
    }
}

/// Calls `done` every `every` until it returns true (then true) or `timeout` has passed (then false).
pub fn wait_for(mut done: impl FnMut() -> bool, timeout: Duration, every: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if done() {
            return true;
        }
        std::thread::sleep(every);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_wait_for() {
        let mut calls = 0;
        assert!(wait_for(
            || {
                calls += 1;
                calls == 3
            },
            Duration::from_secs(5),
            Duration::from_millis(1)
        ));
        assert_eq!(calls, 3);
        assert!(!wait_for(|| false, Duration::from_millis(20), Duration::from_millis(5)));
    }
}
