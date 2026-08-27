//! Zellij WASM bridge. Scan / paint / keys live in the host `board-tui`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use zellij_agent_board::{
    bridge_close_plan, float_size_from_config, format_focus, format_places, now_ms, parse_jump,
    AgentId, BridgeClosePlan, FloatSize, FloatingLayerState, PanePlace,
};
use zellij_tile::prelude::*;

const PLUGIN_NAME: &str = "zellij-agent-board";
const PLACES_WRITER: &str = r#"
dir="${TMPDIR:-/tmp}/zellij-agent-board"
mkdir -p "$dir"
file="$dir/places"
incoming="$dir/places.incoming"
printf '%s' "${ZAB_PLACES}" >"$incoming"
if [ ! -s "$incoming" ]; then
  rm -f "$incoming"
  exit 0
fi
if [ ! -f "$file" ]; then
  mv "$incoming" "$file"
  exit 0
fi
awk '
  FNR == NR {
    if ($0 ~ /^PLACE /) {
      split($0, a, /[ \t]+/)
      seen[a[2] SUBSEP a[3]] = 1
      print
    }
    next
  }
  /^PLACE / {
    split($0, a, /[ \t]+/)
    if (!((a[2] SUBSEP a[3]) in seen)) print
  }
' "$incoming" "$file" | sort >"$file.tmp" && mv "$file.tmp" "$file"
rm -f "$incoming"
"#;

const FOCUS_WRITER: &str = r#"
dir="${TMPDIR:-/tmp}/zellij-agent-board"
mkdir -p "$dir"
printf '%s' "${ZAB_FOCUS}" >"$dir/focus"
"#;

