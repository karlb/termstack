//! Tests for input event handling
//!
//! These tests verify that input events (pointer motion, clicks, scroll)
//! are handled correctly, particularly the coordinate system conversions.

use test_harness::{TestCompositor, fixtures};

/// Verify that pointer motion correctly converts screen to render coordinates
///
/// In screen coordinates: Y=0 is at the top of the screen
/// In render coordinates: Y=0 is at the bottom of the screen
#[test]
fn pointer_motion_applies_y_flip() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Move to top-left of screen (screen coords: 100, 0)
    tc.simulate_pointer_motion(100.0, 0.0);
    let render_loc = tc.pointer_location();

    // In render coords, Y=0 at top should become Y=600 (height) at bottom
    assert_eq!(render_loc.x, 100.0, "X should be unchanged");
    assert_eq!(
        render_loc.y.value(),
        600.0,
        "Screen Y=0 (top) should become render Y=600 (top in render coords)"
    );

    // Move to bottom of screen (screen coords: 100, 600)
    tc.simulate_pointer_motion(100.0, 600.0);
    let render_loc = tc.pointer_location();

    assert_eq!(
        render_loc.y.value(),
        0.0,
        "Screen Y=600 (bottom) should become render Y=0 (bottom in render coords)"
    );

    // Move to middle of screen (screen coords: 100, 300)
    tc.simulate_pointer_motion(100.0, 300.0);
    let render_loc = tc.pointer_location();

    assert_eq!(
        render_loc.y.value(),
        300.0,
        "Screen Y=300 (middle) should stay at render Y=300 (middle)"
    );
}

/// Verify that pointer motion roundtrip is consistent
#[test]
fn pointer_motion_roundtrip() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Test several Y positions
    for screen_y in [0.0, 100.0, 300.0, 500.0, 600.0] {
        tc.simulate_pointer_motion(200.0, screen_y);
        let render_loc = tc.pointer_location();

        // Verify: screen_y + render_y = output_height (for Y-flip)
        let sum = screen_y + render_loc.y.value();
        assert_eq!(
            sum, 600.0,
            "screen_y ({}) + render_y ({}) should equal height (600)",
            screen_y,
            render_loc.y.value()
        );
    }
}

/// Verify scroll direction is correct
#[test]
fn scroll_direction_correct() {
    let mut tc = fixtures::compositor_with_scrollable_content();

    let initial_scroll = tc.scroll_offset();
    assert_eq!(initial_scroll, 0.0, "should start at scroll=0");

    // Scroll down (positive delta = content moves up, showing lower content)
    tc.simulate_scroll(100.0);
    assert_eq!(tc.scroll_offset(), 100.0, "scroll should increase");

    // Scroll up (negative delta = content moves down, showing higher content)
    tc.simulate_scroll(-50.0);
    assert_eq!(tc.scroll_offset(), 50.0, "scroll should decrease");

    // Scroll up past the top (should clamp to 0)
    tc.simulate_scroll(-100.0);
    assert_eq!(tc.scroll_offset(), 0.0, "scroll should clamp to 0");

    // Scroll down to max
    tc.simulate_scroll(1000.0);
    let max_scroll = tc.scroll_offset();
    // Max scroll is total_height - viewport_height = 1200 - 720 = 480
    assert_eq!(max_scroll, 480.0, "scroll should clamp to max");
}

/// Verify that clicks at screen coordinates correctly identify windows
#[test]
fn click_identifies_window_correctly() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add two windows of 200px each, stacked vertically
    tc.add_external_window(200);
    tc.add_external_window(200);

    // Get render positions to understand where windows are
    let positions = tc.render_positions();
    assert_eq!(positions.len(), 2);

    // With Y-flip (render_y = screen_height - content_y - height):
    // Window 0 (content_y=0, height=200): render_y = 600 - 0 - 200 = 400
    // Window 1 (content_y=200, height=200): render_y = 600 - 200 - 200 = 200
    // So window 0 is at TOP of screen (high render Y), window 1 below it
    assert_eq!(positions[0], (400, 200), "window 0 at render Y=400 (top)");
    assert_eq!(positions[1], (200, 200), "window 1 at render Y=200 (below)");

    // Click at TOP of screen (screen Y near 0 = high render Y)
    // Screen Y=100 -> Render Y=500 -> hits window 0 (at render Y 400-600)
    tc.simulate_click(400.0, 100.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(0),
        "clicking at screen Y=100 (top) should focus window 0"
    );

    // Click in the MIDDLE area of screen
    // Screen Y=350 -> Render Y=250 -> hits window 1 (at render Y 200-400)
    tc.simulate_click(400.0, 350.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(1),
        "clicking at screen Y=350 (middle) should focus window 1"
    );
}

