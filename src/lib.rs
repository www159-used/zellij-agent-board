//! Agent board core. No Zellij types — the WASM adapter maps host events in.

mod agent;
mod discover;
mod floating_state;
mod status;

pub use agent::{keep_cursor_agent, workspace_from_argv, Agent, AgentId, PanePlace};
pub use discover::{parse_host_line, parse_scan_line, Found, HookNotice, HostLine};
pub use floating_state::FloatingLayerState;
pub use status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Confirm,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Dismiss,
    Jump { session: String, pane_id: u32 },
}

#[derive(Debug, Default)]
pub struct Board {
    pub agents: Vec<Agent>,
    pub selected: usize,
    pub hooks_installed: bool,
}

impl Board {
    pub fn ingest(&mut self, text: &str) {
        let (found, hooks) = self.collect_host_lines(text);
        self.replace_from_scan(found);
        self.apply_notices(hooks);
    }

    /// Pipe / spool notice. Updates status on existing rows only; never creates or drops rows.
    pub fn ingest_notice(&mut self, text: &str) {
        let (_, hooks) = self.collect_host_lines(text);
        self.apply_notices(hooks);
    }

    fn collect_host_lines(&mut self, text: &str) -> (Vec<Found>, Vec<HookNotice>) {
        let mut found = Vec::new();
        let mut hooks = Vec::new();
        for line in text.lines() {
            match parse_host_line(line) {
                Some(HostLine::Scan(row)) => found.push(row),
                Some(HostLine::Hook(notice)) => hooks.push(notice),
                Some(HostLine::Meta { hooks_installed }) => {
                    self.hooks_installed = hooks_installed;
                }
                None => {}
            }
        }
        (found, hooks)
    }

    fn apply_notices(&mut self, hooks: Vec<HookNotice>) {
        for notice in hooks {
            self.apply_hook_event(&notice.id, &notice.event, &notice.detail);
        }
    }

    pub fn apply_hook_event(&mut self, id: &AgentId, event: &str, detail: &str) {
        if let Some(status) = Status::from_cursor_hook(event) {
            self.apply_hook(id, status, detail);
        }
    }

