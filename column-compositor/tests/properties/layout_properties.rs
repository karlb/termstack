//! Property-based tests for layout invariants

use proptest::prelude::*;

// Note: These tests use mock data since we can't easily create WindowEntry instances
// in property tests. The actual layout module has its own tests.

#[derive(Debug, Clone)]
struct MockWindowPosition {
    y: i32,
    height: u32,
}

fn calculate_mock_layout(heights: &[u32], output_height: u32, scroll_offset: f64) -> Vec<MockWindowPosition> {
    let mut y_accumulator: i32 = 0;
    let mut positions = Vec::with_capacity(heights.len());

    for &height in heights {
        let y = y_accumulator - scroll_offset as i32;
        positions.push(MockWindowPosition { y, height });
        y_accumulator += height as i32;
    }

    positions
}

proptest! {
    /// Windows never overlap in column layout
    #[test]
    fn windows_never_overlap(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll in 0f64..1000.0,
    ) {
        let positions = calculate_mock_layout(&heights, 720, scroll);

        for i in 1..positions.len() {
            let prev = &positions[i - 1];
            let curr = &positions[i];

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
        let total: u32 = heights.iter().sum();
        let positions = calculate_mock_layout(&heights, 720, 0.0);

        if let Some(last) = positions.last() {
            let calculated_total = (last.y + last.height as i32) as u32;
            prop_assert_eq!(calculated_total, total);
        }
    }

    /// Window positions are deterministic
    #[test]
    fn layout_is_deterministic(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll in 0f64..1000.0,
    ) {
        let positions1 = calculate_mock_layout(&heights, 720, scroll);
        let positions2 = calculate_mock_layout(&heights, 720, scroll);

        prop_assert_eq!(positions1.len(), positions2.len());

        for (p1, p2) in positions1.iter().zip(positions2.iter()) {
            prop_assert_eq!(p1.y, p2.y);
            prop_assert_eq!(p1.height, p2.height);
        }
    }

    /// Scroll offset only affects y positions, not heights
    #[test]
    fn scroll_only_affects_y(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll1 in 0f64..1000.0,
        scroll2 in 0f64..1000.0,
    ) {
        let positions1 = calculate_mock_layout(&heights, 720, scroll1);
        let positions2 = calculate_mock_layout(&heights, 720, scroll2);

        for (p1, p2) in positions1.iter().zip(positions2.iter()) {
            // Heights should be the same regardless of scroll
            prop_assert_eq!(p1.height, p2.height);

            // Y difference should match scroll difference
            let y_diff = p1.y - p2.y;
            let scroll_diff = (scroll2 - scroll1) as i32;
            prop_assert_eq!(y_diff, scroll_diff);
        }
    }

    /// Empty window list produces empty layout
    #[test]
    fn empty_windows_empty_layout(_scroll in 0f64..1000.0) {
        let positions = calculate_mock_layout(&[], 720, _scroll);
        prop_assert!(positions.is_empty());
    }
}
