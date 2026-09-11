//! LaunchPlugin toggle: a second instance means "close", but key-repeat
//! must not count as a second press.

/// Ignore sibling launches this long after open (macOS key-repeat delay).
pub const TOGGLE_DEBOUNCE_MS: u64 = 500;

/// Which plugin pane ids to close when more than one board is visible.
///
/// `own_id` is the instance making the decision. The oldest instance owns
/// toggle; a young oldest only drops newcomers (held Alt+q). After the
/// debounce, a live oldest closes everyone. A leftover that was hidden
/// only closes itself so the next Alt+q can open. Young newcomers stay
/// and let the oldest decide — if they also closed themselves, leftover
/// + new would both vanish (looks like the first open failed).
pub fn duplicate_close_ids(
    own_id: u32,
    board_ids: &[u32],
    opened_at_ms: u64,
    now_ms: u64,
) -> Vec<u32> {
    duplicate_close_ids_with_focus(own_id, board_ids, opened_at_ms, now_ms, false)
}

/// `leftover` is sticky: set when this instance was hidden or unfocused.
/// LaunchPlugin may show the leftover and steal focus; do not recompute
/// leftover from the current focus/float flags at sibling-detect time.
pub fn duplicate_close_ids_with_focus(
    own_id: u32,
    board_ids: &[u32],
    opened_at_ms: u64,
    now_ms: u64,
    leftover: bool,
) -> Vec<u32> {
    if board_ids.len() <= 1 || !board_ids.contains(&own_id) {
        return Vec::new();
    }
    let oldest = board_ids.iter().copied().min().expect("non-empty");
    let have_clock = opened_at_ms > 0 && now_ms > 0;
    let young = have_clock && now_ms.saturating_sub(opened_at_ms) < TOGGLE_DEBOUNCE_MS;

    if own_id != oldest {
        if young {
            return Vec::new();
        }
        return vec![own_id];
    }
    if young {
        return board_ids
            .iter()
            .copied()
            .filter(|id| *id != own_id)
            .collect();
    }
    if leftover {
        return vec![own_id];
    }
    board_ids.to_vec()
}

/// Hide the floating layer only when the surviving board is going away.
pub fn closes_the_board(close_ids: &[u32], plugin_ids: &[u32]) -> bool {
    !plugin_ids.is_empty() && plugin_ids.iter().all(|id| close_ids.contains(id))
}

/// What the WASM bridge should close when it sees other same-URL instances.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeClosePlan {
    None,
    /// Close these plugin panes; this instance stays (open path / key-repeat).
    Drop {
        ids: Vec<u32>,
    },
    /// Close the board: TUI plus every plugin id, including this one.
    Shutdown {
        ids: Vec<u32>,
    },
}

