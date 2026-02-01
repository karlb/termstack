//! Boundary and edge case tests
//!
//! These tests verify behavior at boundaries and edge cases that property tests might miss.

use compositor::layout::ColumnLayout;
use test_harness::TestCompositor;

// ========== Empty state tests ==========

#[test]
fn empty_window_list_click_returns_none() {
    let compositor = TestCompositor::new_headless(1280, 720);

    // Click anywhere on empty compositor should find no window
    assert!(compositor.window_at(100.0).is_none());
    assert!(compositor.window_at(0.0).is_none());
    assert!(compositor.window_at(719.0).is_none());
    assert!(compositor.window_at(-100.0).is_none());
    assert!(compositor.window_at(1000.0).is_none());
}

#[test]
fn empty_window_list_render_positions_empty() {
    let compositor = TestCompositor::new_headless(1280, 720);
    assert!(compositor.render_positions().is_empty());
}

#[test]
fn empty_window_list_total_height_zero() {
    let compositor = TestCompositor::new_headless(1280, 720);
    assert_eq!(compositor.total_content_height(), 0);
}

#[test]
fn empty_layout_scroll_to_show_returns_none() {
    let layout = ColumnLayout::calculate_from_heights(std::iter::empty(), 720, 0.0);
    assert!(layout.scroll_to_show(0, 720).is_none());
    assert!(layout.scroll_to_show(100, 720).is_none());
}

// ========== Click at exact boundary tests ==========

#[test]
fn click_at_exact_window_top_boundary() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);
    compositor.add_external_window(200);

    // Window 0: render_y = 720 - 200 = 520 (bottom), top = 720
    // Window 1: render_y = 720 - 400 = 320 (bottom), top = 520
    // Click exactly at boundary between windows (render_y = 520)
    // This is the top of window 1 and bottom of window 0

    let positions = compositor.render_positions();
    assert_eq!(positions.len(), 2);

    let (w0_y, _) = positions[0];
    let (w1_y, _) = positions[1];

    // Window 0 should end where window 1 begins
    assert_eq!(w0_y, w1_y + 200);

    // Click at the boundary
    let boundary_y = (w0_y) as f64;
    let result = compositor.window_at(boundary_y);

    // Boundary behavior: the click at boundary should hit window 0 (range is [y, y+h))
    assert!(result == Some(0) || result == Some(1), "Click at boundary should hit a window");
}

#[test]
fn click_at_exact_window_bottom_boundary() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);

    let positions = compositor.render_positions();
    let (render_y, _height) = positions[0];

    // Click exactly at the bottom edge (render_y is the bottom in render coords)
    let result = compositor.window_at(render_y as f64);

    // Should be inside window 0 (boundary is inclusive at bottom)
    assert_eq!(result, Some(0));
}

#[test]
fn click_just_outside_window_bottom() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);

    let positions = compositor.render_positions();
    let (render_y, _height) = positions[0];

    // Click just below the bottom edge
    let result = compositor.window_at(render_y as f64 - 1.0);

    // Should be outside (below) window 0
    assert!(result.is_none(), "Click below window should find nothing");
}

// ========== Scroll boundary tests ==========

#[test]
fn scroll_to_negative_clamped_to_zero() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);
    compositor.add_external_window(200);

    // Try to scroll negative
    compositor.scroll(-1000.0);

    assert_eq!(compositor.scroll_offset(), 0.0, "Negative scroll should clamp to 0");
}

#[test]
fn scroll_beyond_max_clamped() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(500);
    compositor.add_external_window(500);

    // Total = 1000, output = 720, max_scroll = 280
    let max_scroll = 1000 - 720;

    // Try to scroll way beyond
    compositor.scroll(10000.0);

    assert!(
        (compositor.scroll_offset() - max_scroll as f64).abs() < 0.1,
        "Scroll beyond max should clamp to {} but got {}",
        max_scroll,
        compositor.scroll_offset()
    );
}

#[test]
fn scroll_with_content_smaller_than_viewport() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(100); // Much smaller than 720

    // Try to scroll down - should stay at 0 because content fits
    compositor.scroll(100.0);

    assert_eq!(
        compositor.scroll_offset(),
        0.0,
        "Should not scroll when content fits in viewport"
    );
}

#[test]
fn scroll_exactly_to_max() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(500);
    compositor.add_external_window(500);

    let max_scroll = (1000 - 720) as f64;
    compositor.set_scroll(max_scroll);

    assert!(
        (compositor.scroll_offset() - max_scroll).abs() < 0.1,
        "Setting scroll exactly to max should work"
    );

    // Scroll down by 1 more - should stay at max
    compositor.scroll(1.0);
    assert!(
        (compositor.scroll_offset() - max_scroll).abs() < 0.1,
        "Scrolling past max should clamp"
    );
}

// ========== Height edge cases ==========

#[test]
fn window_height_minimum() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(1); // Minimum height

    let snapshot = compositor.snapshot();
    assert_eq!(snapshot.window_heights[0], 1);
    assert_eq!(snapshot.total_height, 1);
}

#[test]
fn window_height_very_large() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(10000); // Very large

    let snapshot = compositor.snapshot();
    assert_eq!(snapshot.window_heights[0], 10000);
    assert_eq!(snapshot.total_height, 10000);

    // Should still be able to interact with it
    let positions = compositor.render_positions();
    assert_eq!(positions.len(), 1);
}

