//! Agent board core. No Zellij types — WASM and host TUI map events in.

mod agent;
mod ansi;
mod discover;
mod float_size;
mod floating_state;
mod protocol;
mod render;
#[cfg(not(target_arch = "wasm32"))]
mod scan;
mod status;
mod theme;
mod toggle;

pub use agent::{keep_cursor_agent, workspace_from_argv, Agent, AgentId, PanePlace};
pub use discover::{parse_host_line, parse_scan_line, Found, HookNotice, HostLine};
pub use float_size::{float_size_from_config, FloatSize};
pub use floating_state::FloatingLayerState;
#[cfg(not(target_arch = "wasm32"))]
pub use protocol::persist_seen;
pub use protocol::{
    focus_path, format_focus, format_jump, format_places, format_seen, format_started,
    merge_places, parse_focus, parse_jump, parse_places, places_path, seen_dir, spool_dir,
    started_dir, PIPE_NAME,
};
#[cfg(not(target_arch = "wasm32"))]
pub use protocol::{host_places_path, load_places, persist_places};
pub use render::{frame_patch, paint, paint_to_size, render_board, Frame, FramePatch, PaintCtx};
#[cfg(not(target_arch = "wasm32"))]
pub use scan::{
    places_from_list_panes_json, scan_host_text, scan_places, scan_places_for,
    zellij_ids_from_env_blob,
};
pub use status::Status;
pub use toggle::{
    bridge_close_plan, closes_the_board, duplicate_close_ids, duplicate_close_ids_with_focus,
    now_ms, BridgeClosePlan, TOGGLE_DEBOUNCE_MS,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    First,
    Last,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    Confirm,
    Dismiss,
    ToggleHelp,
    StartHint,
    StartSearch,
    NextMatch,
    PrevMatch,
    Backspace,
    Input(char),
    Digit(u8),
    GPrefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Dismiss,
    Jump { session: String, pane_id: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HintState {
    labels: Vec<Option<String>>,
    query: String,
    jump_prefix: String,
    /// Rows visible when flash started; labels and query stay inside it.
    scope: (usize, usize),
}

/// Vim / Helix `/` — incremental search in the list, not a picker.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchState {
    query: String,
    origin: usize,
}

#[derive(Debug, Default)]
pub struct Board {
    pub agents: Vec<Agent>,
    pub selected: usize,
    pub hooks_installed: bool,
    pub now: u64,
    /// Host unix epoch; advanced by Timer so working elapsed keeps moving.
    pub wall_now: u64,
    /// Frozen host epoch from open; done "ago" uses this and does not tick.
    pub wall_at_open: u64,
    pub help_visible: bool,
    hint: Option<HintState>,
    search: Option<SearchState>,
    last_search: String,
    motion_count: Option<u32>,
    g_pending: bool,
    page_len: usize,
    list_height: u16,
    wide_list: bool,
}

impl Board {
    pub fn ingest(&mut self, text: &str) -> bool {
        let before = self.agents.clone();
        let before_hooks = self.hooks_installed;
        let (found, hooks, seen, started) = self.collect_host_lines(text);
        self.replace_from_scan(found);
        self.apply_notices(hooks);
        self.apply_seen(seen);
        self.apply_started(started);
        self.hooks_installed != before_hooks || self.agents != before
    }

    /// Pipe / spool notice. Updates status on existing rows only; never creates or drops rows.
    pub fn ingest_notice(&mut self, text: &str) {
        let (_, hooks, seen, started) = self.collect_host_lines(text);
        self.apply_notices(hooks);
        self.apply_seen(seen);
        self.apply_started(started);
    }

    fn collect_host_lines(
        &mut self,
        text: &str,
    ) -> (
        Vec<Found>,
        Vec<HookNotice>,
        Vec<(AgentId, u64)>,
        Vec<(AgentId, u64)>,
    ) {
        let mut found = Vec::new();
        let mut hooks = Vec::new();
        let mut seen = Vec::new();
        let mut started = Vec::new();
        for line in text.lines() {
            match parse_host_line(line) {
                Some(HostLine::Scan(row)) => found.push(row),
                Some(HostLine::Hook(notice)) => hooks.push(notice),
                Some(HostLine::Seen { id, finished_at }) => seen.push((id, finished_at)),
                Some(HostLine::Started { id, started_at }) => started.push((id, started_at)),
                Some(HostLine::Meta {
                    hooks_installed,
                    epoch,
                }) => {
                    self.hooks_installed = hooks_installed;
                    if let Some(epoch) = epoch {
                        // First META anchors both clocks; later rescans leave them alone
                        // so working only moves via tick, and ago stays frozen at open.
                        if self.wall_now == 0 {
                            self.wall_now = epoch;
                        }
                        if self.wall_at_open == 0 {
                            self.wall_at_open = epoch;
                        }
                    }
                }
                None => {}
            }
        }
        (found, hooks, seen, started)
    }

    fn apply_notices(&mut self, hooks: Vec<HookNotice>) {
        for notice in hooks {
            self.apply_hook_event(
                &notice.id,
                &notice.event,
                &notice.detail,
                notice.at_epoch,
                notice.at_stamp.as_deref(),
            );
        }
    }

    pub fn apply_hook_event(
        &mut self,
        id: &AgentId,
        event: &str,
        detail: &str,
        at_epoch: Option<u64>,
        at_stamp: Option<&str>,
    ) {
        if let Some(status) = Status::from_cursor_hook(event) {
            self.apply_hook(id, status, detail, at_epoch, at_stamp);
            if event == "beforeSubmitPrompt" {
                if let Some(agent) = self.agents.iter().find(|agent| agent.id == *id) {
                    if let Some(started_at) = agent.started_at {
                        persist_turn_start(id, started_at);
                    }
                }
            }
        }
    }

    pub fn apply_hook(
        &mut self,
        id: &AgentId,
        status: Status,
        detail: &str,
        at_epoch: Option<u64>,
        _at_stamp: Option<&str>,
    ) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == *id) {
            let changing = agent.status != status;
            if changing {
                agent.status_since = self.now;
            }
            agent.status = status;
            match status {
                Status::Working | Status::Compact => {
                    agent.finished_at = None;
                    if changing {
                        agent.visited = false;
                    }
                    if changing || agent.started_at.is_none() {
                        let candidate =
                            at_epoch.or_else(|| (self.wall_now > 0).then_some(self.wall_now));
                        agent.started_at = match (agent.started_at, candidate) {
                            (Some(old), Some(new)) => Some(old.min(new)),
                            (Some(old), None) => Some(old),
                            (None, next) => next,
                        };
                    }
                    if !detail.is_empty() {
                        agent.detail = detail.to_string();
                    }
                }
                Status::Done => {
                    clear_turn_start(&agent.id);
                    agent.started_at = None;
                    if changing {
                        agent.visited = false;
                    }
                    agent.finished_at =
                        at_epoch.or_else(|| (self.wall_now > 0).then_some(self.wall_now));
                    if !detail.is_empty() {
                        agent.detail = detail.to_string();
                    }
                }
                Status::Idle | Status::Ended => {
                    clear_turn_start(&agent.id);
                    agent.detail.clear();
                    agent.finished_at = None;
                    agent.started_at = None;
                }
                _ => {
                    agent.finished_at = None;
                    agent.started_at = None;
                    if !detail.is_empty() {
                        agent.detail = detail.to_string();
                    }
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.now = self.now.saturating_add(1);
        if self.wall_now > 0 {
            self.wall_now = self.wall_now.saturating_add(1);
        }
    }

    pub fn needs_clock(&self) -> bool {
        self.agents
            .iter()
            .any(|agent| matches!(agent.status, Status::Working | Status::Compact))
    }

    pub fn paint(&self, ctx: render::PaintCtx<'_>) -> render::Frame {
        render::paint(self, ctx)
    }

    pub fn apply_places<I>(&mut self, places: I) -> bool
    where
        I: IntoIterator<Item = (AgentId, PanePlace)>,
    {
        let mut changed = false;
        let mut order_changed = false;
        let mut hint_fields_changed = false;
        for (id, place) in places {
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                let place = place.keep_names(&agent.tab_name, &agent.pane_title);
                if agent.tab_name != place.tab_name {
                    agent.tab_name = place.tab_name;
                    changed = true;
                    hint_fields_changed = true;
                }
                if agent.tab_position != Some(place.tab_position) {
                    agent.tab_position = Some(place.tab_position);
                    changed = true;
                    order_changed = true;
                }
                if agent.pane_title != place.pane_title {
                    agent.pane_title = place.pane_title;
                    changed = true;
                }
            }
        }
        if !changed {
            return false;
        }
        if order_changed {
            let selected_id = self.agents.get(self.selected).map(|agent| agent.id.clone());
            self.sort_agents();
            self.restore_selection(selected_id);
        } else if hint_fields_changed && self.is_hinting() {
            self.recompute_hint_labels();
        }
        true
    }

    pub fn sessions_missing_titles(&self) -> Vec<String> {
        let mut sessions: Vec<String> = self
            .agents
            .iter()
            .filter(|agent| agent.tab_name.is_empty() && agent.pane_title.is_empty())
            .map(|agent| agent.id.session.clone())
            .filter(|name| !name.is_empty())
            .collect();
        sessions.sort();
        sessions.dedup();
        sessions
    }

    pub fn decide(&mut self, key: Key) -> Action {
        if self.help_visible {
            return match key {
                Key::ToggleHelp | Key::Dismiss => {
                    self.help_visible = false;
                    Action::None
                }
                _ => Action::None,
            };
        }
        if self.is_searching() {
            return match key {
                Key::Dismiss => {
                    if let Some(search) = self.search.take() {
                        self.selected = search.origin.min(self.agents.len().saturating_sub(1));
                    }
                    Action::None
                }
                Key::Confirm => {
                    if let Some(search) = self.search.take() {
                        self.last_search = search.query;
                    }
                    Action::None
                }
                Key::Backspace => {
                    if let Some(search) = self.search.as_mut() {
                        search.query.pop();
                    }
                    self.reveal_search_match();
                    Action::None
                }
                Key::Input(ch) => self.apply_search_input(ch),
                Key::NextMatch => {
                    self.goto_match(1);
                    Action::None
                }
                Key::PrevMatch => {
                    self.goto_match(-1);
                    Action::None
                }
                _ => Action::None,
            };
        }
        if self.is_hinting() {
            return match key {
                Key::Dismiss => {
                    self.hint = None;
                    Action::None
                }
                Key::StartHint => Action::None,
                Key::Backspace => {
                    if let Some(hint) = self.hint.as_mut() {
                        if hint.jump_prefix.is_empty() {
                            hint.query.pop();
                        } else {
                            hint.jump_prefix.pop();
                        }
                    }
                    self.recompute_hint_labels();
                    self.reveal_first_hint_match();
                    Action::None
                }
                Key::Input(ch) => self.apply_hint_input(ch),
                Key::Confirm => self.jump_at(self.selected),
                _ => Action::None,
            };
        }
        match key {
            Key::ToggleHelp => {
                self.clear_motion();
                self.help_visible = true;
                Action::None
            }
            Key::Dismiss => Action::Dismiss,
            Key::Digit(digit) => {
                self.g_pending = false;
                self.push_count_digit(digit);
                Action::None
            }
            Key::GPrefix => {
                if self.g_pending {
                    self.g_pending = false;
                    self.goto_counted_or_first();
                } else {
                    self.g_pending = true;
                }
                Action::None
            }
            Key::First => {
                self.g_pending = false;
                self.goto_counted_or_first();
                Action::None
            }
            Key::Last => {
                self.g_pending = false;
                self.goto_counted_or_last();
                Action::None
            }
            Key::Up => {
                self.g_pending = false;
                let steps = self.take_count();
                self.move_by(-(steps as isize));
                Action::None
            }
            Key::Down => {
                self.g_pending = false;
                let steps = self.take_count();
                self.move_by(steps as isize);
                Action::None
            }
            Key::HalfPageDown => {
                self.g_pending = false;
                let steps = self.half_page() * self.take_count();
                self.move_by(steps as isize);
                Action::None
            }
            Key::HalfPageUp => {
                self.g_pending = false;
                let steps = self.half_page() * self.take_count();
                self.move_by(-(steps as isize));
                Action::None
            }
            Key::PageDown => {
                self.g_pending = false;
                let steps = self.page_span() * self.take_count();
                self.move_by(steps as isize);
                Action::None
            }
            Key::PageUp => {
                self.g_pending = false;
                let steps = self.page_span() * self.take_count();
                self.move_by(-(steps as isize));
                Action::None
            }
            Key::Confirm => {
                self.clear_motion();
                self.jump_at(self.selected)
            }
            Key::StartHint => {
                self.clear_motion();
                self.hint = Some(HintState {
                    labels: vec![None; self.agents.len()],
                    query: String::new(),
                    jump_prefix: String::new(),
                    scope: self.visible_range(),
                });
                Action::None
            }
            Key::StartSearch => {
                self.clear_motion();
                self.search = Some(SearchState {
                    query: String::new(),
                    origin: self.selected,
                });
                Action::None
            }
            Key::NextMatch => {
                self.goto_match(1);
                Action::None
            }
            Key::PrevMatch => {
                self.goto_match(-1);
                Action::None
            }
            Key::Backspace | Key::Input(_) => Action::None,
        }
    }

    pub fn set_page_len(&mut self, len: usize) {
        self.page_len = len;
    }

    pub fn set_list_geometry(&mut self, width: u16, height: u16) {
        self.wide_list = width >= 50;
        let content = if height >= 3 {
            height.saturating_sub(1)
        } else {
            height
        };
        let chrome = 1 + u16::from(!self.hooks_installed);
        self.list_height = content.saturating_sub(chrome);
    }

    pub fn visible_range(&self) -> (usize, usize) {
        if self.list_height == 0 {
            return (0, self.agents.len());
        }
        self.list_viewport(self.list_height, self.wide_list)
    }

    pub fn list_viewport(&self, body_height: u16, wide: bool) -> (usize, usize) {
        let len = self.agents.len();
        if len == 0 || body_height == 0 {
            return (0, 0);
        }
        let selected = self.selected.min(len - 1);
        let multi = self
            .agents
            .windows(2)
            .any(|pair| pair[0].id.session != pair[1].id.session);
        let mut start = selected;
        while start > 0
            && self.measure_agent_block(start - 1, selected + 1, wide, multi) <= body_height
        {
            start -= 1;
        }
        let mut end = selected + 1;
        while end < len && self.measure_agent_block(start, end + 1, wide, multi) <= body_height {
            end += 1;
        }
        (start, end)
    }

    fn measure_agent_block(&self, start: usize, end: usize, wide: bool, multi: bool) -> u16 {
        let mut lines = 0u16;
        // Same as paint: a viewport that starts mid-session still draws a header.
        let mut last_session = None;
        for index in start..end {
            let session = self.agents[index].id.session.as_str();
            if multi && last_session != Some(session) {
                if last_session.is_some() {
                    lines = lines.saturating_add(1);
                }
                lines = lines.saturating_add(1);
            }
            last_session = Some(session);
            lines = lines.saturating_add(1);
            if wide && self.row_has_activity(index) {
                lines = lines.saturating_add(1);
            }
        }
        lines
    }

    fn row_has_activity(&self, index: usize) -> bool {
        self.agents
            .get(index)
            .is_some_and(|agent| !agent.detail.is_empty() || agent.status == Status::Found)
    }

    fn clear_motion(&mut self) {
        self.motion_count = None;
        self.g_pending = false;
    }

    fn push_count_digit(&mut self, digit: u8) {
        if digit == 0 && self.motion_count.is_none() {
            self.selected = 0;
            return;
        }
        let next = self
            .motion_count
            .unwrap_or(0)
            .saturating_mul(10)
            .saturating_add(u32::from(digit));
        self.motion_count = Some(next.min(10_000));
    }

    fn take_count(&mut self) -> usize {
        self.motion_count.take().unwrap_or(1).max(1) as usize
    }

    fn page_span(&self) -> usize {
        if self.page_len == 0 {
            10
        } else {
            self.page_len
        }
    }

    fn half_page(&self) -> usize {
        (self.page_span() / 2).max(1)
    }

    fn move_by(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let last = self.agents.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    fn goto_counted_or_first(&mut self) {
        if let Some(count) = self.motion_count.take() {
            self.goto_line(count);
        } else if !self.agents.is_empty() {
            self.selected = 0;
        }
    }

    fn goto_counted_or_last(&mut self) {
        if let Some(count) = self.motion_count.take() {
            self.goto_line(count);
        } else if !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
    }

    fn goto_line(&mut self, line: u32) {
        if self.agents.is_empty() {
            return;
        }
        let index = (line.max(1) as usize).saturating_sub(1);
        self.selected = index.min(self.agents.len() - 1);
    }

    pub fn is_hinting(&self) -> bool {
        self.hint.is_some()
    }

    pub fn is_searching(&self) -> bool {
        self.search.is_some()
    }

    pub fn search_query(&self) -> &str {
        self.search
            .as_ref()
            .map(|search| search.query.as_str())
            .unwrap_or("")
    }

    pub fn highlight_query(&self) -> &str {
        if self.is_searching() {
            self.search_query()
        } else {
            self.hint_query()
        }
    }

    pub fn hint_query(&self) -> &str {
        self.hint
            .as_ref()
            .map(|hint| hint.query.as_str())
            .unwrap_or("")
    }

    pub fn hint_jump_prefix(&self) -> &str {
        self.hint
            .as_ref()
            .map(|hint| hint.jump_prefix.as_str())
            .unwrap_or("")
    }

    pub fn hint_label(&self, index: usize) -> Option<&str> {
        self.hint
            .as_ref()
            .and_then(|hint| hint.labels.get(index))
            .and_then(Option::as_deref)
    }

    pub fn agent_matches(&self, index: usize) -> bool {
        let query = self.highlight_query();
        if query.is_empty() {
            return true;
        }
        self.agents
            .get(index)
            .is_some_and(|agent| agent_matches(agent, query))
    }

    /// Which searchable field hit, for paint highlight.
    /// Prefer session → project → tab.
    pub fn hint_match_field(&self, index: usize) -> Option<HintField> {
        let query = self.highlight_query();
        if query.is_empty() {
            return None;
        }
        let agent = self.agents.get(index)?;
        match_targets(agent)
            .into_iter()
            .find_map(|(field, text)| text_match_range(text, query).map(|_| field))
    }

    pub fn hint_match_range(&self, index: usize) -> Option<(usize, usize)> {
        let query = self.highlight_query();
        if query.is_empty() {
            return None;
        }
        let agent = self.agents.get(index)?;
        match_targets(agent)
            .into_iter()
            .find_map(|(_, text)| text_match_range(text, query))
    }

    fn apply_hint_input(&mut self, ch: char) -> Action {
        let ch = ch.to_ascii_lowercase();
        let Some(hint) = self.hint.as_ref() else {
            return Action::None;
        };
        if !hint.query.is_empty() {
            let mut jump_prefix = hint.jump_prefix.clone();
            jump_prefix.push(ch);
            let label_matches: Vec<usize> = hint
                .labels
                .iter()
                .enumerate()
                .filter_map(|(index, label)| {
                    label
                        .as_ref()
                        .is_some_and(|label| label.starts_with(&jump_prefix))
                        .then_some(index)
                })
                .collect();
            if label_matches.len() == 1
                && hint.labels[label_matches[0]].as_deref() == Some(jump_prefix.as_str())
            {
                return self.jump_at(label_matches[0]);
            }
            if !label_matches.is_empty() {
                if let Some(hint) = self.hint.as_mut() {
                    hint.jump_prefix = jump_prefix;
                }
                return Action::None;
            }
        }

        let mut query = hint.query.clone();
        query.push(ch);
        let (start, end) = self.hint_scope();
        let has_matches = self.agents[start..end]
            .iter()
            .any(|agent| agent_matches(agent, &query));
        if !has_matches {
            return Action::None;
        }
        if let Some(hint) = self.hint.as_mut() {
            hint.query = query;
            hint.jump_prefix.clear();
        }
        self.recompute_hint_labels();
        let sole_match = self.hint.as_ref().and_then(|hint| {
            let matched: Vec<usize> = hint
                .labels
                .iter()
                .enumerate()
                .filter_map(|(index, label)| label.as_ref().map(|_| index))
                .collect();
            (matched.len() == 1).then_some(matched[0])
        });
        if let Some(index) = sole_match {
            return self.jump_at(index);
        }
        self.reveal_first_hint_match();
        Action::None
    }

    fn apply_search_input(&mut self, ch: char) -> Action {
        let ch = ch.to_ascii_lowercase();
        let Some(search) = self.search.as_ref() else {
            return Action::None;
        };
        let mut query = search.query.clone();
        query.push(ch);
        if !self.agents.iter().any(|agent| agent_matches(agent, &query)) {
            return Action::None;
        }
        if let Some(search) = self.search.as_mut() {
            search.query = query;
        }
        self.reveal_search_match();
        Action::None
    }

    fn recompute_hint_labels(&mut self) {
        let Some(hint) = self.hint.as_ref() else {
            return;
        };
        let query = hint.query.clone();
        let (start, end) = hint.scope;
        let (start, end) = (start.min(self.agents.len()), end.min(self.agents.len()));
        let matches: Vec<usize> = self
            .agents
            .iter()
            .enumerate()
            .filter_map(|(index, agent)| {
                (index >= start && index < end && !query.is_empty() && agent_matches(agent, &query))
                    .then_some(index)
            })
            .collect();
        let mut available: Vec<u8> = HINT_ALPHABET
            .iter()
            .copied()
            .filter(|candidate| {
                let mut extended = query.clone();
                extended.push(char::from(*candidate));
                !self.agents[start..end]
                    .iter()
                    .any(|agent| agent_matches(agent, &extended))
            })
            .collect();
        if available.is_empty() {
            available.extend_from_slice(HINT_ALPHABET);
        }
        let generated = labels_for(matches.len(), &available);
        let item_count = self.agents.len();
        if let Some(hint) = self.hint.as_mut() {
            hint.labels = vec![None; item_count];
            for (index, label) in matches.into_iter().zip(generated) {
                hint.labels[index] = Some(label);
            }
        }
    }

    fn hint_scope(&self) -> (usize, usize) {
        let len = self.agents.len();
        let (start, end) = self
            .hint
            .as_ref()
            .map(|hint| hint.scope)
            .unwrap_or((0, len));
        let start = start.min(len);
        (start, end.min(len).max(start))
    }

    fn reveal_first_hint_match(&mut self) {
        let Some(index) = self
            .hint
            .as_ref()
            .and_then(|hint| hint.labels.iter().position(Option::is_some))
        else {
            return;
        };
        let (start, end) = self.visible_range();
        if index >= start && index < end {
            return;
        }
        self.selected = index;
    }

    fn match_indices(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }
        self.agents
            .iter()
            .enumerate()
            .filter_map(|(index, agent)| agent_matches(agent, query).then_some(index))
            .collect()
    }

    fn active_search_query(&self) -> &str {
        if let Some(search) = &self.search {
            search.query.as_str()
        } else {
            self.last_search.as_str()
        }
    }

    fn reveal_search_match(&mut self) {
        let origin = self
            .search
            .as_ref()
            .map(|search| search.origin)
            .unwrap_or(0);
        let query = self.search_query().to_string();
        let hits = self.match_indices(&query);
        if hits.is_empty() {
            if let Some(search) = &self.search {
                self.selected = search.origin.min(self.agents.len().saturating_sub(1));
            }
            return;
        }
        self.selected = hits
            .iter()
            .copied()
            .find(|&index| index >= origin)
            .unwrap_or(hits[0]);
    }

    fn goto_match(&mut self, dir: isize) {
        let query = self.active_search_query().to_string();
        let hits = self.match_indices(&query);
        if hits.is_empty() {
            return;
        }
        self.selected = if dir >= 0 {
            hits.iter()
                .copied()
                .find(|&index| index > self.selected)
                .unwrap_or(hits[0])
        } else {
            hits.iter()
                .copied()
                .rev()
                .find(|&index| index < self.selected)
                .unwrap_or(*hits.last().unwrap())
        };
    }

    pub fn mark_visited(&mut self, id: &AgentId) -> bool {
        let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == *id) else {
            return false;
        };
        if !agent.unread_done() {
            return false;
        }
        agent.visited = true;
        true
    }

    fn apply_seen(&mut self, seen: Vec<(AgentId, u64)>) {
        for (id, finished_at) in seen {
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                if agent.status == Status::Done && agent.finished_at == Some(finished_at) {
                    agent.visited = true;
                }
            }
        }
    }

    fn apply_started(&mut self, started: Vec<(AgentId, u64)>) {
        for (id, started_at) in started {
            if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
                if !matches!(agent.status, Status::Working | Status::Compact) {
                    continue;
                }
                match agent.started_at {
                    Some(old) if old <= started_at => {}
                    _ => agent.started_at = Some(started_at),
                }
            }
        }
    }

    fn jump_at(&mut self, index: usize) -> Action {
        if self.is_hinting() {
            self.hint = None;
        }
        self.search = None;
        let Some(agent) = self.agents.get_mut(index) else {
            return Action::None;
        };
        if agent.status == Status::Done {
            agent.visited = true;
        }
        Action::Jump {
            session: agent.id.session.clone(),
            pane_id: agent.id.pane_id,
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

    fn replace_from_scan(&mut self, found: Vec<Found>) -> bool {
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
                started_at: prior.and_then(|agent| agent.started_at),
                finished_at: prior.and_then(|agent| agent.finished_at),
                visited: prior.is_some_and(|agent| agent.visited),
            });
        }
        sort_agent_rows(&mut next);
        if next == previous {
            self.agents = previous;
            return false;
        }
        self.agents = next;
        self.restore_selection(selected_id);
        true
    }

    fn sort_agents(&mut self) {
        sort_agent_rows(&mut self.agents);
    }

    fn restore_selection(&mut self, selected_id: Option<AgentId>) {
        self.selected = selected_id
            .and_then(|id| self.agents.iter().position(|agent| agent.id == id))
            .unwrap_or(0)
            .min(self.agents.len().saturating_sub(1));
        if self.is_hinting() {
            self.recompute_hint_labels();
        }
    }
}

