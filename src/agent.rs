//! One row on the board. Identity is (session, pane), same as zj-agent-mob.

use crate::status::Status;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct AgentId {
    pub session: String,
    pub pane_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub id: AgentId,
    pub tool: String,
    pub status: Status,
    pub workspace: Option<String>,
    pub tab_name: String,
    pub tab_position: Option<usize>,
    pub pane_title: String,
    pub detail: String,
}

impl Agent {
    pub fn place_path(&self) -> String {
        let tab = if self.tab_name.is_empty() {
            "-"
        } else {
            self.tab_name.as_str()
        };
        let pane = if self.pane_title.is_empty() {
            format!("#{}", self.id.pane_id)
        } else {
            self.pane_title.clone()
        };
        format!("{} > {} > {}", self.id.session, tab, pane)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanePlace {
    pub tab_position: usize,
    pub tab_name: String,
    pub pane_title: String,
}

/// Cursor CLI wrapper argv: keep interactive `agent`, drop `agent ls` and friends.
pub fn keep_cursor_agent(argv: &[String]) -> bool {
    let bin = argv
        .first()
        .map(|s| s.rsplit('/').next().unwrap_or(s))
        .unwrap_or("");
    if bin != "agent" && bin != "cursor-agent" {
        return false;
    }
    let skip = [
        "ls", "status", "whoami", "login", "logout", "update", "about",
    ];
    !argv.iter().skip(1).any(|arg| skip.contains(&arg.as_str()))
}

/// Best-effort `--workspace` from an argv list the process scan will supply.
pub fn workspace_from_argv(argv: &[String]) -> Option<String> {
    let mut args = argv.iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            return Some(value.to_string());
        }
        if arg == "--workspace" {
            return args.next().cloned();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{keep_cursor_agent, workspace_from_argv};

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn keeps_a_live_agent_and_drops_ls() {
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--workspace",
            "/tmp/w",
            "--continue"
        ])));
        assert!(!keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "ls"
        ])));
        assert!(!keep_cursor_agent(&argv(&["vim", "src/main.rs"])));
        assert!(!keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--use-system-ca",
            "index.js",
            "ls"
        ])));
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--use-system-ca",
            "index.js",
            "--workspace",
            "/tmp/w"
        ])));
    }

    #[test]
    fn reads_workspace_flag() {
        assert_eq!(
            workspace_from_argv(&argv(&["agent", "--workspace", "/tmp/w"])).as_deref(),
            Some("/tmp/w")
        );
        assert_eq!(
            workspace_from_argv(&argv(&["agent", "--workspace=/tmp/w"])).as_deref(),
            Some("/tmp/w")
        );
    }
}
