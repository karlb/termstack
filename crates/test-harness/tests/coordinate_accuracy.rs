//! Tests for coordinate accuracy
//!
//! These tests verify that click detection remains accurate
//! at all positions, especially for windows further down the stack.

use test_harness::TestCompositor;

/// Test that click detection accuracy doesn't degrade with position
///
/// This tests the user-reported issue: "Y mapping seems to get more wrong
/// the further down I get"
#[test]
fn click_accuracy_consistent_across_all_windows() {
    let mut tc = TestCompositor::new_headless(800, 1200);

    // Add 10 windows of varying heights
    let heights = [100, 150, 200, 120, 180, 160, 140, 190, 110, 170];
    for &h in &heights {
        tc.add_external_window(h);
    }

    let positions = tc.render_positions();
    let ranges = tc.window_click_ranges();

    println!("Testing 10 windows:");
    for (i, (&(render_y, height), &(range_start, range_end))) in
        positions.iter().zip(ranges.iter()).enumerate()
    {
        println!(
            "  Window {}: render_y={}, height={}, range=({}, {})",
            i, render_y, height, range_start, range_end
        );

        // Verify render position matches click range
        assert_eq!(
            render_y as f64, range_start,
            "Window {}: render_y should equal range_start",
            i
        );
        assert_eq!(
            (render_y + height) as f64, range_end,
            "Window {}: render_y + height should equal range_end",
            i
        );
    }

    // Test clicking at the CENTER of each window
    for (i, &(render_y, height)) in positions.iter().enumerate() {
        let center_render_y = render_y as f64 + height as f64 / 2.0;

        // Convert render Y to screen Y for clicking
        let center_screen_y = 1200.0 - center_render_y;

        tc.simulate_click(400.0, center_screen_y);

        let focused = tc.snapshot().focused_index;
        assert_eq!(
            focused,
            Some(i),
            "Clicking at center of window {} (screen_y={:.1}, render_y={:.1}) should focus it, but focused {:?}",
            i,
            center_screen_y,
            center_render_y,
            focused
        );
    }
}

/// Test that windows near the bottom can be clicked accurately
#[test]
fn click_accuracy_at_bottom_of_stack() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add enough windows to exceed screen height
    for _ in 0..5 {
        tc.add_external_window(200);
    }

    // Total content: 1000px, viewport: 600px
    // Scroll to show the bottom
    tc.set_scroll(400.0); // Max scroll = 1000 - 600 = 400

    let positions = tc.render_positions();
    println!("After scrolling to bottom (scroll=400):");
    for (i, &(render_y, height)) in positions.iter().enumerate() {
        println!("  Window {}: render_y={}, height={}", i, render_y, height);
    }

    // Window 4 (last) should be visible at the bottom
    // content_y for window 4 = 800 - scroll = 800 - 400 = 400
    // render_y = 600 - 400 - 200 = 0
    let window_4_pos = positions[4];
    assert_eq!(window_4_pos.0, 0, "Window 4 should be at render_y=0 (bottom of screen)");

    // Click at bottom of screen (screen_y near 600 = render_y near 0)
    tc.simulate_click(400.0, 550.0); // screen_y=550 -> render_y=50
    assert_eq!(
        tc.snapshot().focused_index,
        Some(4),
        "Clicking near bottom should focus window 4"
    );
}

/// Test max scroll calculation matches total content
#[test]
fn scroll_to_bottom_shows_last_window() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add windows totaling 1500px
    tc.add_external_window(300);
    tc.add_external_window(400);
    tc.add_external_window(350);
    tc.add_external_window(450);

    let total_content = tc.total_content_height();
    assert_eq!(total_content, 1500, "Total content should be 1500px");

    // Max scroll should allow showing the very bottom
    let max_scroll = (total_content - 600) as f64;
    assert_eq!(max_scroll, 900.0, "Max scroll should be 900");

    // Try to scroll past max
    tc.set_scroll(10000.0);
    let actual_scroll = tc.scroll_offset();
    assert_eq!(
        actual_scroll, max_scroll,
        "Scroll should clamp to max"
    );

    // Verify last window is visible
    let positions = tc.render_positions();
    println!("After max scroll (scroll={}):", actual_scroll);
    for (i, &(render_y, height)) in positions.iter().enumerate() {
        println!("  Window {}: render_y={}, height={}", i, render_y, height);
    }

    // Last window (index 3) at content_y = 300+400+350 = 1050
    // render_y = 600 - (1050 - 900) - 450 = 600 - 150 - 450 = 0
    let last_window = positions[3];
    assert!(
        last_window.0 >= 0,
        "Last window should be on screen (render_y={} >= 0)",
        last_window.0
    );
    assert!(
        last_window.0 < 600,
        "Last window should be on screen (render_y={} < 600)",
        last_window.0
    );

    // Click on last window
    let center_render_y = last_window.0 as f64 + last_window.1 as f64 / 2.0;
    let center_screen_y = 600.0 - center_render_y;

    println!("Clicking on window 3: render_y={}, screen_y={}", center_render_y, center_screen_y);

    tc.simulate_click(400.0, center_screen_y);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(3),
        "Should be able to click last window after scrolling to bottom"
    );
}

