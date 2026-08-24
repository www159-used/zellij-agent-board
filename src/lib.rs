//! Agent board core. No Zellij types — the WASM adapter maps host events in.

mod agent;
mod ansi;
mod discover;
mod float_size;
mod floating_state;
mod render;
mod status;
mod theme;

pub use agent::{keep_cursor_agent, workspace_from_argv, Agent, AgentId, PanePlace};
pub use discover::{parse_host_line, parse_scan_line, Found, HookNotice, HostLine};
pub use float_size::{float_size_from_config, FloatSize};
pub use floating_state::FloatingLayerState;
pub use render::{paint, Frame, PaintCtx};
pub use status::Status;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Confirm,
    Dismiss,
    ToggleHelp,
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
    pub now: u64,
    pub help_visible: bool,
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
            if agent.status != status {
                agent.status_since = self.now;
            }
            agent.status = status;
            match status {
                Status::Idle | Status::Ended => agent.detail.clear(),
                _ if !detail.is_empty() => agent.detail = detail.to_string(),
                _ => {}
            }
        }
    }

    pub fn tick(&mut self) {
        self.now = self.now.saturating_add(1);
    }

    pub fn paint(&self, ctx: render::PaintCtx<'_>) -> render::Frame {
        render::paint(self, ctx)
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
            Key::ToggleHelp => {
                self.help_visible = !self.help_visible;
                Action::None
            }
            Key::Dismiss if self.help_visible => {
                self.help_visible = false;
                Action::None
            }
            Key::Dismiss => Action::Dismiss,
            Key::Up => {
                if !self.help_visible && self.selected > 0 {
                    self.selected -= 1;
                }
                Action::None
            }
            Key::Down => {
                if !self.help_visible && !self.agents.is_empty() {
                    self.selected = (self.selected + 1).min(self.agents.len() - 1);
                }
                Action::None
            }
            Key::Confirm if self.help_visible => Action::None,
            Key::Confirm => self.jump_at(self.selected),
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines_for(usize::MAX)
    }

    pub fn lines_for(&self, rows: usize) -> Vec<String> {
        self.paint(render::PaintCtx {
            rows,
            cols: 120,
            home: "",
        })
        .texts()
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
                status_since: prior.map(|agent| agent.status_since).unwrap_or(self.now),
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
        let joined = board.lines().join("\n");
        assert!(joined.contains("working"));
        assert!(joined.contains("Shell cargo test --lib"));
        assert!(joined.contains('└'));
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
        assert!(lines[0].contains("agent-board"));
        assert!(lines[0].contains("found"));
        assert!(lines.iter().any(|line| line.contains("hooks 未装")));
        let joined = lines.join("\n");
        assert!(joined.contains("found"));
        assert!(joined.contains("no report yet"));
        assert!(joined.contains("tab:notes"));
        assert!(joined.contains("pane:3"));
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
        assert!(lines
            .first()
            .is_some_and(|line| line.contains("agent-board")));
        assert!(lines
            .iter()
            .any(|line| line.contains('›') && line.contains('8')));
        assert!(lines
            .last()
            .is_some_and(|line| line.contains("e go") && line.contains("? help")));
        assert!(!lines
            .iter()
            .any(|line| line.contains('1') && line.contains('›')));
    }

    #[test]
    fn question_mark_toggles_help_and_esc_closes_it_first() {
        let mut board = Board::default();
        ingest_two(&mut board);
        assert_eq!(board.decide(Key::ToggleHelp), Action::None);
        assert!(board.help_visible);
        assert_eq!(board.decide(Key::Confirm), Action::None);
        assert_eq!(board.decide(Key::Dismiss), Action::None);
        assert!(!board.help_visible);
        assert_eq!(board.decide(Key::Dismiss), Action::Dismiss);
    }
}
