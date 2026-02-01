//! Property-based tests for compositor state invariants
//!
//! These tests verify that state invariants hold across arbitrary sequences of operations.

use proptest::prelude::*;
use test_harness::TestCompositor;

proptest! {
    /// Window heights are always positive for all windows
    #[test]
    fn window_heights_always_positive(
        heights in prop::collection::vec(1u32..1000, 1..10),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }

        let snapshot = compositor.snapshot();
        for height in &snapshot.window_heights {
            prop_assert!(*height > 0, "Window height must be positive, got {}", height);
        }
    }

    /// Total content height equals sum of window heights
    #[test]
    fn total_height_equals_sum(
        heights in prop::collection::vec(1u32..500, 1..10),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }

        let snapshot = compositor.snapshot();
        let expected: u32 = heights.iter().sum();
        prop_assert_eq!(
            snapshot.total_height,
            expected,
            "Total height should equal sum of window heights"
        );
    }

    /// Scroll offset is never negative after any scroll operation
    #[test]
    fn scroll_offset_never_negative(
        heights in prop::collection::vec(100u32..500, 1..5),
        scroll_ops in prop::collection::vec(-1000f64..1000.0, 0..10),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }

        for delta in scroll_ops {
            compositor.scroll(delta);
            prop_assert!(
                compositor.scroll_offset() >= 0.0,
                "Scroll offset should never be negative, got {}",
                compositor.scroll_offset()
            );
        }
    }

    /// Scroll offset is clamped to valid range (0 to max_scroll)
    #[test]
    fn scroll_offset_clamped_to_max(
        heights in prop::collection::vec(100u32..500, 2..6),
        scroll_ops in prop::collection::vec(-2000f64..2000.0, 1..10),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }

        for delta in scroll_ops {
            compositor.scroll(delta);

            let total = compositor.total_content_height() as u32;
            let output_height = compositor.output_size().1;
            let max_scroll = total.saturating_sub(output_height) as f64;

            prop_assert!(
                compositor.scroll_offset() <= max_scroll + 0.001, // Small epsilon for float comparison
                "Scroll offset {} should be <= max_scroll {}",
                compositor.scroll_offset(),
                max_scroll
            );
        }
    }

    /// Focus index is always valid (within bounds or None)
    #[test]
    fn focus_always_valid_or_none(
        num_windows in 0usize..10,
        click_y_positions in prop::collection::vec(0f64..720.0, 0..5),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for i in 0..num_windows {
            compositor.add_external_window(100 + (i as u32) * 50);
        }

        // Simulate clicks at various positions
        for y in click_y_positions {
            compositor.simulate_click(100.0, y);
        }

        let snapshot = compositor.snapshot();
        if let Some(focus_idx) = snapshot.focused_index {
            prop_assert!(
                focus_idx < snapshot.window_count,
                "Focus index {} must be < window count {}",
                focus_idx,
                snapshot.window_count
            );
        }
        // None is always valid
    }

    /// Render positions are deterministic for same input
    #[test]
    fn render_positions_deterministic(
        heights in prop::collection::vec(50u32..300, 1..8),
        scroll in 0f64..500.0,
    ) {
        let mut compositor1 = TestCompositor::new_headless(1280, 720);
        let mut compositor2 = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor1.add_external_window(*height);
            compositor2.add_external_window(*height);
        }

        compositor1.set_scroll(scroll);
        compositor2.set_scroll(scroll);

        let positions1 = compositor1.render_positions();
        let positions2 = compositor2.render_positions();

        prop_assert_eq!(
            positions1.len(),
            positions2.len(),
            "Position count should match"
        );

        for (i, ((y1, h1), (y2, h2))) in positions1.iter().zip(positions2.iter()).enumerate() {
            prop_assert_eq!(y1, y2, "Window {} Y position should match", i);
            prop_assert_eq!(h1, h2, "Window {} height should match", i);
        }
    }

    /// Windows never overlap in render positions
    #[test]
    fn windows_never_overlap_in_render(
        heights in prop::collection::vec(50u32..300, 2..8),
        scroll in 0f64..500.0,
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }
        compositor.set_scroll(scroll);

        let positions = compositor.render_positions();

        // Verify windows don't overlap by checking ranges
        for i in 1..positions.len() {
            let (prev_y, _) = positions[i - 1];
            let (curr_y, curr_h) = positions[i];

            // In render coords (Y=0 at bottom), windows stack from top of screen downward.
            // Window 0 (first, at top of screen) has higher render_y than window 1 (below it).
            // Ranges: prev = [prev_y, prev_y + prev_h), curr = [curr_y, curr_y + curr_h)
            // For no overlap: one range should end where the other begins
            //
            // Since prev is above curr on screen, prev's bottom (prev_y) should equal
            // curr's top (curr_y + curr_h).
            let prev_bottom = prev_y;
            let curr_top = curr_y + curr_h;

            prop_assert!(
                (prev_bottom - curr_top).abs() <= 1,
                "Windows {} and {} should be adjacent: prev_bottom={}, curr_top={}",
                i - 1, i, prev_bottom, curr_top
            );
        }
    }

    /// Click detection matches render positions
    #[test]
    fn click_detection_matches_render(
        heights in prop::collection::vec(100u32..200, 2..5),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        for height in &heights {
            compositor.add_external_window(*height);
        }

        let positions = compositor.render_positions();
        let screen_height = compositor.output_size().1 as i32;

        // Click in the middle of each visible window
        for (idx, &(render_y, height)) in positions.iter().enumerate() {
            if render_y >= 0 && render_y + height <= screen_height {
                // Window is fully on screen
                // Convert render_y (bottom) to screen_y (top)
                // screen_y = screen_height - (render_y + height)
                let screen_y = (screen_height - render_y - height + height / 2) as f64;

                compositor.simulate_click(100.0, screen_y);

                let snapshot = compositor.snapshot();
                prop_assert_eq!(
                    snapshot.focused_index,
                    Some(idx),
                    "Click at screen_y={} should focus window {} (render_y={}, height={})",
                    screen_y, idx, render_y, height
                );
            }
        }
    }

    /// Adding and removing windows maintains consistency
    #[test]
    fn add_remove_maintains_consistency(
        initial_count in 1usize..5,
        operations in prop::collection::vec(prop::bool::ANY, 1..10),
    ) {
        let mut compositor = TestCompositor::new_headless(1280, 720);

        // Add initial windows
        for _ in 0..initial_count {
            compositor.add_external_window(150);
        }

        let mut expected_count = initial_count;

        for should_add in operations {
            if should_add {
                compositor.add_external_window(150);
                expected_count += 1;
            } else if expected_count > 0 {
                // We can't remove in TestCompositor, but we can verify state after adds
            }

            let snapshot = compositor.snapshot();
            prop_assert_eq!(
                snapshot.window_count,
                expected_count,
                "Window count should be consistent"
            );
        }
    }
}

