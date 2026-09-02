//! Zellij WASM bridge. Scan / paint / keys live in the host `board-tui`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use zellij_agent_board::{
    bridge_close_plan, float_size_from_config, format_focus, format_places, is_host_tui_exit,
    looks_like_board_tui, now_ms, parse_jump, should_open_tui, should_shutdown_on_tui_close,
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

/// `bash -c` (not `-l`) keeps ZAB_* env. Absolute zellij — plugin PATH
/// has no Homebrew. Regular `new-pane`, not a command pane (those steal Esc).
const TUI_LAUNCHER: &str = r#"
zellij_bin="${ZAB_ZELLIJ:-/opt/homebrew/bin/zellij}"
cmd=("$zellij_bin")
if [ -n "${ZAB_SESSION:-}" ]; then
  cmd+=(--session "$ZAB_SESSION")
fi
cmd+=(action new-pane --floating --close-on-exit --name board-tui
  --width "$ZAB_W" --height "$ZAB_H" --x "$ZAB_X" --y "$ZAB_Y")
if [ -n "${ZAB_TAB:-}" ]; then
  cmd+=(--tab-id "$ZAB_TAB")
fi
cmd+=(-- "$ZAB_TUI")
"${cmd[@]}"
"#;

#[derive(Default)]
struct State {
    own_plugin_id: Option<u32>,
    client_id: Option<ClientId>,
    permissions_granted: bool,
    floating_layer: FloatingLayerState,
    current_session: Option<String>,
    own_tab_id: Option<usize>,
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
                    // A previous quit may have hidden the float layer.
                    // LaunchPlugin { floating } then lands in that layer and
                    // Alt+q looks like a no-op. Do not enlarge this empty
                    // pane — it would cover the board until SessionUpdate.
                    let _ = self.show_float();
                    if !self.close_if_duplicate() {
                        self.ensure_tui(false);
                        if self.tui_id.is_none() {
                            set_timeout(0.5);
                        }
                    }
                }
                false
            }
            Event::Timer(_) => {
                if self.permissions_granted && self.tui_id.is_none() {
                    self.ensure_tui(true);
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
                    self.remember_own_tab(session);
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
            Event::CommandPaneExited(..) => false,
            Event::PaneClosed(PaneId::Terminal(pane_id)) => {
                if is_host_tui_exit(self.tui_id, pane_id, false) {
                    self.tui_id = None;
                    self.tui_visible = false;
                    if should_shutdown_on_tui_close(self.bridge_hidden) {
                        self.shutdown_bridge();
                    } else if self.tui_attempts < 3 {
                        set_timeout(0.4);
                    }
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

    fn ensure_tui(&mut self, from_timer: bool) {
        self.adopt_existing_tui();
        if self.tui_id.is_some() {
            self.hide_bridge_keep_tui();
            return;
        }
        if !should_open_tui(
            self.permissions_granted,
            self.dying,
            self.tui_id.is_some() || self.find_tui_pane().is_some(),
            self.tui_attempts,
            from_timer,
        ) {
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
        let _ = self.show_float();
        let mut context = BTreeMap::new();
        context.insert("zellij_agent_board".to_string(), "tui-launch".to_string());
        let mut env = BTreeMap::new();
        env.insert("ZAB_TUI".to_string(), self.tui_path.clone());
        env.insert("ZAB_W".to_string(), self.float_size.width.clone());
        env.insert("ZAB_H".to_string(), self.float_size.height.clone());
        env.insert("ZAB_X".to_string(), self.float_size.x.clone());
        env.insert("ZAB_Y".to_string(), self.float_size.y.clone());
        env.insert("ZAB_ZELLIJ".to_string(), "/opt/homebrew/bin/zellij".into());
        if let Some(session) = self
            .current_session
            .as_deref()
            .filter(|name| !name.is_empty())
        {
            env.insert("ZAB_SESSION".to_string(), session.to_string());
        }
        if let Some(tab_id) = self.own_tab_id {
            env.insert("ZAB_TAB".to_string(), tab_id.to_string());
        }
        run_command_with_env_variables_and_cwd(
            &["/bin/bash", "-c", TUI_LAUNCHER],
            env,
            PathBuf::from("."),
            context,
        );
    }

    fn show_float(&self) -> Result<bool, String> {
        show_floating_panes(self.own_tab_id)
    }

    fn remember_own_tab(&mut self, session: &SessionInfo) {
        if let Some(own) = self.own_plugin_id {
            for tab in &session.tabs {
                let Some(panes) = session.panes.panes.get(&tab.position) else {
                    continue;
                };
                if panes.iter().any(|pane| pane.is_plugin && pane.id == own) {
                    self.own_tab_id = Some(tab.tab_id);
                    return;
                }
            }
        }
        if let Some(tab) = session.tabs.iter().find(|tab| tab.active) {
            self.own_tab_id = Some(tab.tab_id);
        }
    }

    fn hide_bridge_keep_tui(&mut self) {
        if self.bridge_hidden {
            return;
        }
        let _ = self.show_float();
        if let Some(id) = self.tui_id {
            show_pane_with_id(PaneId::Terminal(id), true, true);
        }
        hide_self();
        let _ = self.show_float();
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
            let _ = hide_floating_panes(self.own_tab_id);
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
    looks_like_board_tui(pane.terminal_command.as_deref(), &pane.title)
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
