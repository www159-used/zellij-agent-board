//! LaunchPlugin toggle: a second instance means "close", but key-repeat
//! must not count as a second press.

/// Ignore sibling launches this long after open (macOS key-repeat delay).
pub const TOGGLE_DEBOUNCE_MS: u64 = 500;

/// Which plugin pane ids to close when more than one board is visible.
///
/// `own_id` is the instance making the decision. The oldest instance owns
/// toggle; a young oldest only drops newcomers (held Alt+q). After the
/// debounce, the oldest closes everyone. Newcomers only close themselves so
/// a missed PaneUpdate on the oldest cannot stack windows.
pub fn duplicate_close_ids(
    own_id: u32,
    board_ids: &[u32],
    opened_at_ms: u64,
    now_ms: u64,
) -> Vec<u32> {
    if board_ids.len() <= 1 || !board_ids.contains(&own_id) {
        return Vec::new();
    }
    let oldest = board_ids.iter().copied().min().expect("non-empty");
    let have_clock = opened_at_ms > 0 && now_ms > 0;
    let young = have_clock && now_ms.saturating_sub(opened_at_ms) < TOGGLE_DEBOUNCE_MS;

    if own_id != oldest {
        return vec![own_id];
    }
    if young {
        return board_ids
            .iter()
            .copied()
            .filter(|id| *id != own_id)
            .collect();
    }
    board_ids.to_vec()
}

/// Hide the floating layer only when the surviving board is going away.
/// A key-repeat newcomer closing itself must not hide the oldest pane.
pub fn closes_the_board(close_ids: &[u32], plugin_ids: &[u32]) -> bool {
    plugin_ids
        .iter()
        .copied()
        .min()
        .is_some_and(|oldest| close_ids.contains(&oldest))
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
        assert_eq!(duplicate_close_ids(9, &[3, 9], opened, now), vec![9]);
    }

    #[test]
    fn later_press_closes_everyone() {
        let opened = 1_000;
        let now = opened + TOGGLE_DEBOUNCE_MS;
        assert_eq!(duplicate_close_ids(3, &[3, 9], opened, now), vec![3, 9]);
        assert_eq!(duplicate_close_ids(9, &[3, 9], opened, now), vec![9]);
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
}
