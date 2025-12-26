//! Property-based tests for terminal sizing state machine

use proptest::prelude::*;
use terminal::sizing::{SizingAction, TerminalSizingState};
use terminal::Terminal;

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

    /// Alternate screen output never inflates content_rows
    ///
    /// TUI apps (vim, fzf, mc) use alternate screen mode. Any lines they print
    /// in that mode should NOT increase content_rows, as they're temporary
    /// full-screen UI that disappears when the app exits.
    #[test]
    fn alternate_screen_never_inflates_content(
        pre_alt_lines in 1u32..20,
        alt_screen_lines in 1u32..100,
        post_alt_lines in 1u32..20,
    ) {
        let mut terminal = Terminal::new(80, 100).expect("spawn terminal");

        // Phase 1: Pre-alternate screen - normal shell output
        for _ in 0..pre_alt_lines {
            terminal.inject_bytes(b"line\n");
        }
        let content_before_alt = terminal.content_rows();

        // Verify we're NOT in alternate screen yet
        prop_assert!(!terminal.is_alternate_screen());

        // Phase 2: Enter alternate screen (TUI app starts)
        terminal.inject_bytes(b"\x1b[?1049h");
        prop_assert!(terminal.is_alternate_screen());
        let content_at_alt_entry = terminal.content_rows();

        // Phase 2b: Output lots of lines in alternate screen (TUI drawing)
        for i in 0..alt_screen_lines {
            terminal.inject_bytes(format!("tui line {}\n", i).as_bytes());
        }
        let content_during_alt = terminal.content_rows();

        // CRITICAL: Content rows should NOT have increased during alternate screen
        prop_assert_eq!(
            content_during_alt,
            content_at_alt_entry,
            "content_rows should not increase during alternate screen"
        );

        // Phase 3: Exit alternate screen (TUI app ends)
        terminal.inject_bytes(b"\x1b[?1049l");
        prop_assert!(!terminal.is_alternate_screen());
        let content_after_alt = terminal.content_rows();

        // Content should be same as before entering alt screen
        prop_assert_eq!(
            content_after_alt,
            content_at_alt_entry,
            "content_rows should be preserved through alt screen cycle"
        );

        // Phase 4: Post-alternate screen - back to normal shell
        for _ in 0..post_alt_lines {
            terminal.inject_bytes(b"post line\n");
        }
        let final_content = terminal.content_rows();

        // Final content should be pre_alt_lines + post_alt_lines (not alt_screen_lines!)
        prop_assert!(
            final_content <= content_before_alt + post_alt_lines,
            "final content ({}) should be <= pre ({}) + post ({})",
            final_content, content_before_alt, post_alt_lines
        );
    }
}
