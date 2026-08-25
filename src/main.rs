//! Zellij WASM adapter. Core list / jump / hook merge lives in `zellij_agent_board`.

use std::collections::BTreeMap;

use zellij_agent_board::{
    closes_the_board, duplicate_close_ids_with_focus, float_size_from_config, now_ms, Action,
    AgentId, Board, FloatSize, FloatingLayerState, Key, PaintCtx, PanePlace, TOGGLE_DEBOUNCE_MS,
};
use zellij_tile::prelude::*;

const PLUGIN_NAME: &str = "zellij-agent-board";
/// Default Zellij float is a title strip. Treat anything this large as enlarged.
const ENLARGE_MIN_ROWS: usize = 12;
const ENLARGE_MIN_COLS: usize = 40;
/// One enlarge retry after open. After this, never resize again (leftovers
/// must not keep calling change_floating_panes_coordinates).
const OPEN_ENLARGE_RETRY_MS: u64 = 400;
const SCAN_LAUNCHER: &str = r#"
s="${ZELLIJ_AGENT_BOARD_SCAN:-$HOME/.config/zellij/plugins/zellij-agent-board-scan.sh}"
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
    enlarge_issued: bool,
    enlarged_once: bool,
    opened_at_ms: u64,
    /// Sticky: hidden or unfocused. Next Alt+q must keep the newcomer.
    suppressed: bool,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        let ids = get_plugin_ids();
        self.own_plugin_id = Some(ids.plugin_id);
        self.client_id = Some(ids.client_id);
        self.opened_at_ms = now_ms();
        self.float_size = float_size_from_config(&configuration);
        subscribe(&[
            EventType::Key,
            EventType::SessionUpdate,
            EventType::PaneUpdate,
            EventType::PermissionRequestResult,
            EventType::RunCommandResult,
            EventType::Timer,
            EventType::Visible,
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
                    // Same order as overview: toggle, enlarge, then host work.
                    self.close_if_duplicate();
                    self.enlarge_if_floating();
                    self.remember_sessions(&fetch_live_sessions());
                    self.request_scan();
                    // One short tick so the first enlarge can run after PaneUpdate.
                    set_timeout(0.05);
                }
                true
            }
            Event::Timer(_) => {
                self.board.tick();
                if self.within_open_enlarge_window() && !self.enlarged_once {
                    self.enlarge_pending = false;
                    self.enlarge_issued = false;
                    self.enlarge_if_floating();
                }
                if !self.suppressed {
                    self.request_scan();
                }
                set_timeout(1.0);
                true
            }
            Event::Visible(false) => {
                if !self.is_young() {
                    self.suppressed = true;
                }
                false
            }
            Event::SessionUpdate(sessions, _) => {
                if let Some(session) = sessions.iter().find(|session| session.is_current_session) {
                    self.current_session = Some(session.name.clone());
                    self.floating_layer
                        .capture(self.previous_pane_was_floating(session));
                    self.mark_suppressed_if_layer_hidden(session);
                }
                self.remember_sessions(&sessions);
                // Scan on open + timer. SessionUpdate storms on LaunchPlugin
                // and each scan is pgrep/lsof across every agent.
                true
            }
            Event::PaneUpdate(manifest) => {
                self.pane_manifest = Some(manifest);
                self.mark_suppressed_if_unfocused();
                self.close_if_duplicate();
                self.enlarge_if_floating();
                if let Some(session) = self.current_session.clone() {
                    if let Some(manifest) = self.pane_manifest.as_ref() {
                        self.remember_places(places_from_manifest(
                            &session,
                            manifest,
                            &self.tab_names,
                        ));
                    }
                }
                // overview returns false here; we still need a paint after places.
                true
            }
            Event::RunCommandResult(_code, stdout, _stderr, context) => {
                if context.get("zellij_agent_board").map(String::as_str) != Some("scan") {
                    return false;
                }
                self.scan_inflight = false;
                let text = String::from_utf8_lossy(&stdout);
                self.board.ingest(&text);
                self.push_places();
                true
            }
            Event::Key(key) => {
                let Some(mapped) = map_key(key, self.board.is_hinting()) else {
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
            if viewport_is_large(rows, cols) {
                self.enlarged_once = true;
            }
        } else if viewport_is_large(rows, cols) {
            self.enlarged_once = true;
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
        // Paint as soon as we have permission. overview waits for enlarge and
        // can stay a title strip; we still grow once, but do not skip the frame.
        self.permissions_granted && self.own_plugin_id.is_some()
    }

    fn within_open_enlarge_window(&self) -> bool {
        !self.enlarged_once
            && self.opened_at_ms > 0
            && now_ms().saturating_sub(self.opened_at_ms) < OPEN_ENLARGE_RETRY_MS
    }

    /// Copied from overview: second LaunchPlugin opens a sibling; oldest closes all.
    /// A leftover that was hidden only closes itself so the newcomer can stay.
    fn close_if_duplicate(&mut self) {
        if !self.permissions_granted || !self.own_pane_is_listed() {
            return;
        }
        let Some(manifest) = self.pane_manifest.as_ref() else {
            return;
        };
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let panes = manifest.panes.values().flatten().collect::<Vec<_>>();
        let Some(own_url) = panes
            .iter()
            .find(|pane| pane.is_plugin && pane.id == own_id)
            .and_then(|pane| pane.plugin_url.as_deref())
        else {
            return;
        };
        let board_ids: Vec<u32> = panes
            .iter()
            .filter(|pane| pane.is_plugin && same_board_plugin(pane.plugin_url.as_deref(), own_url))
            .map(|pane| pane.id)
            .collect();
        let close_ids = duplicate_close_ids_with_focus(
            own_id,
            &board_ids,
            self.opened_at_ms,
            now_ms(),
            self.suppressed,
        );
        if close_ids.is_empty() {
            return;
        }
        if closes_the_board(&close_ids, &board_ids) {
            self.suppressed = true;
            self.restore_floating_layer();
        }
        for id in close_ids {
            close_pane_with_id(PaneId::Plugin(id));
        }
    }

    fn is_young(&self) -> bool {
        self.opened_at_ms > 0 && now_ms().saturating_sub(self.opened_at_ms) < TOGGLE_DEBOUNCE_MS
    }

    fn mark_suppressed_if_unfocused(&mut self) {
        if self.is_young() {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let Some(manifest) = self.pane_manifest.as_ref() else {
            return;
        };
        if manifest
            .panes
            .values()
            .flatten()
            .any(|pane| pane.is_plugin && pane.id == own_id && !pane.is_focused)
        {
            self.suppressed = true;
        }
    }

    /// Hide leaves the leftover focused; unfocused/Visible often never fire.
    /// The tab that owns this pane is the only float-layer flag we can trust.
    fn mark_suppressed_if_layer_hidden(&mut self, session: &SessionInfo) {
        if self.is_young() {
            return;
        }
        let Some(own_id) = self.own_plugin_id else {
            return;
        };
        let Some(tab_pos) = session.panes.panes.iter().find_map(|(&tab, panes)| {
            panes
                .iter()
                .any(|pane| pane.is_plugin && pane.id == own_id)
                .then_some(tab)
        }) else {
            return;
        };
        if session
            .tabs
            .iter()
            .any(|tab| tab.position == tab_pos && !tab.are_floating_panes_visible)
        {
            self.suppressed = true;
        }
    }

    fn own_pane_is_listed(&self) -> bool {
        let Some(own_id) = self.own_plugin_id else {
            return false;
        };
        self.pane_manifest.as_ref().is_some_and(|manifest| {
            manifest
                .panes
                .values()
                .flatten()
                .any(|pane| pane.is_plugin && pane.id == own_id)
        })
    }

    fn restore_floating_layer(&self) {
        if self.floating_layer.should_hide_on_close() {
            let _ = hide_floating_panes(None);
        }
    }

    fn enlarge_if_floating(&mut self) {
        if self.suppressed
            || self.enlarged_once
            || self.enlarge_pending
            || self.enlarge_issued
            || !self.permissions_granted
        {
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
        self.enlarge_issued = true;
        change_floating_panes_coordinates(vec![(PaneId::Plugin(own_id), coords)]);
    }

    fn request_scan(&mut self) {
        if self.suppressed || !self.permissions_granted || self.scan_inflight {
            return;
        }
        self.scan_inflight = true;
        let mut context = BTreeMap::new();
        context.insert("zellij_agent_board".to_string(), "scan".to_string());
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

    fn apply(&mut self, action: Action) -> bool {
        match action {
            Action::Dismiss => {
                self.dismiss();
                false
            }
            Action::Jump { session, pane_id } => {
                // Same as overview: dismiss, then move. overview uses go_to_tab;
                // we focus the agent pane instead.
                self.dismiss();
                if self.current_session.as_deref() == Some(session.as_str()) {
                    focus_terminal_pane(pane_id, false, false);
                } else {
                    switch_session_with_focus(&session, None, Some((pane_id, false)));
                }
                false
            }
            Action::None => true,
        }
    }

    fn dismiss(&mut self) {
        // Close only. hide_floating_panes before close_self leaves a leftover
        // that the next Alt+q treats as a toggle (looks like it did not open).
        // If close is dropped, this instance stays leftover and must not toggle.
        self.suppressed = true;
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

fn viewport_is_large(rows: usize, cols: usize) -> bool {
    rows >= ENLARGE_MIN_ROWS && cols >= ENLARGE_MIN_COLS
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

fn map_key(key: KeyWithModifier, hinting: bool) -> Option<Key> {
    // Do not map Alt+q here. overview leaves Ctrl+y to LaunchPlugin + close_if_duplicate;
    // handling the open key inside the plugin races with LaunchPlugin and reopens.
    if !key.has_no_modifiers() {
        return None;
    }
    match key.bare_key {
        BareKey::Esc | BareKey::Char('q') => Some(Key::Dismiss),
        BareKey::Char('?') if !hinting => Some(Key::ToggleHelp),
        BareKey::Backspace if hinting => Some(Key::Backspace),
        BareKey::Char('s') if !hinting => Some(Key::StartHint),
        BareKey::Char(ch) if hinting => Some(Key::Input(ch)),
        BareKey::Up | BareKey::Char('k') | BareKey::Left | BareKey::Char('h') if !hinting => {
            Some(Key::Up)
        }
        BareKey::Down | BareKey::Char('j') | BareKey::Right | BareKey::Char('l') if !hinting => {
            Some(Key::Down)
        }
        BareKey::Enter | BareKey::Char('e') => Some(Key::Confirm),
        _ => None,
    }
}

/// Same plugin identity as overview's exact URL match, plus wasm basename so
/// `file:/…` vs `file://…` still toggle together.
fn same_board_plugin(url: Option<&str>, own_url: &str) -> bool {
    let Some(url) = url else {
        return false;
    };
    if url == own_url {
        return true;
    }
    wasm_basename(url) == Some("zellij-agent-board.wasm")
        && wasm_basename(own_url) == Some("zellij-agent-board.wasm")
}

fn wasm_basename(url: &str) -> Option<&str> {
    url.rsplit('/')
        .next()
        .filter(|name| name.ends_with(".wasm"))
}
