//! Scan planning and the exclusive daemon ownership lock.
//! Seen/started markers retain their existing TUI/hook input channels.
//!
//! The lock is `flock`: the kernel drops it if this process dies, even
//! when Drop does not run.

use std::fs::{self, File, OpenOptions};
use std::path::Path;

use fs2::FileExt;

use crate::discover::parse_host_line;
use crate::HostLine;

/// Home session first so the open board gets titles before remote
/// `list-panes` (zab is last alphabetically and used to wait for everyone).
pub fn refresh_sessions(from_scan: &[String], home: &str) -> Vec<String> {
    let mut sessions = from_scan.to_vec();
    sessions.sort();
    sessions.dedup();
    if home.is_empty() {
        return sessions;
    }
    if let Some(pos) = sessions.iter().position(|session| session == home) {
        let home = sessions.remove(pos);
        sessions.insert(0, home);
    }
    sessions
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

#[cfg(test)]
mod tests {
    use super::{refresh_sessions, sessions_from_scan, try_acquire_lock};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn refresh_sessions_puts_home_first() {
        assert_eq!(
            refresh_sessions(&["ww".into(), "zab".into(), "lp".into()], "zab"),
            ["zab", "lp", "ww"]
        );
        assert_eq!(
            refresh_sessions(&["ww".into(), "lp".into()], ""),
            ["lp", "ww"]
        );
    }

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
        let path = dir.join("daemon.lock");
        let first = try_acquire_lock(&path).expect("first writer");
        assert!(try_acquire_lock(&path).is_none());
        drop(first);
        assert!(try_acquire_lock(&path).is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
