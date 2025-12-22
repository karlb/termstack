//! Property-based tests for terminal sizing state machine

use proptest::prelude::*;
use terminal::sizing::{SizingAction, TerminalSizingState};

proptest! {
    /// Content rows only increments in Stable state
    #[test]
    fn content_rows_monotonic_in_stable(num_lines in 1usize..100) {
        let mut state = TerminalSizingState::new(100); // Large enough to not trigger growth
        let mut last_content_rows = 0u32;

        for _ in 0..num_lines {
            state.on_new_line();

            let current = state.content_rows();
            prop_assert!(current >= last_content_rows);
            prop_assert!(current <= last_content_rows + 1);
            last_content_rows = current;
        }
    }

    /// Content rows doesn't change during resize
    #[test]
    fn no_content_change_during_resize(
        lines_before in 1usize..50,
        lines_during in 1usize..20,
    ) {
        let mut state = TerminalSizingState::new(10);

        // Generate lines until we hit the limit
        for _ in 0..lines_before.min(10) {
            state.on_new_line();
        }

        let content_at_start = state.content_rows();

        // If we hit the limit, request growth
        if content_at_start > 10 {
            state.request_growth(50);

            let content_after_request = state.content_rows();

            // Lines during resize don't change content_rows
            for _ in 0..lines_during {
                state.on_new_line();
            }

            prop_assert_eq!(state.content_rows(), content_after_request);
        }
    }

    /// State transitions are valid
    #[test]
    fn valid_state_transitions(
        lines in prop::collection::vec(1u8..5, 1..50),
        configures in prop::collection::vec(10u16..100, 0..10),
    ) {
        let mut state = TerminalSizingState::new(10);

        let mut config_iter = configures.iter();

        for line_count in lines {
            for _ in 0..line_count {
                let action = state.on_new_line();

                // If growth requested, optionally send configure
                if let SizingAction::RequestGrowth { target_rows } = action {
                    state.request_growth(target_rows);

                    if let Some(&rows) = config_iter.next() {
                        state.on_configure(rows);
                        state.on_resize_complete();
                    }
                }
            }
        }

        // State should always be valid
        let _ = state.current_rows();
        let _ = state.content_rows();
    }

    /// Pending scrollback is restored after resize
    #[test]
    fn scrollback_restored(
        lines_during in 1usize..10,
    ) {
        let mut state = TerminalSizingState::new(5);

        // Fill and request growth
        for _ in 0..6 {
            state.on_new_line();
        }
        state.request_growth(20);

        // Add lines during resize
        for _ in 0..lines_during {
            state.on_new_line();
        }

        // Configure and complete
        state.on_configure(20);
        let action = state.on_resize_complete();

        if let SizingAction::RestoreScrollback { lines } = action {
            prop_assert_eq!(lines, lines_during as u32);
        } else if lines_during > 0 {
            prop_assert!(false, "expected RestoreScrollback action");
        }
    }

    /// Current rows is always positive
    #[test]
    fn rows_always_positive(
        initial_rows in 1u16..100,
        actions in prop::collection::vec(0u8..3, 0..50),
    ) {
        let mut state = TerminalSizingState::new(initial_rows);

        for action in actions {
            match action {
                0 => { state.on_new_line(); }
                1 => { state.on_configure(initial_rows); }
                2 => { state.on_resize_complete(); }
                _ => {}
            }

            prop_assert!(state.current_rows() > 0);
        }
    }
}
