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
pub fn load_session_for_pid(config: &Path, pid: u32) -> Option<ClaudeSession> {
    let path = sessions_dir(config).join(format!("{pid}.json"));
    let bytes = fs::read(&path).ok()?;
    let session: ClaudeSession = serde_json::from_slice(&bytes).ok()?;
    if session.pid != pid || session.session_id.is_empty() {
        return None;
    }
    Some(session)
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
    fn rejects_missing_or_mismatched_pid() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = sessions_dir(dir.path());
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("1.json"),
            r#"{"sessionId":"abc","pid":99,"cwd":"/tmp","status":"idle"}"#,
        )
        .unwrap();
        assert!(load_session_for_pid(dir.path(), 1).is_none());
        assert!(load_session_for_pid(dir.path(), 99).is_none());
    }
}
