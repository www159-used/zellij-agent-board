//! Zellij WASM adapter. Core list / jump / hook merge lives in `agent_board`.

use std::collections::BTreeMap;

use agent_board::{
    float_size_from_config, Action, AgentId, Board, FloatSize, FloatingLayerState, Key, PaintCtx,
    PanePlace,
};
use zellij_tile::prelude::*;

const PLUGIN_NAME: &str = "agent-board";
const SCAN_LAUNCHER: &str = r#"
s="${AGENT_BOARD_SCAN:-$HOME/.config/zellij/plugins/agent-board-scan.sh}"
if [ -x "$s" ]; then exec "$s"; fi
if [ -x ./scripts/scan-agents.sh ]; then exec ./scripts/scan-agents.sh; fi
printf 'META hooks=0\n'
"#;

#[derive(Default)]
struct State {
    board: Board,
    own_plugin_id: Option<u32>,
    client_id: Option<ClientId>,
    permissions_granted: bool,
    floating_layer: FloatingLayerState,
    current_session: Option<String>,
    tab_names: BTreeMap<(String, usize), String>,
    places: BTreeMap<AgentId, PanePlace>,
    scan_inflight: bool,
    pane_manifest: Option<PaneManifest>,
    float_size: FloatSize,
    enlarge_pending: bool,
    enlarged_once: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_plugin_id = Some(ids.plugin_id);
        self.client_id = Some(ids.client_id);
        self.float_size = float_size_from_config(&configuration);
        subscribe(&[
            EventType::Key,
            EventType::SessionUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::RunCommandResult,
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
                self.permissions_granted = result == PermissionStatus::Granted;
                if self.permissions_granted {
                    if let Some(id) = self.own_plugin_id {
                        rename_plugin_pane(id, PLUGIN_NAME);
                    }
                    self.remember_sessions(&fetch_live_sessions());
                    self.request_scan();
                    self.enlarge_if_floating();
                    set_timeout(1.0);
                }
                true
            }
            Event::Timer(_) => {
                self.board.tick();
                set_timeout(1.0);
                true
            }
            Event::SessionUpdate(sessions, _) => {
                if let Some(session) = sessions.iter().find(|session| session.is_current_session) {
                    self.current_session = Some(session.name.clone());
                    self.floating_layer
                        .capture(self.previous_pane_was_floating(session));
                }
                self.remember_sessions(&sessions);
                self.request_scan();
                true
            }
            Event::PaneUpdate(manifest) => {
                self.pane_manifest = Some(manifest);
                if let Some(session) = self.current_session.clone() {
                    if let Some(manifest) = self.pane_manifest.as_ref() {
                        self.remember_places(places_from_manifest(
                            &session,
                            manifest,
                            &self.tab_names,
                        ));
                    }
                }
                self.enlarge_if_floating();
                true
            }
            Event::RunCommandResult(_code, stdout, _stderr, context) => {
                if context.get("agent_board").map(String::as_str) != Some("scan") {
                    return false;
                }
                self.scan_inflight = false;
                let text = String::from_utf8_lossy(&stdout);
                self.board.ingest(&text);
                self.push_places();
                true
            }
            Event::Key(key) => {
                let Some(mapped) = map_key(key) else {
                    return false;
                };
                let action = self.board.decide(mapped);
                self.apply(action)
            }
            _ => false,
        }
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let Some(payload) = pipe_message.payload.as_deref() else {
            return false;
        };
        let before_hooks = self.board.hooks_installed;
        let before = self.board.agents.clone();
        self.board.ingest_notice(payload);
        self.board.agents != before || self.board.hooks_installed != before_hooks
    }

    fn render(&mut self, rows: usize, cols: usize) {
        if self.enlarge_pending {
            self.enlarge_pending = false;
            self.enlarged_once = true;
            return;
        }
        if !self.can_paint() {
            return;
        }
        let frame = self.board.paint(PaintCtx {
            rows,
            cols,
            home: self.current_session.as_deref().unwrap_or(""),
        });
        write_plugin_lines(&frame.lines);
    }
}

