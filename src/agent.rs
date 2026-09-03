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
    /// Plugin tick when status last changed (legacy; working elapsed uses started_at).
    pub status_since: u64,
    /// Host unix epoch when this working turn began.
    pub started_at: Option<u64>,
    /// Host unix epoch when the agent finished (done rows).
    pub finished_at: Option<u64>,
    /// This done cycle was opened (board jump or the pane was just focused).
    pub visited: bool,
}

impl Agent {
    pub fn unread_done(&self) -> bool {
        self.status == Status::Done && !self.visited
    }

    pub fn project(&self) -> &str {
        self.workspace
            .as_deref()
            .map(|path| {
                path.trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(path)
            })
            .filter(|name| !name.is_empty())
            .unwrap_or("-")
    }

    pub fn display_task(&self) -> &str {
        self.pane_title.as_str()
    }

    pub fn tab_label(&self) -> String {
        if !self.tab_name.is_empty() {
            self.tab_name.clone()
        } else if let Some(position) = self.tab_position {
            (position + 1).to_string()
        } else {
            "-".to_string()
        }
    }

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

impl PanePlace {
    /// Keep a previously known tab/pane name when a later update is blank.
    /// Opening the board in another session often sends empty titles for
    /// everyone else; done/time still arrive from hooks.
    pub fn keep_names(self, tab_name: &str, pane_title: &str) -> Self {
        Self {
            tab_position: self.tab_position,
            tab_name: if self.tab_name.is_empty() {
                tab_name.to_string()
            } else {
                self.tab_name
            },
            pane_title: if self.pane_title.is_empty() {
                pane_title.to_string()
            } else {
                self.pane_title
            },
        }
    }
}

/// Keep interactive sessions, drop one-shot subcommands. Bins and skip
/// lists come from the catalog; `agent ls` is still kept here so the host
/// scan can decide picker vs live chat.
pub fn keep_cursor_agent(argv: &[String]) -> bool {
    crate::catalog::keep_process(argv)
}

/// Short CLI badge from the catalog. Unknown tools get a 2-char fallback.
pub fn tool_label(tool: &str) -> String {
    crate::catalog::badge_for(tool).0
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
        // `ls` may be a resumed chat; the host scan drops pure pickers.
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "ls"
        ])));
        assert!(!keep_cursor_agent(&argv(&["vim", "src/main.rs"])));
        assert!(!keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--use-system-ca",
            "index.js",
            "status"
        ])));
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--use-system-ca",
            "index.js",
            "--workspace",
            "/tmp/w"
        ])));
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--use-system-ca",
            "index.js",
            "ls"
        ])));
    }

    #[test]
    fn keeps_codebuddy_and_drops_one_shots() {
        assert!(keep_cursor_agent(&argv(&["codebuddy"])));
        assert!(keep_cursor_agent(&argv(&[
            "/Users/ww/.local/bin/codebuddy"
        ])));
        assert!(keep_cursor_agent(&argv(&["codebuddy", "-c"])));
        assert!(keep_cursor_agent(&argv(&["codebuddy", "-r"])));
        assert!(!keep_cursor_agent(&argv(&["codebuddy", "--version"])));
        assert!(!keep_cursor_agent(&argv(&["codebuddy", "update"])));
        assert!(!keep_cursor_agent(&argv(&["node", "server.js"])));
        assert!(keep_cursor_agent(&argv(&["claude"])));
        assert!(!keep_cursor_agent(&argv(&["claude", "-p"])));
        assert!(keep_cursor_agent(&argv(&["opencode"])));
        assert!(!keep_cursor_agent(&argv(&["opencode", "run"])));
    }

    #[test]
    fn labels_cli_families() {
        use super::tool_label;
        assert_eq!(tool_label("agent"), "CA");
        assert_eq!(tool_label("cursor-agent"), "CA");
        assert_eq!(tool_label("codebuddy"), "CB");
        assert_eq!(tool_label("cbc"), "CB");
        assert_eq!(tool_label("claude"), "CC");
        assert_eq!(tool_label("opencode"), "OC");
    }

    #[test]
    fn project_is_the_workspace_basename() {
        use crate::{Agent, AgentId, Status};

        let agent = Agent {
            id: AgentId {
                session: "ww".into(),
                pane_id: 3,
            },
            tool: "agent".into(),
            status: Status::Found,
            workspace: Some("/tmp/api/".into()),
            tab_name: String::new(),
            tab_position: None,
            pane_title: String::new(),
            detail: String::new(),
            status_since: 0,
            started_at: None,
            finished_at: None,
            visited: false,
        };
        assert_eq!(agent.project(), "api");
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