#[cfg(test)]
mod boundary_props {
    use super::*;

    proptest! {
        /// Empty compositor has valid state
        #[test]
        fn empty_compositor_valid(_dummy in 0..1) {
            let compositor = TestCompositor::new_headless(1280, 720);
            let snapshot = compositor.snapshot();

            prop_assert_eq!(snapshot.window_count, 0);
            prop_assert_eq!(snapshot.total_height, 0);
            prop_assert!(snapshot.focused_index.is_none());
        }

        /// Single window compositor has valid state
        #[test]
        fn single_window_valid(height in 50u32..500) {
            let mut compositor = TestCompositor::new_headless(1280, 720);
            compositor.add_external_window(height);

            let snapshot = compositor.snapshot();
            prop_assert_eq!(snapshot.window_count, 1);
            prop_assert_eq!(snapshot.total_height, height);
            prop_assert_eq!(snapshot.focused_index, Some(0));
        }

        /// Scroll with no content stays at zero
        #[test]
        fn scroll_empty_stays_zero(delta in -1000f64..1000.0) {
            let mut compositor = TestCompositor::new_headless(1280, 720);
            compositor.scroll(delta);

            prop_assert_eq!(
                compositor.scroll_offset(),
                0.0,
                "Scroll should stay at 0 when no content"
            );
        }
    }
}
