//! Read Claude Code's live session registry under `~/.claude/sessions/`.
//!
//! Claude writes one `<pid>.json` per interactive process with `sessionId`,
//! `cwd`, and `status` (`idle` / busy variants). Exact resume uses
//! `claude -r <sessionId>`, never `-c/--continue`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One live Claude process as published under `sessions/<pid>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSession {
    pub session_id: String,
    pub pid: u32,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub status: String,
}

impl ClaudeSession {
    pub fn is_idle(&self) -> bool {
        self.status == "idle"
    }
}

/// Config root: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    dirs_home()
        .map(|home| home.join(".claude"))
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn sessions_dir(config: &Path) -> PathBuf {
    config.join("sessions")
}

/// Load the registry entry for a live Claude pid, if present and valid.
/// The filename is the live pid; the body `pid` can lag after `/resume`.
pub fn load_session_for_pid(config: &Path, pid: u32) -> Option<ClaudeSession> {
    let path = sessions_dir(config).join(format!("{pid}.json"));
    let bytes = fs::read(&path).ok()?;
    let mut session: ClaudeSession = serde_json::from_slice(&bytes).ok()?;
    if session.session_id.is_empty() {
        return None;
    }
    session.pid = pid;
    Some(session)
}

/// Find a registry row for any of `pids`, including Linux children.
///
/// The board scan keeps the lowest pid in a pane (the wrapper). Real Claude
/// writes `sessions/<node-pid>.json` on the child, so looking at only the
/// wrapper leaves status empty and `z` used to reject the pane as not
/// sleepable.
pub fn load_session_for_pids(config: &Path, pids: &[u32]) -> Option<ClaudeSession> {
    let mut search: Vec<u32> = pids.to_vec();
    for pid in pids {
        search.extend(child_pids(*pid));
    }
    search.sort_unstable();
    search.dedup();
    search
        .iter()
        .find_map(|pid| load_session_for_pid(config, *pid))
}

/// Direct children of `pid`. Empty on macOS (no `/proc`); the pane's other
/// scanned pids still cover an `exec -a claude` child.
fn child_pids(pid: u32) -> Vec<u32> {
    let path = format!("/proc/{pid}/task/{pid}/children");
    fs::read_to_string(path)
        .map(|text| {
            text.split_whitespace()
                .filter_map(|word| word.parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{load_session_for_pid, sessions_dir, ClaudeSession};
    use std::fs;

    #[test]
    fn parses_idle_session_registry_row() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions_dir(dir.path());
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("4242.json"),
            r#"{
              "sessionId": "7c61f52f-5b66-46e7-a5dc-e2283947dfd1",
              "pid": 4242,
              "cwd": "/tmp/ww",
              "status": "idle",
              "peerFeatures": ["notify_idle"]
            }"#,
        )
        .unwrap();
        let session = load_session_for_pid(dir.path(), 4242).unwrap();
        assert_eq!(
            session,
            ClaudeSession {
                session_id: "7c61f52f-5b66-46e7-a5dc-e2283947dfd1".into(),
                pid: 4242,
                cwd: "/tmp/ww".into(),
                status: "idle".into(),
            }
        );
        assert!(session.is_idle());
    }

    #[test]
    fn filename_pid_wins_when_body_lags() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions_dir(dir.path());
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("1.json"),
            r#"{"sessionId":"abc","pid":99,"cwd":"/tmp","status":"idle"}"#,
        )
        .unwrap();
        let session = super::load_session_for_pid(dir.path(), 1).unwrap();
        assert_eq!(session.session_id, "abc");
        assert_eq!(session.pid, 1);
        assert!(super::load_session_for_pid(dir.path(), 99).is_none());
    }

    #[test]
    fn load_session_for_pids_finds_the_child_registry_row() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions_dir(dir.path());
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("1010.json"),
            r#"{"sessionId":"sid-child","pid":1010,"cwd":"/tmp","status":"idle"}"#,
        )
        .unwrap();
        let session = super::load_session_for_pids(dir.path(), &[1000, 1010]).unwrap();
        assert_eq!(session.session_id, "sid-child");
        assert_eq!(session.pid, 1010);
        assert!(super::load_session_for_pids(dir.path(), &[1000]).is_none());
    }
}