#[derive(Default)]
struct State {
    own_plugin_id: Option<u32>,
    client_id: Option<ClientId>,
    permissions_granted: bool,
    floating_layer: FloatingLayerState,
    current_session: Option<String>,
    tab_names: BTreeMap<(String, usize), String>,
    places: BTreeMap<AgentId, PanePlace>,
    known_board_ids: std::collections::BTreeSet<u32>,
    last_places: String,
    pane_manifest: Option<PaneManifest>,
    float_size: FloatSize,
    opened_at_ms: u64,
    dying: bool,
    tui_path: String,
    tui_id: Option<u32>,
    tui_visible: bool,
    tui_attempts: u8,
    launch_focus_written: bool,
    bridge_hidden: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_plugin_id = Some(ids.plugin_id);
        self.client_id = Some(ids.client_id);
        self.opened_at_ms = now_ms();
        self.float_size = float_size_from_config(&configuration);
        self.tui_path = tui_path(&configuration);
        subscribe(&[
            EventType::SessionUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::CommandPaneOpened,
            EventType::CommandPaneExited,
            EventType::PaneClosed,
            EventType::Timer,
        ]);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
            PermissionType::ReadCliPipes,
        ]);
    }

    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::PermissionRequestResult(result) => {
                if result == PermissionStatus::Granted {
                    self.permissions_granted = true;
                    if let Some(id) = self.own_plugin_id {
                        rename_plugin_pane(id, PLUGIN_NAME);
                    }
                    self.enlarge_self();
                    if !self.close_if_duplicate() {
                        set_timeout(0.1);
                    }
                }
                false
            }
            Event::Timer(_) => {
                if !self.dying && self.permissions_granted && self.tui_id.is_none() {
                    self.ensure_tui();
                    if self.tui_id.is_none() && self.tui_attempts < 3 {
                        set_timeout(0.4);
                    }
                }
                false
            }
            Event::SessionUpdate(sessions, _) => {
                if let Some(session) = sessions.iter().find(|session| session.is_current_session) {
                    if self.current_session.as_deref() != Some(session.name.as_str()) {
                        self.current_session = Some(session.name.clone());
                    }
                    self.floating_layer
                        .capture(self.previous_pane_was_floating(session));
                    self.remember_launch_focus(session);
                }
                self.harvest_board_ids_from_sessions(&sessions);
                self.remember_sessions(&sessions);
                false
            }
            Event::PaneUpdate(manifest) => {
                self.harvest_board_ids_from_manifest(&manifest);
                self.pane_manifest = Some(manifest);
                if self.close_if_duplicate() {
                    return false;
                }
                self.adopt_existing_tui();
                if self.tui_id.is_some() {
                    self.hide_bridge_keep_tui();
                }
                if let Some(session) = self.current_session.clone() {
                    if let Some(manifest) = self.pane_manifest.as_ref() {
                        self.remember_places(places_from_manifest(
                            &session,
                            manifest,
                            &self.tab_names,
                        ));
                    }
                }
                false
            }
            Event::CommandPaneOpened(pane_id, context) => {
                if context.get("zellij_agent_board").map(String::as_str) == Some("tui") {
                    self.tui_id = Some(pane_id);
                    self.tui_visible = true;
                    self.hide_bridge_keep_tui();
                }
                false
            }
            Event::CommandPaneExited(pane_id, _, _)
            | Event::PaneClosed(PaneId::Terminal(pane_id)) => {
                if self.tui_id == Some(pane_id) {
                    self.tui_id = None;
                    self.tui_visible = false;
                    self.shutdown_bridge();
                }
                false
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let Some(payload) = pipe_message.payload.as_deref() else {
            return false;
        };
        if let Some((session, pane_id)) = parse_jump(payload) {
            if self.current_session.as_deref() == Some(session.as_str()) {
                focus_terminal_pane(pane_id, false, false);
            } else {
                switch_session_with_focus(&session, None, Some((pane_id, false)));
            }
            let ids = self.all_board_plugin_ids();
            self.shutdown_board(&ids);
        }
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn close_if_duplicate(&mut self) -> bool {
        if self.dying || !self.permissions_granted {
            return self.dying;
        }
        let Some(own_id) = self.own_plugin_id else {
            return false;
        };
        let board_ids = self.all_board_plugin_ids();
        if board_ids.len() <= 1 {
            return false;
        }
        let tui_up = self.tui_id.is_some() || self.find_tui_pane().is_some();
        match bridge_close_plan(own_id, &board_ids, self.opened_at_ms, now_ms(), tui_up) {
            BridgeClosePlan::None => false,
            BridgeClosePlan::Drop { ids } => {
                let dying = ids.contains(&own_id);
                if dying {
                    self.dying = true;
                }
                for id in ids {
                    close_pane_with_id(PaneId::Plugin(id));
                }
                if dying {
                    close_self();
                }
                dying
            }
            BridgeClosePlan::Shutdown { ids } => {
                self.shutdown_board(&ids);
                true
            }
        }
    }

    fn ensure_tui(&mut self) {
        if self.dying || !self.permissions_granted {
            return;
        }
        self.adopt_existing_tui();
        if self.tui_id.is_some() {
            self.hide_bridge_keep_tui();
            return;
        }
        if self.tui_attempts >= 3 || self.find_tui_pane().is_some() {
            return;
        }
        self.open_tui();
    }

    fn find_tui_pane(&self) -> Option<u32> {
        self.pane_manifest.as_ref().and_then(|manifest| {
            manifest
                .panes
                .values()
                .flatten()
                .find(|pane| is_board_tui(pane))
                .map(|pane| pane.id)
        })
    }

    fn adopt_existing_tui(&mut self) {
        if self.tui_id.is_some() {
            return;
        }
        if let Some(id) = self.find_tui_pane() {
            self.tui_id = Some(id);
            self.tui_visible = true;
        }
    }

    fn open_tui(&mut self) {
        self.tui_attempts = self.tui_attempts.saturating_add(1);
        // `zellij action new-pane --floating` is the path we already verified
        // from the host. Opening a command pane from inside plugin `update`
        // either returns None or lands in a hidden float layer.
        let mut context = BTreeMap::new();
        context.insert("zellij_agent_board".to_string(), "tui-launch".to_string());
        let mut args = vec!["zellij".to_string()];
        if let Some(session) = self
            .current_session
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            args.push("--session".into());
            args.push(session.to_string());
        }
        args.extend([
            "action".into(),
            "new-pane".into(),
            "--floating".into(),
            "--near-current-pane".into(),
            "--close-on-exit".into(),
            "--name".into(),
            "board-tui".into(),
            "--width".into(),
            self.float_size.width.clone(),
            "--height".into(),
            self.float_size.height.clone(),
            "--x".into(),
            self.float_size.x.clone(),
            "--y".into(),
            self.float_size.y.clone(),
            "--".into(),
            self.tui_path.clone(),
        ]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run_command(&args, context);
    }

    fn enlarge_self(&self) {
        let Some(id) = self.own_plugin_id else {
            return;
        };
        let Some(coords) = self.float_coords() else {
            return;
        };
        change_floating_panes_coordinates(vec![(PaneId::Plugin(id), coords)]);
    }

    fn float_coords(&self) -> Option<FloatingPaneCoordinates> {
        FloatingPaneCoordinates::new(
            Some(self.float_size.x.clone()),
            Some(self.float_size.y.clone()),
            Some(self.float_size.width.clone()),
            Some(self.float_size.height.clone()),
            None,
            None,
        )
    }

    fn hide_bridge_keep_tui(&mut self) {
        if self.bridge_hidden {
            return;
        }
        let _ = show_floating_panes(None);
        if let Some(id) = self.tui_id {
            show_pane_with_id(PaneId::Terminal(id), true, true);
        }
        hide_self();
        let _ = show_floating_panes(None);
        if let Some(id) = self.tui_id {
            show_pane_with_id(PaneId::Terminal(id), true, true);
        }
        self.bridge_hidden = true;
    }

    fn shutdown_bridge(&mut self) {
        self.dying = true;
        close_self();
    }

    fn shutdown_board(&mut self, plugin_ids: &[u32]) {
        self.dying = true;
        if let Some(id) = self.tui_id.or_else(|| self.find_tui_pane()) {
            close_pane_with_id(PaneId::Terminal(id));
        }
        if let Some(manifest) = self.pane_manifest.as_ref() {
            for pane in manifest.panes.values().flatten() {
                if is_board_tui(pane) {
                    close_pane_with_id(PaneId::Terminal(pane.id));
                }
            }
        }
        for id in plugin_ids {
            close_pane_with_id(PaneId::Plugin(*id));
        }
        if self.floating_layer.should_hide_on_close() {
            let _ = hide_floating_panes(None);
        }
        close_self();
    }

    fn harvest_board_ids_from_sessions(&mut self, sessions: &[SessionInfo]) {
        let Some(session) = sessions.iter().find(|session| session.is_current_session) else {
            return;
        };
        // Plugin pane ids are unique only inside one session. Mixing ww leftover
        // ids (often high) with a new board in lp/learn (often low) made
        // bridge_close_plan treat the new Alt+q as a second instance and shut
        // the board down — empty "no cursor agents" or a flash then gone.
        self.known_board_ids.clear();
        for pane in session.panes.panes.values().flatten() {
            if is_board_plugin(pane) {
                self.known_board_ids.insert(pane.id);
            }
        }
    }

    fn harvest_board_ids_from_manifest(&mut self, manifest: &PaneManifest) {
        for pane in manifest.panes.values().flatten() {
            if is_board_plugin(pane) {
                self.known_board_ids.insert(pane.id);
            }
        }
    }

    fn all_board_plugin_ids(&self) -> Vec<u32> {
        let mut ids = self.known_board_ids.clone();
        if let Some(own_id) = self.own_plugin_id {
            ids.insert(own_id);
        }
        ids.into_iter().collect()
    }

    fn remember_sessions(&mut self, sessions: &[SessionInfo]) {
        for (key, name) in tab_names_from_sessions(sessions) {
            self.tab_names.insert(key, name);
        }
        self.remember_places(places_from_sessions(sessions));
    }

    fn remember_places(&mut self, places: Vec<(AgentId, PanePlace)>) {
        let mut stored = false;
        for (id, place) in places {
            let place = match self.places.get(&id) {
                Some(old) => place.keep_names(&old.tab_name, &old.pane_title),
                None => place,
            };
            if self.places.get(&id) != Some(&place) {
                self.places.insert(id, place);
                stored = true;
            }
        }
        if stored {
            self.flush_places();
        }
    }

    fn flush_places(&mut self) {
        let text = format_places(self.places.iter().filter_map(|(id, place)| {
            if place.tab_name.is_empty() && place.pane_title.is_empty() {
                None
            } else {
                Some((id.clone(), place.clone()))
            }
        }));
        if text == self.last_places {
            return;
        }
        self.last_places = text.clone();
        let mut env = BTreeMap::new();
        env.insert("ZAB_PLACES".to_string(), text);
        let mut context = BTreeMap::new();
        context.insert("zellij_agent_board".to_string(), "places".to_string());
        run_command_with_env_variables_and_cwd(
            &["/bin/bash", "-lc", PLACES_WRITER],
            env,
            PathBuf::from("."),
            context,
        );
    }

    fn remember_launch_focus(&mut self, session: &SessionInfo) {
        if self.launch_focus_written || session.name.is_empty() {
            return;
        }
        let Some(pane_id) = self.previous_terminal_pane(session) else {
            return;
        };
        self.flush_focus(&session.name, pane_id);
        self.launch_focus_written = true;
    }

    fn previous_terminal_pane(&self, session: &SessionInfo) -> Option<u32> {
        let own = PaneId::Plugin(self.own_plugin_id?);
        session
            .pane_history
            .get(&self.client_id?)?
            .iter()
            .rev()
            .find_map(|id| {
                if *id == own {
                    return None;
                }
                match id {
                    PaneId::Terminal(pane) => Some(*pane),
                    PaneId::Plugin(_) => None,
                }
            })
    }

    fn flush_focus(&self, session: &str, pane_id: u32) {
        let mut env = BTreeMap::new();
        env.insert("ZAB_FOCUS".to_string(), format_focus(session, pane_id));
        let mut context = BTreeMap::new();
        context.insert("zellij_agent_board".to_string(), "focus".to_string());
        run_command_with_env_variables_and_cwd(
            &["/bin/bash", "-lc", FOCUS_WRITER],
            env,
            PathBuf::from("."),
            context,
        );
    }

    fn previous_pane_was_floating(&self, session: &SessionInfo) -> Option<bool> {
        let previous_pane = self.nth_previous_pane(session)?;
        session
            .panes
            .panes
            .values()
            .flatten()
            .find(|pane| pane_id(pane) == previous_pane)
            .map(|pane| pane.is_floating)
    }

    fn nth_previous_pane(&self, session: &SessionInfo) -> Option<PaneId> {
        let own_pane = PaneId::Plugin(self.own_plugin_id?);
        session
            .pane_history
            .get(&self.client_id?)?
            .iter()
            .rev()
            .find(|pane_id| **pane_id != own_pane)
            .copied()
    }
}