    pub fn apply_hook(&mut self, id: &AgentId, status: Status, detail: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == *id) {
            agent.status = status;
            match status {
                Status::Idle | Status::Ended => agent.detail.clear(),
                _ if !detail.is_empty() => agent.detail = detail.to_string(),
                _ => {}
            }
        }
    }

    pub fn apply_places<I>(&mut self, places: I)
    where
        I: IntoIterator<Item = (AgentId, PanePlace)>,
    {
        let selected_id = self.agents.get(self.selected).map(|agent| agent.id.clone());
        for (id, place) in places {
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                agent.tab_name = place.tab_name;
                agent.tab_position = Some(place.tab_position);
                agent.pane_title = place.pane_title;
            }
        }
        self.sort_agents();
        self.restore_selection(selected_id);
    }

    pub fn decide(&mut self, key: Key) -> Action {
        match key {
            Key::Dismiss => Action::Dismiss,
            Key::Up => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                Action::None
            }
            Key::Down => {
                if !self.agents.is_empty() {
                    self.selected = (self.selected + 1).min(self.agents.len() - 1);
                }
                Action::None
            }
            Key::Confirm => self.jump_at(self.selected),
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines_for(usize::MAX)
    }

    pub fn lines_for(&self, rows: usize) -> Vec<String> {
        let mut header = vec!["agent-board · cursor".to_string()];
        if !self.hooks_installed {
            header.push("hooks 未装".to_string());
        }
        let body = if self.agents.is_empty() {
            vec!["no cursor agents".to_string()]
        } else {
            self.agents
                .iter()
                .enumerate()
                .map(|(index, agent)| {
                    let mark = if index == self.selected { ">" } else { " " };
                    if agent.detail.is_empty() {
                        format!(
                            "{mark}{n}  {path}  {status}",
                            n = index + 1,
                            path = agent.place_path(),
                            status = agent.status.label(),
                        )
                    } else {
                        format!(
                            "{mark}{n}  {path}  {status}  {detail}",
                            n = index + 1,
                            path = agent.place_path(),
                            status = agent.status.label(),
                            detail = agent.detail,
                        )
                    }
                })
                .collect()
        };
        let footer = "j/k move   Enter jump   q close".to_string();
        if rows == usize::MAX || header.len() + body.len() < rows {
            header.extend(body);
            header.push(footer);
            return header;
        }
        let body_rows = rows.saturating_sub(header.len() + 1).max(1);
        let selected = if self.agents.is_empty() {
            0
        } else {
            self.selected.min(body.len().saturating_sub(1))
        };
        let mut start = selected.saturating_sub(body_rows / 2);
        if start + body_rows > body.len() {
            start = body.len().saturating_sub(body_rows);
        }
        header.extend(body.into_iter().skip(start).take(body_rows));
        header.push(footer);
        header
    }

    fn replace_from_scan(&mut self, found: Vec<Found>) {
        let selected_id = self.agents.get(self.selected).map(|agent| agent.id.clone());
        let previous: Vec<Agent> = std::mem::take(&mut self.agents);
        let mut next = Vec::new();
        for row in found {
            let argv = if row.argv.is_empty() {
                vec![row.tool.clone()]
            } else {
                row.argv
            };
            if !keep_cursor_agent(&argv) {
                continue;
            }
            let prior = previous.iter().find(|agent| agent.id == row.id);
            next.push(Agent {
                id: row.id,
                tool: row.tool,
                status: prior.map(|agent| agent.status).unwrap_or(Status::Found),
                workspace: workspace_from_argv(&argv),
                tab_name: prior
                    .map(|agent| agent.tab_name.clone())
                    .unwrap_or_default(),
                tab_position: prior.and_then(|agent| agent.tab_position),
                pane_title: prior
                    .map(|agent| agent.pane_title.clone())
                    .unwrap_or_default(),
                detail: prior.map(|agent| agent.detail.clone()).unwrap_or_default(),
            });
        }
        self.agents = next;
        self.sort_agents();
        self.restore_selection(selected_id);
    }

    fn sort_agents(&mut self) {
        self.agents.sort_by(|a, b| {
            a.id.session
                .cmp(&b.id.session)
                .then(
                    a.tab_position
                        .unwrap_or(usize::MAX)
                        .cmp(&b.tab_position.unwrap_or(usize::MAX)),
                )
                .then(a.id.pane_id.cmp(&b.id.pane_id))
        });
    }

    fn restore_selection(&mut self, selected_id: Option<AgentId>) {
        self.selected = selected_id
            .and_then(|id| self.agents.iter().position(|agent| agent.id == id))
            .unwrap_or(0)
            .min(self.agents.len().saturating_sub(1));
    }

    fn jump_at(&self, index: usize) -> Action {
        self.agents
            .get(index)
            .map(|agent| Action::Jump {
                session: agent.id.session.clone(),
                pane_id: agent.id.pane_id,
            })
            .unwrap_or(Action::None)
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Agent, AgentId, Board, Key, PanePlace, Status};

    fn ingest_two(board: &mut Board) {
        board.ingest(
            "\
META hooks=1
SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww
SCAN lp 8 agent /Users/ww/.local/bin/agent --workspace /tmp/lp
",
        );
    }

    #[test]
    fn dismiss_closes_the_board() {
        let mut board = Board::default();
        assert_eq!(board.decide(Key::Dismiss), Action::Dismiss);
    }

    #[test]
    fn scan_creates_rows_and_hooks_cannot() {
        let mut board = Board::default();
        board.ingest_notice("HOOK ww 3 beforeSubmitPrompt\n");
        assert!(board.agents.is_empty());

        ingest_two(&mut board);
        assert!(board.hooks_installed);
        assert_eq!(board.agents.len(), 2);
        assert_eq!(board.agents[0].id.session, "lp");
        assert_eq!(board.agents[0].status, Status::Found);
        assert_eq!(board.agents[1].workspace.as_deref(), Some("/tmp/ww"));

        board.ingest_notice("HOOK ww 3 beforeSubmitPrompt\nHOOK missing 9 stop\n");
        assert_eq!(board.agents[1].status, Status::Working);
        board.ingest_notice("HOOK ww 3 preToolUse Shell cargo test --lib\n");
        assert_eq!(board.agents[1].detail, "Shell cargo test --lib");
        assert!(board
            .lines()
            .iter()
            .any(|line| line.contains("working") && line.contains("Shell cargo test --lib")));
        assert_eq!(board.agents.len(), 2);
    }

    #[test]
    fn drops_subcommands_and_gone_processes() {
        let mut board = Board::default();
        board.ingest("SCAN ww 3 agent /Users/ww/.local/bin/agent ls\n");
        assert!(board.agents.is_empty());

        ingest_two(&mut board);
        board.ingest("SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n");
        assert_eq!(board.agents.len(), 1);
        assert_eq!(board.agents[0].id.session, "ww");
    }

    #[test]
    fn moves_and_jumps() {
        let mut board = Board::default();
        ingest_two(&mut board);
        assert_eq!(
            board.decide(Key::Confirm),
            Action::Jump {
                session: "lp".into(),
                pane_id: 8
            }
        );
        board.decide(Key::Down);
        assert_eq!(
            board.decide(Key::Confirm),
            Action::Jump {
                session: "ww".into(),
                pane_id: 3
            }
        );
    }

    #[test]
    fn keeps_status_and_selection_across_rescan() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.apply_hook(
            &AgentId {
                session: "ww".into(),
                pane_id: 3,
            },
            Status::Working,
            "Shell cargo test",
        );
        board.decide(Key::Down);
        board.ingest(
            "SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             SCAN lp 8 agent /Users/ww/.local/bin/agent --workspace /tmp/lp\n",
        );
        assert_eq!(board.selected, 1);
        assert_eq!(board.agents[1].status, Status::Working);
        assert_eq!(board.agents[1].detail, "Shell cargo test");
    }

    #[test]
    fn lists_found_rows_and_missing_hooks() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=0\nSCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/w\n",
        );
        board.apply_places([(
            AgentId {
                session: "ww".into(),
                pane_id: 3,
            },
            PanePlace {
                tab_position: 1,
                tab_name: "notes".into(),
                pane_title: "agent".into(),
            },
        )]);
        let lines = board.lines();
        assert!(lines[0].contains("cursor"));
        assert_eq!(lines[1], "hooks 未装");
        assert!(lines
            .iter()
            .any(|line| line.contains("ww > notes > agent") && line.contains("found")));
    }

    #[test]
    fn sorts_by_session_then_tab_then_pane() {
        let mut board = Board::default();
        board.ingest(
            "\
SCAN ww 9 agent /Users/ww/.local/bin/agent --workspace /tmp/ww
SCAN ww 2 agent /Users/ww/.local/bin/agent --workspace /tmp/ww
SCAN lp 4 agent /Users/ww/.local/bin/agent --workspace /tmp/lp
",
        );
        board.apply_places([
            (
                AgentId {
                    session: "ww".into(),
                    pane_id: 9,
                },
                PanePlace {
                    tab_position: 0,
                    tab_name: "code".into(),
                    pane_title: "right".into(),
                },
            ),
            (
                AgentId {
                    session: "ww".into(),
                    pane_id: 2,
                },
                PanePlace {
                    tab_position: 1,
                    tab_name: "logs".into(),
                    pane_title: "agent".into(),
                },
            ),
            (
                AgentId {
                    session: "lp".into(),
                    pane_id: 4,
                },
                PanePlace {
                    tab_position: 0,
                    tab_name: "main".into(),
                    pane_title: "agent".into(),
                },
            ),
        ]);
        let paths: Vec<_> = board.agents.iter().map(Agent::place_path).collect();
        assert_eq!(
            paths,
            vec![
                "lp > main > agent",
                "ww > code > right",
                "ww > logs > agent",
            ]
        );
    }

    #[test]
    fn windows_rows_around_the_selection() {
        let mut board = Board::default();
        let mut scan = String::from("META hooks=1\n");
        for pane in 1..=8 {
            scan.push_str(&format!(
                "SCAN ww {pane} agent /Users/ww/.local/bin/agent --workspace /tmp/{pane}\n"
            ));
        }
        board.ingest(&scan);
        board.selected = 7;
        let lines = board.lines_for(4);
        assert_eq!(
            lines.first().map(String::as_str),
            Some("agent-board · cursor")
        );
        assert!(lines.iter().any(|line| line.starts_with(">8")));
        assert_eq!(
            lines.last().map(String::as_str),
            Some("j/k move   Enter jump   q close")
        );
        assert!(!lines.iter().any(|line| line.contains("#1")));
    }
}