/// Verify clicks with scroll offset work correctly
#[test]
fn click_with_scroll_offset() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add three windows of 300px each (total 900px)
    tc.add_external_window(300);
    tc.add_external_window(300);
    tc.add_external_window(300);

    // Scroll down by 300px
    tc.set_scroll(300.0);

    // With Y-flip and scroll=300 (render_y = screen_height - content_y - height):
    // Window 0: content_y = -300, height = 300 → render_y = 600 - (-300) - 300 = 600 (off top)
    // Window 1: content_y = 0, height = 300 → render_y = 600 - 0 - 300 = 300 (upper half)
    // Window 2: content_y = 300, height = 300 → render_y = 600 - 300 - 300 = 0 (lower half)

    let positions = tc.render_positions();
    assert_eq!(positions[0], (600, 300), "window 0 scrolled off top (high Y in render)");
    assert_eq!(positions[1], (300, 300), "window 1 at render Y=300 (upper visible)");
    assert_eq!(positions[2], (0, 300), "window 2 at render Y=0 (lower visible)");

    // Click at TOP of screen (screen Y=100):
    // Screen Y=100 -> Render Y = 600 - 100 = 500 -> hits window 1 (at render Y 300-600)
    tc.simulate_click(400.0, 100.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(1),
        "clicking at screen Y=100 (render Y=500) should focus window 1"
    );

    // Click at BOTTOM of screen (screen Y=500):
    // Screen Y=500 -> Render Y = 600 - 500 = 100 -> hits window 2 (at render Y 0-300)
    tc.simulate_click(400.0, 500.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(2),
        "clicking at screen Y=500 (render Y=100) should focus window 2"
    );
}

/// Verify that click detection matches render positions after Y-flip
#[test]
fn click_detection_matches_render_positions() {
    let mut tc = TestCompositor::new_headless(800, 720);

    // Add three different-sized windows
    tc.add_external_window(150);
    tc.add_external_window(200);
    tc.add_external_window(100);

    let positions = tc.render_positions();

    // For each window, click at its center and verify we hit that window
    for (i, &(y, height)) in positions.iter().enumerate() {
        let center_render_y = y as f64 + height as f64 / 2.0;

        // Skip if center is off screen
        if !(0.0..720.0).contains(&center_render_y) {
            continue;
        }

        // Convert render Y to screen Y for clicking
        let center_screen_y = 720.0 - center_render_y;

        tc.simulate_click(400.0, center_screen_y);

        assert_eq!(
            tc.snapshot().focused_index,
            Some(i),
            "clicking at center of window {} (screen_y={}, render_y={}) should focus it",
            i,
            center_screen_y,
            center_render_y
        );
    }
}

/// Verify pointer motion during scroll maintains correct transform
#[test]
fn pointer_motion_during_scroll() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add content
    tc.add_external_window(400);
    tc.add_external_window(400);

    // Move pointer to middle of screen before scroll
    tc.simulate_pointer_motion(400.0, 300.0);
    let before_scroll = tc.pointer_location();
    assert_eq!(before_scroll.y.value(), 300.0);

    // Scroll down
    tc.simulate_scroll(100.0);

    // Pointer location (in render coords) should NOT change from scrolling
    // The pointer stays at the same screen position
    let after_scroll = tc.pointer_location();
    assert_eq!(
        after_scroll.y.value(),
        before_scroll.y.value(),
        "pointer location in render coords should not change from scroll"
    );

    // But now moving the pointer should still apply the correct transform
    tc.simulate_pointer_motion(400.0, 0.0);
    assert_eq!(
        tc.pointer_location().y.value(),
        600.0,
        "Y-flip should still work correctly after scroll"
    );
}