fn persist_turn_start(id: &AgentId, started_at: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    crate::protocol::persist_started(&id.session, id.pane_id, started_at);
    let _ = (id, started_at);
}

fn clear_turn_start(id: &AgentId) {
    #[cfg(not(target_arch = "wasm32"))]
    crate::protocol::clear_started(&id.session, id.pane_id);
    let _ = id;
}

fn sort_agent_rows(agents: &mut [Agent]) {
    agents.sort_by(|a, b| {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintField {
    Session,
    Project,
    Tab,
}

const HINT_ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";

fn agent_matches(agent: &Agent, query: &str) -> bool {
    match_targets(agent)
        .into_iter()
        .any(|(_, text)| text_match_range(text, query).is_some())
}

fn match_targets(agent: &Agent) -> Vec<(HintField, &str)> {
    let mut out = vec![(HintField::Session, agent.id.session.as_str())];
    let project = agent.project();
    if project != "-" {
        out.push((HintField::Project, project));
    }
    if !agent.tab_name.is_empty() {
        out.push((HintField::Tab, agent.tab_name.as_str()));
    }
    out
}

fn text_match_range(text: &str, query: &str) -> Option<(usize, usize)> {
    if query.is_empty() || text.is_empty() {
        return None;
    }
    let text: Vec<char> = text.chars().map(|ch| ch.to_ascii_lowercase()).collect();
    let query: Vec<char> = query.chars().map(|ch| ch.to_ascii_lowercase()).collect();
    text.windows(query.len())
        .position(|window| window == query)
        .map(|start| (start, query.len()))
}

fn labels_for(count: usize, alphabet: &[u8]) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let base = alphabet.len();
    let mut width = 1;
    let mut capacity = base;
    while capacity < count {
        width += 1;
        capacity = capacity.saturating_mul(base);
    }
    (0..count)
        .map(|mut index| {
            let mut label = vec![alphabet[0]; width];
            for slot in label.iter_mut().rev() {
                *slot = alphabet[index % base];
                index /= base;
            }
            String::from_utf8(label).expect("hint alphabet is ASCII")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{persist_seen, seen_dir, Action, Agent, AgentId, Board, Key, PanePlace, Status};

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
        board.ingest_notice("HOOK ww 3 stop @1700000000 +08-24T15:21\n");
        assert_eq!(board.agents[1].status, Status::Done);
        assert_eq!(board.agents[1].finished_at, Some(1_700_000_000));
        let joined = board.lines().join("\n");
        assert!(joined.contains("new 0s"), "{joined}");
        assert_eq!(board.agents.len(), 2);
        assert!(board.agents[1].unread_done());
    }

    #[test]
    fn done_without_a_visit_is_unread_until_jump() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.ingest_notice("HOOK ww 3 stop @1700000000\n");
        assert!(board.agents[1].unread_done());
        assert!(board.lines().join("\n").contains('!'));

        board.selected = 1;
        assert_eq!(
            board.decide(Key::Confirm),
            Action::Jump {
                session: "ww".into(),
                pane_id: 3,
            }
        );
        assert!(!board.agents[1].unread_done());
        assert!(!board.lines().join("\n").contains('!'));
    }

    #[test]
    fn seen_line_marks_the_matching_done_cycle() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 stop @1700000000\n\
             SEEN ww 3 1700000000\n",
        );
        assert!(!board.agents[0].unread_done());
    }

    #[test]
    fn stale_seen_does_not_mark_a_new_done_cycle() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 stop @1700000000\n\
             SEEN ww 3 1690000000\n",
        );
        assert!(board.agents[0].unread_done());
    }

    #[test]
    fn a_new_done_cycle_is_unread_again() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.ingest_notice("HOOK ww 3 stop @1700000000\n");
        let id = board.agents[1].id.clone();
        assert!(board.mark_visited(&id));
        assert!(!board.agents[1].unread_done());
        board.ingest_notice("HOOK ww 3 beforeSubmitPrompt @1700000100\n");
        board.ingest_notice("HOOK ww 3 stop @1700000200\n");
        assert!(board.agents[1].unread_done());
    }

    #[test]
    fn persist_seen_survives_a_reopen_ingest() {
        persist_seen("ww", 3, 1_700_000_000);
        let path = seen_dir().join("ww-3");
        let body = std::fs::read_to_string(&path).expect("seen file");
        let mut board = Board::default();
        board.ingest(&format!(
            "META hooks=1\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 stop @1700000000\n\
             {body}"
        ));
        assert!(!board.agents[0].unread_done());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn working_elapsed_uses_host_epoch_across_reopen() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1 epoch=1700000134\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 beforeSubmitPrompt @1700000000 +08-24T15:00\n",
        );
        assert_eq!(board.wall_now, 1_700_000_134);
        assert_eq!(board.agents[0].started_at, Some(1_700_000_000));
        let joined = board.lines().join("\n");
        assert!(joined.contains("2m14s"), "{joined}");

        // Simulate q then Alt+q: fresh board, same spool/hook epoch.
        let mut reopened = Board::default();
        reopened.ingest(
            "META hooks=1 epoch=1700000200\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 beforeSubmitPrompt @1700000000 +08-24T15:00\n",
        );
        assert_eq!(reopened.agents[0].started_at, Some(1_700_000_000));
        let joined = reopened.lines().join("\n");
        assert!(joined.contains("3m20s"), "{joined}");
    }

    #[test]
    fn working_elapsed_keeps_turn_start_after_later_tool_hook() {
        let mut reopened = Board::default();
        reopened.ingest(
            "META hooks=1 epoch=1700000200\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 postToolUse @1700000199 +08-24T15:03 Grep src\n\
             STARTED ww 3 1700000000\n",
        );
        assert_eq!(reopened.agents[0].started_at, Some(1_700_000_000));
        let joined = reopened.lines().join("\n");
        assert!(joined.contains("3m20s"), "{joined}");
        assert!(!joined.contains("1s"), "{joined}");
    }

    #[test]
    fn working_ticks_while_ago_stays_frozen() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1 epoch=1700000134\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             SCAN ww 9 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             HOOK ww 3 beforeSubmitPrompt @1700000000 +08-24T15:00\n\
             HOOK ww 9 stop @1700000000 +08-24T15:00\n",
        );
        assert_eq!(board.wall_now, 1_700_000_134);
        assert_eq!(board.wall_at_open, 1_700_000_134);
        let before = board.lines().join("\n");
        assert!(before.contains("2m14s"), "{before}");
        assert!(before.contains("new 2m14s"), "{before}");

        board.tick();
        board.tick();
        board.ingest(
            "META hooks=1 epoch=1700000999\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n\
             SCAN ww 9 agent /Users/ww/.local/bin/agent --workspace /tmp/ww\n",
        );
        assert_eq!(board.wall_now, 1_700_000_136);
        assert_eq!(board.wall_at_open, 1_700_000_134);
        let after = board.lines().join("\n");
        assert!(after.contains("2m16s"), "{after}");
        assert!(after.contains("new 2m14s"), "{after}");
        assert!(!after.contains("16m"), "{after}");
    }

    #[test]
    fn drops_subcommands_and_gone_processes() {
        let mut board = Board::default();
        board.ingest("SCAN ww 3 agent /Users/ww/.local/bin/agent status\n");
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
            Some(1_700_000_000),
            None,
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
    fn identical_rescan_is_a_no_op_even_while_hinting() {
        let mut board = Board::default();
        ingest_two(&mut board);
        let place = PanePlace {
            tab_position: 0,
            tab_name: "openapi".into(),
            pane_title: String::new(),
        };
        assert!(board.apply_places([(
            AgentId {
                session: "lp".into(),
                pane_id: 8,
            },
            place.clone(),
        )]));
        board.decide(Key::StartHint);
        let labels = board.hint.as_ref().map(|hint| hint.labels.clone());
        assert!(!board.ingest(
            "\
META hooks=1 epoch=1700000120
SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/ww
SCAN lp 8 agent /Users/ww/.local/bin/agent --workspace /tmp/lp
"
        ));
        assert_eq!(board.hint.as_ref().map(|hint| hint.labels.clone()), labels);
        assert!(!board.apply_places([(
            AgentId {
                session: "lp".into(),
                pane_id: 8,
            },
            place,
        )]));
    }

    #[test]
    fn blank_place_update_does_not_wipe_pane_title() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1 epoch=1700000134\n\
             SCAN mysql_syncer 2 agent /Users/ww/.local/bin/agent\n\
             HOOK mysql_syncer 2 stop @1700000000 +08-25T14:11\n",
        );
        assert!(board.apply_places([(
            AgentId {
                session: "mysql_syncer".into(),
                pane_id: 2,
            },
            PanePlace {
                tab_position: 1,
                tab_name: "refactor/use-yotta".into(),
                pane_title: "refactor/use-yotta".into(),
            },
        )]));
        assert!(board.agents[0].pane_title.contains("use-yotta"));
        assert!(!board.apply_places([(
            AgentId {
                session: "mysql_syncer".into(),
                pane_id: 2,
            },
            PanePlace {
                tab_position: 1,
                tab_name: String::new(),
                pane_title: String::new(),
            },
        )]));
        assert_eq!(board.agents[0].tab_name, "refactor/use-yotta");
        assert_eq!(board.agents[0].pane_title, "refactor/use-yotta");
        assert!(board.sessions_missing_titles().is_empty());
        let text = board.lines().join("\n");
        assert!(text.contains("done"), "{text}");
        assert!(text.contains("use-yotta"), "{text}");
        assert!(board.sessions_missing_titles().is_empty());
    }

    #[test]
    fn sessions_missing_titles_skips_named_rows() {
        let mut board = Board::default();
        board.ingest(
            "META hooks=1 epoch=1700000134\n\
             SCAN ww 3 agent /Users/ww/.local/bin/agent\n\
             SCAN lp 0 agent /Users/ww/.local/bin/agent\n",
        );
        assert_eq!(board.sessions_missing_titles(), ["lp", "ww"]);
        board.apply_places([(
            AgentId {
                session: "ww".into(),
                pane_id: 3,
            },
            PanePlace {
                tab_position: 0,
                tab_name: "self".into(),
                pane_title: "review".into(),
            },
        )]);
        assert_eq!(board.sessions_missing_titles(), ["lp"]);
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
        assert!(lines[0].contains("found"));
        assert!(lines.iter().any(|line| line.contains("hooks 未装")));
        let joined = lines.join("\n");
        assert!(joined.contains("found"));
        assert!(joined.contains("no report yet"));
        assert!(!joined.contains("tab:"));
        assert!(!joined.contains("pane:"));
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
        assert!(lines.first().is_some_and(|line| line.contains("found")));
        assert!(lines
            .iter()
            .any(|line| line.contains('›') && line.contains('8')));
        assert!(lines
            .last()
            .is_some_and(|line| line.contains(" go ") && line.contains(" more ")));
        assert!(!lines
            .iter()
            .any(|line| line.contains('›') && line.contains("/1")));
    }

    fn ingest_panes(board: &mut Board, count: u32) {
        let mut scan = String::from("META hooks=1\n");
        for pane in 1..=count {
            scan.push_str(&format!(
                "SCAN ww {pane} agent /Users/ww/.local/bin/agent --workspace /tmp/{pane}\n"
            ));
        }
        board.ingest(&scan);
    }

    #[test]
    fn counted_j_moves_that_many_rows() {
        let mut board = Board::default();
        ingest_panes(&mut board, 15);
        board.decide(Key::Digit(1));
        board.decide(Key::Digit(0));
        board.decide(Key::Down);
        assert_eq!(board.selected, 10);
    }

    #[test]
    fn gg_goes_to_the_first_row_and_g_to_the_last() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.selected = 5;
        board.decide(Key::GPrefix);
        board.decide(Key::GPrefix);
        assert_eq!(board.selected, 0);
        board.decide(Key::Last);
        assert_eq!(board.selected, 7);
    }

    #[test]
    fn counted_g_is_a_one_based_line() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.decide(Key::Digit(3));
        board.decide(Key::Last);
        assert_eq!(board.selected, 2);
        board.decide(Key::Digit(3));
        board.decide(Key::GPrefix);
        board.decide(Key::GPrefix);
        assert_eq!(board.selected, 2);
    }

    #[test]
    fn ctrl_d_moves_half_the_page() {
        let mut board = Board::default();
        ingest_panes(&mut board, 20);
        board.set_page_len(10);
        board.decide(Key::HalfPageDown);
        assert_eq!(board.selected, 5);
        board.decide(Key::HalfPageUp);
        assert_eq!(board.selected, 0);
        board.decide(Key::PageDown);
        assert_eq!(board.selected, 10);
    }

    #[test]
    fn lone_zero_goes_to_the_first_row() {
        let mut board = Board::default();
        ingest_panes(&mut board, 6);
        board.selected = 4;
        board.decide(Key::Digit(0));
        assert_eq!(board.selected, 0);
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

    #[test]
    fn flash_sole_tab_match_jumps_immediately() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.agents[0].tab_name = "Geo-DB".into();
        board.agents[1].tab_name = "notes".into();
        board.decide(Key::StartHint);
        assert_eq!(
            board.decide(Key::Input('g')),
            Action::Jump {
                session: "lp".into(),
                pane_id: 8,
            }
        );
        assert!(!board.is_hinting());
    }

    #[test]
    fn flash_ambiguous_query_then_tip_jumps() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.agents[0].tab_name = "notes".into();
        board.agents[1].tab_name = "logs".into();
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('o')), Action::None);
        assert_eq!(board.hint_query(), "o");
        assert!(board.hint_label(0).is_some());
        assert!(board.hint_label(1).is_some());
        let label = board.hint_label(0).unwrap().to_owned();
        assert_eq!(
            board.decide(Key::Input(label.chars().next().unwrap())),
            Action::Jump {
                session: "lp".into(),
                pane_id: 8,
            }
        );
    }

    #[test]
    fn flash_matches_session_project_tab_not_pane_title_or_detail() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.agents[1].status = Status::Working;
        board.agents[1].detail = "Shell cargo test".into();
        board.agents[1].pane_title = "retry".into();
        board.decide(Key::StartHint);
        assert_eq!(
            board.decide(Key::Input('w')),
            Action::Jump {
                session: "ww".into(),
                pane_id: 3,
            }
        );
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('r')), Action::None);
        assert_eq!(board.hint_query(), "");
        assert_eq!(board.decide(Key::Input('s')), Action::None);
        assert_eq!(board.hint_query(), "");
    }

    #[test]
    fn flash_matches_tab_name() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.agents[0].workspace = None;
        board.agents[1].workspace = None;
        board.agents[0].tab_name = "openapi".into();
        board.agents[1].tab_name = "main".into();
        board.agents[0].pane_title = "should-not-match-this".into();
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('s')), Action::None);
        assert_eq!(board.hint_query(), "");
        assert_eq!(
            board.decide(Key::Input('o')),
            Action::Jump {
                session: "lp".into(),
                pane_id: 8,
            }
        );
    }

    #[test]
    fn flash_ignores_a_unique_match_outside_the_viewport() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.set_list_geometry(80, 8);
        board.selected = 7;
        let (start, end) = board.visible_range();
        assert!(start > 0);
        board.agents[0].tab_name = "Geo-DB".into();
        for agent in board.agents[start..end].iter_mut() {
            agent.tab_name = "notes".into();
        }
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('g')), Action::None);
        assert_eq!(board.hint_query(), "");
        assert!(board.is_hinting());
    }

    #[test]
    fn flash_sole_visible_match_jumps_even_if_more_exist_offscreen() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.set_list_geometry(80, 8);
        board.selected = 7;
        let (start, end) = board.visible_range();
        assert!(start > 0);
        board.agents[0].tab_name = "Geo-DB".into();
        board.agents[end - 1].tab_name = "Geo-DB".into();
        board.decide(Key::StartHint);
        assert_eq!(
            board.decide(Key::Input('g')),
            Action::Jump {
                session: "ww".into(),
                pane_id: board.agents[end - 1].id.pane_id,
            }
        );
        assert!(!board.is_hinting());
    }

    #[test]
    fn flash_labels_stay_inside_the_visible_viewport() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.set_list_geometry(80, 8);
        board.selected = 7;
        let (start, end) = board.visible_range();
        assert!(start > 0);
        for agent in &mut board.agents {
            agent.tab_name = "notes".into();
        }
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('o')), Action::None);
        for index in 0..board.agents.len() {
            if index >= start && index < end {
                assert!(board.hint_label(index).is_some(), "visible {index}");
            } else {
                assert!(board.hint_label(index).is_none(), "offscreen {index}");
            }
        }
    }

    #[test]
    fn flash_does_not_scroll_away_when_matches_are_already_visible() {
        let mut board = Board::default();
        ingest_panes(&mut board, 8);
        board.set_list_geometry(80, 8);
        board.decide(Key::Last);
        let selected = board.selected;
        assert!(selected > 0);
        for agent in &mut board.agents {
            agent.tab_name = "notes".into();
        }
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Input('o')), Action::None);
        assert_eq!(board.selected, selected);
    }

    #[test]
    fn slash_search_moves_selection_and_enter_stays_on_the_board() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.selected = 0;
        assert_eq!(board.agents[0].id.session, "lp");
        board.decide(Key::StartSearch);
        assert_eq!(board.decide(Key::Input('w')), Action::None);
        assert_eq!(board.search_query(), "w");
        assert_eq!(board.agents[board.selected].id.session, "ww");
        assert_eq!(board.decide(Key::Confirm), Action::None);
        assert!(!board.is_searching());
        assert_eq!(board.agents[board.selected].id.session, "ww");
    }

    #[test]
    fn slash_search_escape_restores_the_origin_row() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.selected = 0;
        board.decide(Key::StartSearch);
        board.decide(Key::Input('w'));
        assert_eq!(board.selected, 1);
        assert_eq!(board.decide(Key::Dismiss), Action::None);
        assert_eq!(board.selected, 0);
        assert!(!board.is_searching());
    }

    #[test]
    fn n_walks_matches_after_search_accept() {
        let mut board = Board::default();
        board.ingest(
            "\
META hooks=1
SCAN ww 3 agent /Users/ww/.local/bin/agent --workspace /tmp/a
SCAN ww 4 agent /Users/ww/.local/bin/agent --workspace /tmp/b
SCAN lp 8 agent /Users/ww/.local/bin/agent --workspace /tmp/lp
",
        );
        board.decide(Key::StartSearch);
        board.decide(Key::Input('w'));
        board.decide(Key::Confirm);
        let first = board.selected;
        assert_eq!(board.agents[first].id.session, "ww");
        board.decide(Key::NextMatch);
        assert_eq!(board.agents[board.selected].id.session, "ww");
        assert_ne!(board.selected, first);
        board.decide(Key::NextMatch);
        assert_eq!(board.selected, first);
    }

    #[test]
    fn slash_search_ignores_pane_title_and_detail() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.agents[1].detail = "Shell cargo test".into();
        board.agents[1].pane_title = "retry".into();
        board.decide(Key::StartSearch);
        assert_eq!(board.decide(Key::Input('r')), Action::None);
        assert_eq!(board.search_query(), "");
    }

    #[test]
    fn escape_cancels_flash_before_dismissing() {
        let mut board = Board::default();
        ingest_two(&mut board);
        board.decide(Key::StartHint);
        assert_eq!(board.decide(Key::Dismiss), Action::None);
        assert!(!board.is_hinting());
        assert_eq!(board.decide(Key::Dismiss), Action::Dismiss);
    }
}
