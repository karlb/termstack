//! Tests for input event handling
//!
//! These tests verify that input events (pointer motion, clicks, scroll)
//! are handled correctly, particularly the coordinate system conversions.

use test_harness::TestCompositor;

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
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add some windows to create scrollable content
    tc.add_external_window(400);
    tc.add_external_window(400);
    tc.add_external_window(400);

    // Total content: 1200px, viewport: 600px, should be scrollable

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
    // Max scroll is total_height - viewport_height = 1200 - 600 = 600
    assert_eq!(max_scroll, 600.0, "scroll should clamp to max");
}

/// Verify that clicks at screen coordinates correctly identify windows
#[test]
fn click_identifies_window_correctly() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add two windows of 200px each, stacked vertically
    // Window 0: render Y = 0 to 200 (screen Y = 400 to 600)
    // Window 1: render Y = 200 to 400 (screen Y = 200 to 400)
    tc.add_external_window(200);
    tc.add_external_window(200);

    // Get render positions to understand where windows are
    let positions = tc.render_positions();
    assert_eq!(positions.len(), 2);

    // With no terminals and no scroll:
    // Window 0: render Y = 0 to 200
    // Window 1: render Y = 200 to 400
    assert_eq!(positions[0], (0, 200), "window 0 at render Y=0");
    assert_eq!(positions[1], (200, 200), "window 1 at render Y=200");

    // In screen coordinates (Y=0 at top, height=600):
    // Window 0: screen Y = 400 to 600 (bottom of screen)
    // Window 1: screen Y = 200 to 400 (above window 0)

    // Click in the BOTTOM area of screen (high screen Y = low render Y)
    // Screen Y=500 -> Render Y=100 -> hits window 0
    tc.simulate_click(400.0, 500.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(0),
        "clicking at screen Y=500 should focus window 0"
    );

    // Click in the UPPER-MIDDLE area of screen
    // Screen Y=300 -> Render Y=300 -> hits window 1
    tc.simulate_click(400.0, 300.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(1),
        "clicking at screen Y=300 should focus window 1"
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

    // With no scroll:
    // Window 0: render Y = 0 to 300
    // Window 1: render Y = 300 to 600 (partially visible)
    // Window 2: render Y = 600 to 900 (off screen)

    // Scroll down by 300px
    tc.set_scroll(300.0);

    // Now with scroll=300:
    // Window 0: render Y = -300 to 0 (off screen top)
    // Window 1: render Y = 0 to 300 (visible)
    // Window 2: render Y = 300 to 600 (visible)

    let positions = tc.render_positions();
    assert_eq!(positions[0], (-300, 300), "window 0 scrolled off top");
    assert_eq!(positions[1], (0, 300), "window 1 at render Y=0");
    assert_eq!(positions[2], (300, 300), "window 2 at render Y=300");

    // Click at bottom of visible area (should hit window 2)
    // Screen Y=400 -> Render Y=200 -> hits window 2 (at render Y 300-600)
    // Wait, let me recalculate: render Y=200 is in window 1 range (0-300)
    // Let me click lower: Screen Y=500 -> Render Y=100 -> hits window 1

    tc.simulate_click(400.0, 100.0);
    // Screen Y=100 -> Render Y=500 -> hits window 2 (at render Y 300-600)
    assert_eq!(
        tc.snapshot().focused_index,
        Some(2),
        "clicking at screen Y=100 (render Y=500) should focus window 2"
    );

    // Click at top of visible area (should hit window 1)
    // Screen Y=400 -> Render Y=200 -> hits window 1 (at render Y 0-300)
    tc.simulate_click(400.0, 400.0);
    assert_eq!(
        tc.snapshot().focused_index,
        Some(1),
        "clicking at screen Y=400 (render Y=200) should focus window 1"
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