fn tui_path(configuration: &BTreeMap<String, String>) -> String {
    if let Some(path) = configuration.get("tui") {
        return path.clone();
    }
    if let Ok(path) = std::env::var("ZELLIJ_AGENT_BOARD_TUI") {
        return path;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/.config/zellij/plugins/board-tui")
}

fn is_board_plugin(pane: &PaneInfo) -> bool {
    pane.is_plugin
        && wasm_basename(pane.plugin_url.as_deref().unwrap_or(""))
            == Some("zellij-agent-board.wasm")
}

fn is_board_tui(pane: &PaneInfo) -> bool {
    if pane.is_plugin {
        return false;
    }
    pane.terminal_command
        .as_deref()
        .is_some_and(|command| command.contains("board-tui"))
        || pane.title.contains("board-tui")
}

fn tab_names_from_sessions(sessions: &[SessionInfo]) -> BTreeMap<(String, usize), String> {
    sessions
        .iter()
        .flat_map(|session| {
            session.tabs.iter().filter_map(|tab| {
                let name = tab.name.trim();
                (!name.is_empty()).then(|| ((session.name.clone(), tab.position), name.to_string()))
            })
        })
        .collect()
}

fn tab_name(
    session: &str,
    tab_position: usize,
    names: &BTreeMap<(String, usize), String>,
) -> String {
    names
        .get(&(session.to_string(), tab_position))
        .cloned()
        .unwrap_or_else(|| format!("tab {tab_position}"))
}

fn places_from_sessions(sessions: &[SessionInfo]) -> Vec<(AgentId, PanePlace)> {
    let names = tab_names_from_sessions(sessions);
    sessions
        .iter()
        .flat_map(|session| {
            session
                .panes
                .panes
                .iter()
                .flat_map(|(&tab_position, panes)| {
                    let name = tab_name(&session.name, tab_position, &names);
                    panes
                        .iter()
                        .filter(|pane| !pane.is_plugin)
                        .map(|pane| {
                            (
                                AgentId {
                                    session: session.name.clone(),
                                    pane_id: pane.id,
                                },
                                PanePlace {
                                    tab_position,
                                    tab_name: name.clone(),
                                    pane_title: pane.title.trim().to_string(),
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                })
        })
        .collect()
}

fn places_from_manifest(
    session: &str,
    manifest: &PaneManifest,
    names: &BTreeMap<(String, usize), String>,
) -> Vec<(AgentId, PanePlace)> {
    manifest
        .panes
        .iter()
        .flat_map(|(&tab_position, panes)| {
            let name = tab_name(session, tab_position, names);
            panes
                .iter()
                .filter(|pane| !pane.is_plugin)
                .map(move |pane| {
                    (
                        AgentId {
                            session: session.to_string(),
                            pane_id: pane.id,
                        },
                        PanePlace {
                            tab_position,
                            tab_name: name.clone(),
                            pane_title: pane.title.trim().to_string(),
                        },
                    )
                })
        })
        .collect()
}

fn pane_id(pane: &PaneInfo) -> PaneId {
    if pane.is_plugin {
        PaneId::Plugin(pane.id)
    } else {
        PaneId::Terminal(pane.id)
    }
}

fn wasm_basename(url: &str) -> Option<&str> {
    url.rsplit('/')
        .next()
        .filter(|name| name.ends_with(".wasm"))
}
