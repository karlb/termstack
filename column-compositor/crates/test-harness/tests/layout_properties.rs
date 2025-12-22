//! Property-based tests for layout invariants
//!
//! These tests use the real ColumnLayout::calculate_from_heights function,
//! not a mock reimplementation.

use compositor::layout::ColumnLayout;
use proptest::prelude::*;

proptest! {
    /// Windows never overlap in column layout
    #[test]
    fn windows_never_overlap(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll in 0f64..1000.0,
    ) {
        let layout = ColumnLayout::calculate_from_heights(heights, 720, scroll);

        for i in 1..layout.window_positions.len() {
            let prev = &layout.window_positions[i - 1];
            let curr = &layout.window_positions[i];

            // Previous window's bottom should equal current window's top
            let prev_bottom = prev.y + prev.height as i32;
            prop_assert_eq!(prev_bottom, curr.y);
        }
    }

    /// Total height is sum of window heights
    #[test]
    fn total_height_is_sum_of_windows(
        heights in prop::collection::vec(1u32..500, 1..10),
    ) {
        let expected_total: u32 = heights.iter().sum();
        let layout = ColumnLayout::calculate_from_heights(heights, 720, 0.0);

        prop_assert_eq!(layout.total_height, expected_total);
    }

    /// Window positions are deterministic
    #[test]
    fn layout_is_deterministic(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll in 0f64..1000.0,
    ) {
        let layout1 = ColumnLayout::calculate_from_heights(heights.clone(), 720, scroll);
        let layout2 = ColumnLayout::calculate_from_heights(heights, 720, scroll);

        prop_assert_eq!(layout1.window_positions.len(), layout2.window_positions.len());
        prop_assert_eq!(layout1.total_height, layout2.total_height);

        for (p1, p2) in layout1.window_positions.iter().zip(layout2.window_positions.iter()) {
            prop_assert_eq!(p1.y, p2.y);
            prop_assert_eq!(p1.height, p2.height);
            prop_assert_eq!(p1.visible, p2.visible);
        }
    }

    /// Scroll offset only affects y positions, not heights
    #[test]
    fn scroll_only_affects_y(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll1 in 0f64..1000.0,
        scroll2 in 0f64..1000.0,
    ) {
        let layout1 = ColumnLayout::calculate_from_heights(heights.clone(), 720, scroll1);
        let layout2 = ColumnLayout::calculate_from_heights(heights, 720, scroll2);

        for (p1, p2) in layout1.window_positions.iter().zip(layout2.window_positions.iter()) {
            // Heights should be the same regardless of scroll
            prop_assert_eq!(p1.height, p2.height);

            // Y difference should match scroll difference
            // Note: must truncate each scroll separately (as layout does), not the difference
            let y_diff = p1.y - p2.y;
            let scroll_diff = scroll2 as i32 - scroll1 as i32;
            prop_assert_eq!(y_diff, scroll_diff);
        }

        // Total height unchanged
        prop_assert_eq!(layout1.total_height, layout2.total_height);
    }

    /// Empty window list produces empty layout
    #[test]
    fn empty_windows_empty_layout(scroll in 0f64..1000.0) {
        let layout = ColumnLayout::calculate_from_heights(std::iter::empty(), 720, scroll);
        prop_assert!(layout.window_positions.is_empty());
        prop_assert_eq!(layout.total_height, 0);
    }

    /// Visibility is correctly computed for windows in viewport
    #[test]
    fn visibility_correct(
        heights in prop::collection::vec(100u32..300, 2..8),
        output_height in 400u32..1000,
        scroll in 0f64..500.0,
    ) {
        let layout = ColumnLayout::calculate_from_heights(heights, output_height, scroll);

        for pos in &layout.window_positions {
            let window_bottom = pos.y + pos.height as i32;
            let on_screen = pos.y < output_height as i32 && window_bottom > 0;
            prop_assert_eq!(pos.visible, on_screen);
        }
    }

    /// Invariants always hold
    #[test]
    fn invariants_always_hold(
        heights in prop::collection::vec(1u32..500, 0..10),
        scroll in 0f64..1000.0,
    ) {
        let layout = ColumnLayout::calculate_from_heights(heights, 720, scroll);

        // check_invariants should pass
        prop_assert!(layout.check_invariants().is_ok());
    }

    /// Visible range tracks viewport position
    #[test]
    fn visible_range_correct(
        heights in prop::collection::vec(100u32..300, 1..5),
        output_height in 400u32..1000,
        scroll in 0f64..500.0,
    ) {
        let layout = ColumnLayout::calculate_from_heights(heights, output_height, scroll);

        let expected_start = scroll as u32;
        let expected_end = scroll as u32 + output_height;

        prop_assert_eq!(layout.visible_range.start, expected_start);
        prop_assert_eq!(layout.visible_range.end, expected_end);
    }
}