impl State {
    fn can_paint(&self) -> bool {
        if !self.permissions_granted {
            return false;
        }
        let Some(own_id) = self.own_plugin_id else {
            return false;
        };
        let Some(manifest) = self.pane_manifest.as_ref() else {
            return false;
        };
        let floating = manifest
            .panes
            .values()
            .flatten()
            .any(|pane| pane.is_plugin && pane.id == own_id && pane.is_floating);
        !floating || self.enlarged_once
    }

    fn enlarge_if_floating(&mut self) {
        if self.enlarged_once || self.enlarge_pending || !self.permissions_granted {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let Some(manifest) = self.pane_manifest.as_ref() else {
            return;
        };
        let floating = manifest
            .panes
            .values()
            .flatten()
            .any(|pane| pane.is_plugin && pane.id == own_id && pane.is_floating);
        if !floating {
            return;
        }
        let Some(coords) = FloatingPaneCoordinates::new(
            Some(self.float_size.x.clone()),
            Some(self.float_size.y.clone()),
            Some(self.float_size.width.clone()),
            Some(self.float_size.height.clone()),
            None,
            None,
        ) else {
            return;
        };
        self.enlarge_pending = true;
        change_floating_panes_coordinates(vec![(PaneId::Plugin(own_id), coords)]);
    }

    fn request_scan(&mut self) {
        if !self.permissions_granted || self.scan_inflight {
            return;
        }
        self.scan_inflight = true;
        let mut context = BTreeMap::new();
        context.insert("agent_board".to_string(), "scan".to_string());
        run_command(&["/bin/bash", "-lc", SCAN_LAUNCHER], context);
    }

    fn remember_sessions(&mut self, sessions: &[SessionInfo]) {
        self.tab_names.extend(tab_names_from_sessions(sessions));
        self.remember_places(places_from_sessions(sessions));
    }

    fn remember_places(&mut self, places: Vec<(AgentId, PanePlace)>) {
        for (id, place) in places {
            self.places.insert(id, place);
        }
        self.push_places();
    }

    fn push_places(&mut self) {
        self.board.apply_places(
            self.places
                .iter()
                .map(|(id, place)| (id.clone(), place.clone())),
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

    fn apply(&self, action: Action) -> bool {
        match action {
            Action::Dismiss => {
                self.dismiss();
                false
            }
            Action::Jump { session, pane_id } => {
                self.dismiss();
                if self.current_session.as_deref() == Some(session.as_str()) {
                    focus_terminal_pane(pane_id, true, false);
                } else {
                    switch_session_with_focus(&session, None, Some((pane_id, false)));
                }
                false
            }
            Action::None => true,
        }
    }

    fn dismiss(&self) {
        if self.floating_layer.should_hide_on_close() {
            hide_self();
        }
        close_self();
    }
}

fn fetch_live_sessions() -> Vec<SessionInfo> {
    get_session_list()
        .map(|snapshot| snapshot.live_sessions)
        .unwrap_or_default()
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

fn write_plugin_lines(lines: &[String]) {
    let Some((last, rest)) = lines.split_last() else {
        return;
    };
    for line in rest {
        println!("{line}");
    }
    print!("{last}");
}

fn map_key(key: KeyWithModifier) -> Option<Key> {
    if !key.has_no_modifiers() {
        return None;
    }
    match key.bare_key {
        BareKey::Esc | BareKey::Char('q') => Some(Key::Dismiss),
        BareKey::Char('?') => Some(Key::ToggleHelp),
        BareKey::Up | BareKey::Char('k') | BareKey::Left | BareKey::Char('h') => Some(Key::Up),
        BareKey::Down | BareKey::Char('j') | BareKey::Right | BareKey::Char('l') => Some(Key::Down),
        BareKey::Enter | BareKey::Char('e') => Some(Key::Confirm),
        _ => None,
    }
}