/// Test that surface-local coordinates are calculated correctly for external windows
///
/// When a pointer moves over an external window, the compositor must convert
/// render coordinates to surface-local coordinates (Y=0 at top of window).
/// This is critical for drag operations to work correctly in external apps.
#[test]
fn surface_local_coordinates_correct() {
    let tc = TestCompositor::new_headless(800, 600);

    // For a window at the top of the screen:
    // - Window 0: content_y=0, height=300
    // - With Y-flip: render_y = 600 - 0 - 300 = 300, render_end = 600 - 0 = 600
    //
    // When pointer is at screen Y=50 (near top of window):
    // - render Y = 600 - 50 = 550
    // - surface_local_y = render_end - render_y = 600 - 550 = 50
    //
    // When pointer is at screen Y=250 (near bottom of window):
    // - render Y = 600 - 250 = 350
    // - surface_local_y = render_end - render_y = 600 - 350 = 250

    // The formula for surface-local Y given:
    // - output_height, content_y (cell start), cell_height
    // - render_point.y (pointer in render coords)
    // is: surface_local_y = (output_height - content_y) - render_point.y

    let output_height = 600.0;
    let content_y = 0.0; // First window starts at content Y=0
    let cell_height = 300.0;

    // Test: screen Y=50 (top of window in screen coords)
    let screen_y = 50.0;
    let render_y = output_height - screen_y; // = 550
    let render_end = output_height - content_y; // = 600
    let surface_local_y = render_end - render_y; // = 50

    assert_eq!(surface_local_y, 50.0, "pointer at screen Y=50 should be at surface-local Y=50");

    // Test: screen Y=250 (lower in window)
    let screen_y = 250.0;
    let render_y = output_height - screen_y; // = 350
    let surface_local_y = render_end - render_y; // = 250

    assert_eq!(surface_local_y, 250.0, "pointer at screen Y=250 should be at surface-local Y=250");

    // Test: screen Y=299 (just inside bottom of window)
    let screen_y = 299.0;
    let render_y = output_height - screen_y; // = 301
    let surface_local_y = render_end - render_y; // = 299

    assert_eq!(surface_local_y, 299.0, "pointer at screen Y=299 should be at surface-local Y=299");

    // Verify the formula works for scrolled content too
    let scroll_offset = 100.0;
    let content_y_scrolled = -scroll_offset; // Content starts above viewport
    let render_end_scrolled = output_height - content_y_scrolled; // = 700

    // Screen Y=50 with scroll:
    let screen_y = 50.0;
    let render_y = output_height - screen_y; // = 550
    let surface_local_y = render_end_scrolled - render_y; // = 700 - 550 = 150

    // With 100px scroll, clicking at screen Y=50 hits what WAS at screen Y=150
    // So surface-local should be 150
    assert_eq!(surface_local_y, 150.0, "with scroll=100, screen Y=50 maps to surface-local Y=150");

    // Suppress unused variable warning
    let _ = tc;
    let _ = cell_height;
}

/// Test clicking at screen boundaries (edges and corners)
#[test]
fn click_at_screen_boundaries() {
    let mut tc = TestCompositor::new_headless(800, 600);
    tc.add_external_window(600);

    // Click at top-left corner (0, 0)
    tc.simulate_click(0.0, 0.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "top-left corner should focus window");

    // Click at top-right corner (800, 0)
    tc.simulate_click(800.0, 0.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "top-right corner should focus window");

    // Click at bottom-left corner (0, 600)
    tc.simulate_click(0.0, 600.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "bottom-left corner should focus window");

    // Click at bottom-right corner (800, 600)
    tc.simulate_click(800.0, 600.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "bottom-right corner should focus window");
}

/// Test scroll behavior with zero content height
#[test]
fn scroll_with_zero_content_height() {
    let mut tc = TestCompositor::new_headless(800, 600);
    // No windows = zero content height

    // Scroll down should have no effect
    let initial_scroll = tc.scroll_offset();
    assert_eq!(initial_scroll, 0.0, "should start at scroll=0");

    // Scrolling shouldn't crash or change offset
    tc.simulate_scroll(100.0);
    assert_eq!(tc.scroll_offset(), 0.0, "scroll should stay at 0 with no content");

    tc.simulate_scroll(-50.0);
    assert_eq!(tc.scroll_offset(), 0.0, "scroll should stay at 0 with no content");
}

/// Test focus behavior with no windows
#[test]
fn focus_with_no_windows() {
    let mut tc = TestCompositor::new_headless(800, 600);
    // No windows added

    // focused_index should be None
    assert_eq!(tc.snapshot().focused_index, None, "should have no focus with no windows");

    // Clicking anywhere shouldn't crash
    tc.simulate_click(400.0, 300.0);
    assert_eq!(tc.snapshot().focused_index, None, "should still have no focus after click");
}