#[test]
fn multiple_windows_mixed_heights() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(1);
    compositor.add_external_window(1000);
    compositor.add_external_window(50);
    compositor.add_external_window(500);

    let snapshot = compositor.snapshot();
    assert_eq!(snapshot.window_count, 4);
    assert_eq!(snapshot.total_height, 1 + 1000 + 50 + 500);

    // All should be findable
    let positions = compositor.render_positions();
    assert_eq!(positions.len(), 4);
}

// ========== Layout boundary tests ==========

#[test]
fn layout_single_window_fills_viewport() {
    let layout = ColumnLayout::calculate_from_heights([720], 720, 0.0);

    assert_eq!(layout.total_height, 720);
    assert_eq!(layout.window_positions.len(), 1);
    assert!(layout.window_positions[0].visible);
    assert_eq!(layout.window_positions[0].y, 0);
    assert_eq!(layout.window_positions[0].height, 720);
}

#[test]
fn layout_window_exactly_at_viewport_bottom() {
    // Window starts exactly at viewport bottom edge
    let layout = ColumnLayout::calculate_from_heights([720, 100], 720, 0.0);

    // Second window starts at y=720, which is exactly at viewport bottom
    assert_eq!(layout.window_positions[1].y, 720);
    assert!(!layout.window_positions[1].visible, "Window starting at viewport bottom should not be visible");
}

#[test]
fn layout_window_one_pixel_into_viewport() {
    // Scroll so window is 1 pixel into viewport
    let layout = ColumnLayout::calculate_from_heights([720, 100], 720, 1.0);

    // Second window is now at y=719 (720 - 1 scroll)
    assert_eq!(layout.window_positions[1].y, 719);
    assert!(layout.window_positions[1].visible, "Window 1 pixel into viewport should be visible");
}

#[test]
fn layout_window_one_pixel_out_of_viewport() {
    // Scroll so first window is 1 pixel out of viewport (top)
    let layout = ColumnLayout::calculate_from_heights([100], 720, 101.0);

    // Window is at y = -101, ends at -1 (just above viewport)
    assert_eq!(layout.window_positions[0].y, -101);
    assert!(!layout.window_positions[0].visible, "Window ending above viewport should not be visible");
}

// ========== Focus edge cases ==========

#[test]
fn focus_after_adding_first_window() {
    let mut compositor = TestCompositor::new_headless(1280, 720);

    assert!(compositor.snapshot().focused_index.is_none());

    compositor.add_external_window(200);

    assert_eq!(compositor.snapshot().focused_index, Some(0));
}

#[test]
fn click_off_screen_position() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);

    // Click way below screen
    compositor.simulate_click(100.0, 10000.0);

    // Focus should remain valid
    let snapshot = compositor.snapshot();
    if let Some(idx) = snapshot.focused_index {
        assert!(idx < snapshot.window_count);
    }
}

#[test]
fn click_negative_coordinates() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);

    // Click at negative Y (above screen)
    compositor.simulate_click(100.0, -100.0);

    // Should not panic and state should be valid
    let snapshot = compositor.snapshot();
    if let Some(idx) = snapshot.focused_index {
        assert!(idx < snapshot.window_count);
    }
}

// ========== Visibility edge cases ==========

#[test]
fn visibility_window_spans_entire_viewport() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(1000);

    // Window is taller than viewport
    let visible = compositor.visible_portion(0);
    assert!(visible.is_some());

    let (start, end) = visible.unwrap();
    assert!(start >= 0);
    assert!(end <= 720);
}

#[test]
fn visibility_when_scrolled_partially() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(400);
    compositor.add_external_window(400);

    // Scroll partially
    compositor.set_scroll(200.0);

    // Window 0 should be partially visible (200px scrolled off top)
    let visible0 = compositor.visible_portion(0);
    assert!(visible0.is_some());

    // Window 1 should be fully visible
    let visible1 = compositor.visible_portion(1);
    assert!(visible1.is_some());
}

// ========== Render position consistency ==========

#[test]
fn render_positions_sum_to_total_height() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(150);
    compositor.add_external_window(200);
    compositor.add_external_window(175);

    let positions = compositor.render_positions();
    let total_from_positions: i32 = positions.iter().map(|(_, h)| h).sum();

    assert_eq!(total_from_positions, 150 + 200 + 175);
}

#[test]
fn render_positions_adjacent_windows_touch() {
    let mut compositor = TestCompositor::new_headless(1280, 720);
    compositor.add_external_window(200);
    compositor.add_external_window(200);
    compositor.add_external_window(200);

    let positions = compositor.render_positions();

    for i in 1..positions.len() {
        let (prev_y, _) = positions[i - 1];
        let (curr_y, curr_h) = positions[i];

        // In render coords, windows stack. prev bottom should touch curr top
        // prev_y is render bottom of prev, prev_y + prev_h is top
        // curr_y is render bottom of curr, curr_y + curr_h is top
        // They should be adjacent
        assert_eq!(
            prev_y,
            curr_y + curr_h,
            "Windows {} and {} should be adjacent",
            i - 1,
            i
        );
    }
}