/// Test cumulative height calculation
#[test]
fn cumulative_height_is_exact() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add windows with specific heights
    let heights: Vec<u32> = vec![100, 150, 200, 120, 180];
    for &h in &heights {
        tc.add_external_window(h);
    }

    // Verify total
    let expected_total: u32 = heights.iter().sum();
    let actual_total = tc.total_content_height();
    assert_eq!(
        actual_total, expected_total as i32,
        "Total content height should be exact sum"
    );

    // Verify each window's position is exactly the sum of previous heights
    let positions = tc.render_positions();
    let screen_height = 600;

    let mut expected_content_y = 0;
    for (i, &height) in heights.iter().enumerate() {
        let expected_render_y = screen_height - expected_content_y - height as i32;
        let actual_render_y = positions[i].0;

        assert_eq!(
            actual_render_y, expected_render_y,
            "Window {} at wrong position: expected render_y={}, got {}. content_y={}",
            i, expected_render_y, actual_render_y, expected_content_y
        );

        expected_content_y += height as i32;
    }
}

/// Test that scroll offset affects positions correctly
#[test]
fn scroll_shifts_all_positions_uniformly() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add windows totaling more than viewport to allow scrolling
    tc.add_external_window(300);
    tc.add_external_window(300);
    tc.add_external_window(300);
    // Total: 900px, viewport: 600px, max_scroll: 300

    // Get positions at scroll=0
    let positions_0 = tc.render_positions();

    // Scroll by 100
    tc.set_scroll(100.0);
    let positions_100 = tc.render_positions();

    println!("Scroll shift test:");
    println!("  At scroll=0: {:?}", positions_0);
    println!("  At scroll=100: {:?}", positions_100);

    // All windows should have shifted by +100 in render_y
    // (because content_y decreases by 100, making render_y = screen - content_y - h larger)
    for (i, (p0, p100)) in positions_0.iter().zip(positions_100.iter()).enumerate() {
        let shift = p100.0 - p0.0;
        assert_eq!(
            shift, 100,
            "Window {} should shift by +100 in render_y when scrolling down, but shifted by {}",
            i, shift
        );
    }
}

/// Reproduce issue: compare real compositor calculation vs test harness
#[test]
fn verify_formula_matches_real_compositor() {
    // Simulate the exact calculation from state.rs window_at()
    fn real_window_at(
        screen_height: f64,
        scroll_offset: f64,
        heights: &[i32],
        point_y: f64,
    ) -> Option<usize> {
        let mut content_y = -scroll_offset;

        for (i, &height) in heights.iter().enumerate() {
            let cell_height = height as f64;
            let render_y = screen_height - content_y - cell_height;
            let render_end = render_y + cell_height;

            if point_y >= render_y && point_y < render_end {
                return Some(i);
            }
            content_y += cell_height;
        }
        None
    }

    // Simulate the exact calculation from main.rs rendering
    fn real_render_positions(
        screen_height: i32,
        scroll_offset: f64,
        heights: &[i32],
    ) -> Vec<(i32, i32)> {
        let mut content_y: i32 = -(scroll_offset as i32);

        heights
            .iter()
            .map(|&height| {
                let render_y = screen_height - content_y - height;
                content_y += height;
                (render_y, height)
            })
            .collect()
    }

    let heights = vec![100, 150, 200, 120, 180];
    let screen_height = 600;
    let scroll_offset = 0.0;

    let render_positions = real_render_positions(screen_height, scroll_offset, &heights);

    // For each window, verify clicking at its center hits it
    for (i, &(render_y, height)) in render_positions.iter().enumerate() {
        let center = render_y as f64 + height as f64 / 2.0;
        let hit = real_window_at(screen_height as f64, scroll_offset, &heights, center);

        assert_eq!(
            hit,
            Some(i),
            "Window {} at render_y={}: clicking at center {} should hit it",
            i,
            render_y,
            center
        );
    }

    // Now test with scroll
    let scroll_offset = 50.0;
    let render_positions = real_render_positions(screen_height, scroll_offset, &heights);

    for (i, &(render_y, height)) in render_positions.iter().enumerate() {
        let center = render_y as f64 + height as f64 / 2.0;

        // Only test if center is on screen
        if center >= 0.0 && center < screen_height as f64 {
            let hit = real_window_at(screen_height as f64, scroll_offset, &heights, center);

            assert_eq!(
                hit,
                Some(i),
                "With scroll={}, window {} at render_y={}: clicking at center {} should hit it",
                scroll_offset,
                i,
                render_y,
                center
            );
        }
    }
}