/// `hide_self` while the TUI is up is not a leftover to keep. A later Alt+q
/// must tear the board down (`close_self`), not spawn another suppressed orphan.
/// With no TUI, the newest instance owns the open and older (often suppressed)
/// bridges are dropped.
pub fn bridge_close_plan(
    own_id: u32,
    board_ids: &[u32],
    opened_at_ms: u64,
    now_ms: u64,
    tui_up: bool,
) -> BridgeClosePlan {
    if board_ids.len() <= 1 || !board_ids.contains(&own_id) {
        return BridgeClosePlan::None;
    }
    let newest = board_ids.iter().copied().max().unwrap_or(own_id);
    let others: Vec<u32> = board_ids
        .iter()
        .copied()
        .filter(|id| *id != own_id)
        .collect();
    let have_clock = opened_at_ms > 0 && now_ms > 0;
    let young = have_clock && now_ms.saturating_sub(opened_at_ms) < TOGGLE_DEBOUNCE_MS;

    if tui_up {
        if young {
            return BridgeClosePlan::Drop { ids: others };
        }
        return BridgeClosePlan::Shutdown {
            ids: board_ids.to_vec(),
        };
    }
    if own_id == newest {
        return if others.is_empty() {
            BridgeClosePlan::None
        } else {
            BridgeClosePlan::Drop { ids: others }
        };
    }
    if young {
        return BridgeClosePlan::None;
    }
    BridgeClosePlan::Drop { ids: vec![own_id] }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The plugin opens the host board with `zellij action new-pane … -- board-tui`.
/// That short-lived command pane must not be adopted as the TUI: when it
/// exits, `CommandPaneExited` looks like the user quit and the bridge
/// `close_self`s — Alt+q loads then dies in ~200ms.
pub fn looks_like_board_tui(command: Option<&str>, title: &str) -> bool {
    let command = command.unwrap_or("");
    if is_tui_launcher_command(command) {
        return false;
    }
    command_invokes_board_tui(command) || title.contains("board-tui")
}

pub fn is_tui_launcher_command(command: &str) -> bool {
    command.contains("zellij") && command.contains("new-pane")
}

fn command_invokes_board_tui(command: &str) -> bool {
    command
        .split_whitespace()
        .any(|token| token.rsplit('/').next() == Some("board-tui"))
}

/// The real board is a regular terminal. Command-pane exits are the
/// launcher / places / focus writers — never a user quit.
pub fn is_host_tui_exit(tui_id: Option<u32>, closed_id: u32, command_pane: bool) -> bool {
    !command_pane && tui_id == Some(closed_id)
}

/// The empty plugin is not the board. Open only after the current session
/// is known, once. Later `SessionUpdate`s / `PaneUpdate`s must not spawn
/// another pane; only the Timer retries a failed launch.
pub fn should_open_tui(
    permissions: bool,
    dying: bool,
    tui_up: bool,
    attempts: u8,
    from_timer: bool,
    session_known: bool,
) -> bool {
    if !permissions || dying || tui_up || !session_known {
        return false;
    }
    if attempts == 0 {
        return true;
    }
    from_timer && attempts < 3
}

/// Empty shell must not stay floating. After three failed launches, die.
pub fn should_abandon_empty_bridge(tui_up: bool, attempts: u8) -> bool {
    !tui_up && attempts >= 3
}

/// `hide_self` means the user saw the board. A pane that dies before that
/// is a failed launch, not `q`. Jump sets `dying` so `PaneClosed` does not
/// `close_self` before `switch_session`.
pub fn should_shutdown_on_tui_close(bridge_hidden: bool, dying: bool) -> bool {
    bridge_hidden && !dying
}

/// Tear the board down in the origin session, then move, then kill the
/// bridge. Never switch first — pane ids are only valid here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpStep {
    CloseOriginBoard,
    Focus { pane_id: u32 },
    Switch { session: String, pane_id: u32 },
    CloseBridge,
}

pub fn jump_steps(current: Option<&str>, target: &str, pane_id: u32) -> Vec<JumpStep> {
    let next = if current == Some(target) {
        JumpStep::Focus { pane_id }
    } else {
        JumpStep::Switch {
            session: target.to_string(),
            pane_id,
        }
    };
    vec![JumpStep::CloseOriginBoard, next, JumpStep::CloseBridge]
}

/// After `close_pane_with_id(own)`, `close_self()` is a second close of the
/// same plugin. The diagnostic flag drops only that extra close so a
/// control run can keep the current product sequence.
pub fn should_also_close_self(
    own_id: Option<u32>,
    plugin_ids: &[u32],
    skip_redundant: bool,
) -> bool {
    if !skip_redundant {
        return true;
    }
    !matches!(own_id, Some(id) if plugin_ids.contains(&id))
}

/// `close_pane_with_id(own)` plus `close_self()` is two close APIs for the
/// same pane. The diagnostic flag keeps `close_self` and only lists others
/// for the explicit close, so the origin pane still leaves the screen.
pub fn plugin_ids_to_close_explicitly(
    own_id: Option<u32>,
    plugin_ids: &[u32],
    skip_own_explicit: bool,
) -> Vec<u32> {
    if !skip_own_explicit {
        return plugin_ids.to_vec();
    }
    plugin_ids
        .iter()
        .copied()
        .filter(|id| Some(*id) != own_id)
        .collect()
}

