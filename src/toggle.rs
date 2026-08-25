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