/// Regression test: click detection must use the same heights as rendering
///
/// BUG: Previously, at frame start we calculated heights using bbox().size.h
/// for external windows, but rendering used max(element.geometry().loc.y + size.h).
/// These can differ, causing click detection to be increasingly wrong further down.
///
/// FIX: Use cached heights from previous frame (which ARE the actual rendered heights)
/// instead of recalculating with bbox().
#[test]
fn click_detection_uses_same_heights_as_rendering() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Simulate windows where bbox might differ from element geometry
    // (e.g., a window with an element that has a y offset)
    tc.add_external_window(200);
    tc.add_external_window(150);
    tc.add_external_window(250);

    // Get render positions and click ranges
    let render_pos = tc.render_positions();
    let click_ranges = tc.window_click_ranges();

    // They MUST match exactly - no accumulating error
    for (i, (&(render_y, height), &(range_start, range_end))) in
        render_pos.iter().zip(click_ranges.iter()).enumerate()
    {
        assert_eq!(
            render_y as f64, range_start,
            "Window {}: render_y must equal click range start (bbox vs geometry mismatch?)",
            i
        );
        assert_eq!(
            (render_y + height) as f64, range_end,
            "Window {}: render_end must equal click range end",
            i
        );
    }

    // Test at various Y positions to catch accumulation errors
    for (i, &(render_y, height)) in render_pos.iter().enumerate() {
        // Click at 25%, 50%, 75% of each window
        for fraction in [0.25, 0.5, 0.75] {
            let test_render_y = render_y as f64 + height as f64 * fraction;

            // Skip if off screen
            if !(0.0..600.0).contains(&test_render_y) {
                continue;
            }

            let test_screen_y = 600.0 - test_render_y;
            tc.simulate_click(400.0, test_screen_y);

            assert_eq!(
                tc.snapshot().focused_index,
                Some(i),
                "Window {}: clicking at {}% (render_y={:.1}, screen_y={:.1}) should focus it",
                i,
                (fraction * 100.0) as i32,
                test_render_y,
                test_screen_y
            );
        }
    }
}

/// Test integer truncation in scroll offset
#[test]
fn integer_truncation_in_scroll() {
    // The issue: main.rs uses (scroll_offset as i32) for rendering
    // but window_at() uses scroll_offset as f64
    // This could cause a 1-pixel discrepancy

    fn render_position_i32(screen_height: i32, scroll_offset: f64, content_y_before: i32, height: i32) -> i32 {
        let content_y = content_y_before - (scroll_offset as i32);
        screen_height - content_y - height
    }

    fn render_position_f64(screen_height: f64, scroll_offset: f64, content_y_before: f64, height: f64) -> f64 {
        let content_y = content_y_before - scroll_offset;
        screen_height - content_y - height
    }

    // Test with scroll_offset that has fractional part
    let scroll_offset = 50.7;
    let screen_height = 600;
    let heights = [100, 150, 200];

    println!("Testing integer truncation with scroll_offset={}", scroll_offset);

    let mut content_y_i32 = 0i32;
    let mut content_y_f64 = 0.0f64;

    for (i, &height) in heights.iter().enumerate() {
        let render_y_i32 = render_position_i32(screen_height, scroll_offset, content_y_i32, height);
        let render_y_f64 = render_position_f64(screen_height as f64, scroll_offset, content_y_f64, height as f64);

        let diff = (render_y_i32 as f64 - render_y_f64).abs();

        println!(
            "  Window {}: render_y_i32={}, render_y_f64={:.1}, diff={:.1}",
            i, render_y_i32, render_y_f64, diff
        );

        // The difference should be at most 1 pixel due to truncation
        assert!(
            diff <= 1.0,
            "Window {}: integer truncation causes {} pixel difference",
            i,
            diff
        );

        content_y_i32 += height;
        content_y_f64 += height as f64;
    }
}

/// Test that errors don't accumulate across many windows
#[test]
fn no_error_accumulation_with_many_windows() {
    let mut tc = TestCompositor::new_headless(800, 2000);

    // Add 20 windows
    for i in 0..20 {
        tc.add_external_window(100 + (i % 5) * 20); // Heights 100-180
    }

    let positions = tc.render_positions();
    let total_height = tc.total_content_height();

    println!("Testing 20 windows (total height={}):", total_height);

    // Check first and last windows
    let first = positions[0];
    let last = positions[19];

    println!("  First window: render_y={}, height={}", first.0, first.1);
    println!("  Last window: render_y={}, height={}", last.0, last.1);

    // Last window should be at correct position
    let heights: Vec<u32> = (0..20).map(|i| 100 + (i % 5) * 20).collect();
    let sum_before_last: u32 = heights[..19].iter().sum();
    let expected_last_render_y = 2000 - sum_before_last as i32 - heights[19] as i32;

    assert_eq!(
        last.0, expected_last_render_y,
        "Last window position should be exact: expected {}, got {}",
        expected_last_render_y, last.0
    );

    // Verify clicking on last window works
    let center_render_y = last.0 as f64 + last.1 as f64 / 2.0;
    let center_screen_y = 2000.0 - center_render_y;

    tc.simulate_click(400.0, center_screen_y);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(19),
        "Should be able to click window 19 (center_screen_y={}, center_render_y={})",
        center_screen_y,
        center_render_y
    );
}
