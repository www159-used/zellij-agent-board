//! One host process writes the store. The TUI only reads.
//!
//! The lock is `flock`: the kernel drops it if this process dies, even
//! when Drop does not run.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::discover::parse_host_line;
use crate::protocol::{ensure_state, persist_places, persist_scan, runtime_dir};
use crate::scan::{scan_host_text, scan_places_for};
use crate::HostLine;

pub fn reconcile_lock_path() -> PathBuf {
    runtime_dir().join("reconcile.lock")
}

/// Sessions that actually have a SCAN row. Empty sessions are not listed.
pub fn sessions_from_scan(text: &str) -> Vec<String> {
    let mut sessions: Vec<String> = text
        .lines()
        .filter_map(parse_host_line)
        .filter_map(|line| match line {
            HostLine::Scan(found) if !found.id.session.is_empty() => Some(found.id.session),
            _ => None,
        })
        .collect();
    sessions.sort();
    sessions.dedup();
    sessions
}

pub struct ReconcileLock {
    /// Held so the flock stays until this process exits.
    _file: File,
}

/// Exclusive writer. `flock` is released by the kernel on abort / SIGKILL.
pub fn try_acquire_lock(path: &Path) -> Option<ReconcileLock> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .ok()?;
    file.try_lock_exclusive().ok()?;
    Some(ReconcileLock { _file: file })
}

/// One pass under the lock. Used when the TUI has no snapshot yet.
pub fn reconcile_once() -> bool {
    ensure_state();
    let Some(_lock) = try_acquire_lock(&reconcile_lock_path()) else {
        return false;
    };
    reconcile_pass();
    true
}

/// Same as [`reconcile_once`]: lock, write, exit.
pub fn run_reconcile() -> bool {
    reconcile_once()
}

fn reconcile_pass() {
    let text = scan_host_text();
    persist_scan(&text);
    for session in sessions_from_scan(&text) {
        persist_places(scan_places_for(&[session]));
    }
}

#[cfg(test)]
mod tests {
    use super::{sessions_from_scan, try_acquire_lock};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sessions_from_scan_skips_empty_sessions() {
        let text = "\
META hooks=1
SCAN ww 3 agent /bin/agent --workspace /tmp/ww
SCAN lp 8 agent /bin/agent --workspace /tmp/lp
SCAN ww 4 agent /bin/agent --workspace /tmp/ww
";
        assert_eq!(sessions_from_scan(text), ["lp", "ww"]);
        assert!(sessions_from_scan("META hooks=1\n").is_empty());
    }

    #[test]
    fn second_lock_fails_while_the_first_is_held() {
        let dir = std::env::temp_dir().join(format!(
            "zab-lock-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reconcile.lock");
        let first = try_acquire_lock(&path).expect("first writer");
        assert!(try_acquire_lock(&path).is_none());
        drop(first);
        assert!(try_acquire_lock(&path).is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