/// `hide_self` suppresses the bridge. Later `ClosePane` then goes through
/// Zellij's replace-with-suppressed-pane path. The diagnostic flag keeps
/// the plugin visible so close is a normal pane close.
pub fn should_hide_bridge_keep_tui(already_hidden: bool, skip_hide: bool) -> bool {
    !already_hidden && !skip_hide
}

/// The diagnostic parking variant keeps the bridge as an ordinary floating
/// pane, avoiding `SuppressPane`, while making it non-visible to the user.
pub fn should_park_bridge_keep_tui(already_hidden: bool, park_bridge: bool) -> bool {
    !already_hidden && park_bridge
}

/// Keep `hide_self` while the board is up, but restore the plugin before
/// close so `ClosePane` does not go through replace-with-suppressed-pane.
pub fn should_unsuppress_before_close(enabled: bool, bridge_hidden: bool) -> bool {
    enabled && bridge_hidden
}

#[cfg(test)]
mod tests {
    use super::{closes_the_board, duplicate_close_ids, TOGGLE_DEBOUNCE_MS};

    #[test]
    fn one_instance_closes_nothing() {
        assert!(duplicate_close_ids(3, &[3], 1_000, 1_100).is_empty());
    }

    #[test]
    fn key_repeat_keeps_the_oldest() {
        let opened = 1_000;
        let now = opened + 80;
        assert!(now - opened < TOGGLE_DEBOUNCE_MS);
        assert_eq!(duplicate_close_ids(3, &[3, 9], opened, now), vec![9]);
        assert_eq!(
            duplicate_close_ids(9, &[3, 9], opened, now),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn later_press_closes_everyone() {
        let opened = 1_000;
        let now = opened + TOGGLE_DEBOUNCE_MS;
        assert_eq!(duplicate_close_ids(3, &[3, 9], opened, now), vec![3, 9]);
        assert_eq!(duplicate_close_ids(9, &[3, 9], opened, now), vec![9]);
    }

    #[test]
    fn hidden_leftover_lets_a_new_press_open() {
        use super::duplicate_close_ids_with_focus;
        let leftover_opened = 1_000;
        let now = leftover_opened + 5_000;
        assert_eq!(
            duplicate_close_ids_with_focus(3, &[3, 9], leftover_opened, now, true),
            vec![3]
        );
        assert_eq!(
            duplicate_close_ids_with_focus(9, &[3, 9], now, now + 20, false),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn live_oldest_still_toggles_after_debounce() {
        use super::duplicate_close_ids_with_focus;
        let opened = 1_000;
        let now = opened + TOGGLE_DEBOUNCE_MS;
        assert_eq!(
            duplicate_close_ids_with_focus(3, &[3, 9], opened, now, false),
            vec![3, 9]
        );
    }

    #[test]
    fn missing_clock_still_toggles() {
        assert_eq!(duplicate_close_ids(3, &[3, 9], 0, 0), vec![3, 9]);
    }

    #[test]
    fn only_a_full_close_hides_the_float() {
        assert!(!closes_the_board(&[9], &[3, 9]));
        assert!(closes_the_board(&[3, 9], &[3, 9]));
    }

    #[test]
    fn later_press_with_tui_up_shuts_the_board_down() {
        use super::{bridge_close_plan, BridgeClosePlan};
        let opened = 1_000;
        let now = opened + TOGGLE_DEBOUNCE_MS;
        assert_eq!(
            bridge_close_plan(3, &[3, 9], opened, now, true),
            BridgeClosePlan::Shutdown { ids: vec![3, 9] }
        );
    }

    #[test]
    fn key_repeat_with_tui_up_only_drops_newcomers() {
        use super::{bridge_close_plan, BridgeClosePlan};
        let opened = 1_000;
        assert_eq!(
            bridge_close_plan(3, &[3, 9], opened, opened + 80, true),
            BridgeClosePlan::Drop { ids: vec![9] }
        );
    }

    #[test]
    fn open_path_newest_drops_suppressed_older_bridges() {
        use super::{bridge_close_plan, BridgeClosePlan};
        assert_eq!(
            bridge_close_plan(9, &[3, 5, 9], 1_000, 2_000, false),
            BridgeClosePlan::Drop { ids: vec![3, 5] }
        );
        assert_eq!(
            bridge_close_plan(3, &[3, 5, 9], 1_000, 2_000, false),
            BridgeClosePlan::Drop { ids: vec![3] }
        );
    }

    #[test]
    fn launcher_command_is_not_the_host_tui() {
        use super::looks_like_board_tui;
        let launcher = "zellij --session zab action new-pane --floating --near-current-pane --close-on-exit --name board-tui --width 80% --height 80% --x 10% --y 10% -- /Users/ww/.config/zellij/plugins/board-tui";
        assert!(
            !looks_like_board_tui(Some(launcher), "board-tui"),
            "adopting the launcher makes CommandPaneExited look like q"
        );
        assert!(!looks_like_board_tui(
            Some("zellij action new-pane --floating -- board-tui"),
            "zellij"
        ));
    }

    #[test]
    fn real_board_tui_pane_is_recognized() {
        use super::looks_like_board_tui;
        assert!(looks_like_board_tui(
            Some("/Users/ww/.config/zellij/plugins/board-tui"),
            "board-tui"
        ));
        assert!(looks_like_board_tui(None, "board-tui"));
        assert!(!looks_like_board_tui(Some("/bin/bash"), "bash"));
    }

    #[test]
    fn command_pane_exit_is_never_a_user_quit() {
        use super::is_host_tui_exit;
        assert!(!is_host_tui_exit(Some(9), 9, true));
        assert!(is_host_tui_exit(Some(9), 9, false));
        assert!(!is_host_tui_exit(None, 9, false));
        assert!(!is_host_tui_exit(Some(9), 8, false));
    }

    #[test]
    fn first_open_is_once_and_later_updates_do_not_spawn() {
        use super::should_open_tui;
        assert!(should_open_tui(true, false, false, 0, false, true));
        assert!(!should_open_tui(true, false, false, 1, false, true));
        assert!(should_open_tui(true, false, false, 1, true, true));
        assert!(!should_open_tui(true, false, false, 3, true, true));
        assert!(!should_open_tui(true, false, true, 0, false, true));
        assert!(!should_open_tui(false, false, false, 0, false, true));
    }

    #[test]
    fn tui_does_not_open_before_the_session_is_known() {
        use super::should_open_tui;
        assert!(!should_open_tui(true, false, false, 0, false, false));
        assert!(should_open_tui(true, false, false, 0, false, true));
    }

    #[test]
    fn empty_bridge_dies_after_three_failed_launches() {
        use super::should_abandon_empty_bridge;
        assert!(!should_abandon_empty_bridge(false, 0));
        assert!(!should_abandon_empty_bridge(false, 2));
        assert!(should_abandon_empty_bridge(false, 3));
        assert!(!should_abandon_empty_bridge(true, 3));
    }

    #[test]
    fn failed_launch_does_not_quit_the_bridge() {
        use super::should_shutdown_on_tui_close;
        assert!(!should_shutdown_on_tui_close(false, false));
        assert!(should_shutdown_on_tui_close(true, false));
        assert!(!should_shutdown_on_tui_close(true, true));
    }

    #[test]
    fn jump_closes_the_origin_board_before_switching() {
        use super::{jump_steps, JumpStep};
        assert_eq!(
            jump_steps(Some("zab"), "lp", 8),
            vec![
                JumpStep::CloseOriginBoard,
                JumpStep::Switch {
                    session: "lp".into(),
                    pane_id: 8,
                },
                JumpStep::CloseBridge,
            ]
        );
        assert_eq!(
            jump_steps(Some("zab"), "zab", 3),
            vec![
                JumpStep::CloseOriginBoard,
                JumpStep::Focus { pane_id: 3 },
                JumpStep::CloseBridge,
            ]
        );
        assert_eq!(jump_steps(None, "lp", 1)[0], JumpStep::CloseOriginBoard);
    }

    #[test]
    fn current_close_always_also_closes_self() {
        use super::should_also_close_self;
        assert!(should_also_close_self(Some(3), &[3], false));
        assert!(should_also_close_self(Some(3), &[3, 9], false));
        assert!(should_also_close_self(None, &[3], false));
    }

    #[test]
    fn diagnostic_skips_self_close_when_own_id_already_closed() {
        use super::should_also_close_self;
        assert!(!should_also_close_self(Some(3), &[3], true));
        assert!(!should_also_close_self(Some(3), &[3, 9], true));
    }

    #[test]
    fn diagnostic_still_closes_self_when_own_id_was_not_listed() {
        use super::should_also_close_self;
        assert!(should_also_close_self(Some(3), &[9], true));
        assert!(should_also_close_self(None, &[9], true));
        assert!(should_also_close_self(Some(3), &[], true));
    }

    #[test]
    fn current_close_lists_every_plugin_id() {
        use super::plugin_ids_to_close_explicitly;
        assert_eq!(
            plugin_ids_to_close_explicitly(Some(3), &[3, 9], false),
            vec![3, 9]
        );
    }

    #[test]
    fn diagnostic_omits_own_id_from_the_explicit_close() {
        use super::plugin_ids_to_close_explicitly;
        assert_eq!(
            plugin_ids_to_close_explicitly(Some(3), &[3, 9], true),
            vec![9]
        );
        assert_eq!(
            plugin_ids_to_close_explicitly(Some(3), &[3], true),
            Vec::<u32>::new()
        );
        assert_eq!(plugin_ids_to_close_explicitly(None, &[3], true), vec![3]);
    }

    #[test]
    fn current_open_hides_the_bridge_once() {
        use super::should_hide_bridge_keep_tui;
        assert!(should_hide_bridge_keep_tui(false, false));
        assert!(!should_hide_bridge_keep_tui(true, false));
    }

    #[test]
    fn diagnostic_keeps_the_bridge_visible() {
        use super::should_hide_bridge_keep_tui;
        assert!(!should_hide_bridge_keep_tui(false, true));
        assert!(!should_hide_bridge_keep_tui(true, true));
    }

    #[test]
    fn diagnostic_parks_the_bridge_once_without_suppressing_it() {
        use super::should_park_bridge_keep_tui;
        assert!(should_park_bridge_keep_tui(false, true));
        assert!(!should_park_bridge_keep_tui(true, true));
        assert!(!should_park_bridge_keep_tui(false, false));
    }

    #[test]
    fn current_close_does_not_unsuppress_first() {
        use super::should_unsuppress_before_close;
        assert!(!should_unsuppress_before_close(false, true));
        assert!(!should_unsuppress_before_close(false, false));
    }

    #[test]
    fn diagnostic_unsuppresses_only_a_hidden_bridge() {
        use super::should_unsuppress_before_close;
        assert!(should_unsuppress_before_close(true, true));
        assert!(!should_unsuppress_before_close(true, false));
    }

    #[test]
    fn mixing_foreign_session_ids_shuts_a_new_board_down() {
        // Plugin ids are per-session. Harvest must only pass the current
        // session. If a leftover in ww (id 40) is mixed with a new board in
        // lp (id 2), Alt+q there looks like a second instance and dies.
        use super::{bridge_close_plan, BridgeClosePlan};
        let opened = 1_000;
        let now = opened + TOGGLE_DEBOUNCE_MS;
        assert_eq!(
            bridge_close_plan(2, &[2, 40], opened, now, true),
            BridgeClosePlan::Shutdown { ids: vec![2, 40] }
        );
    }
}
