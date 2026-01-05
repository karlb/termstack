//! Tests for window positioning and click detection
//!
//! These tests verify that:
//! 1. Windows don't overlap (issue: "foot window is fully inside gnome-maps window")
//! 2. Click targets are not vertically flipped (issue: "click targets seem to be flipped")
//! 3. Render positions match click detection positions

use test_harness::{TestCompositor, assertions, fixtures};

#[test]
fn windows_dont_overlap_with_different_heights() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    // Simulate gnome-maps (tall window) and foot (smaller window)
    tc.add_external_window(400); // gnome-maps
    tc.add_external_window(200); // foot

    let render_pos = tc.render_positions();

    // With Y-flip (render_y = 720 - content_y - height):
    // Window 0 (content_y=0, height=400): render_y = 720 - 0 - 400 = 320
    // Window 1 (content_y=400, height=200): render_y = 720 - 400 - 200 = 120
    assert_eq!(render_pos[0].0, 320, "window 0 at render Y=320 (top of screen)");
    assert_eq!(render_pos[0].1, 400, "window 0 should have height 400");
    assert_eq!(render_pos[1].0, 120, "window 1 at render Y=120 (below window 0)");
    assert_eq!(render_pos[1].1, 200, "window 1 should have height 200");

    // With Y-flip, windows don't overlap when window_1.end == window_0.start
    let window_0_start = render_pos[0].0;
    let window_1_end = render_pos[1].0 + render_pos[1].1;
    assert_eq!(window_1_end, window_0_start,
        "windows should be contiguous: window 1 ends at {}, window 0 starts at {}",
        window_1_end, window_0_start);
}

#[test]
fn click_detection_matches_render_positions() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    tc.add_external_window(300);
    tc.add_external_window(200);
    tc.add_external_window(150);

    assertions::assert_render_matches_click_detection(&tc);
}

#[test]
fn click_targets_not_vertically_flipped() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    // Add windows in order: window 0 first, then window 1
    tc.add_external_window(200); // Window 0
    tc.add_external_window(300); // Window 1

    // Clicking at Y=50 (top of screen, within window 0) should hit window 0
    assertions::assert_click_at_y_hits_window(&tc, 50.0, Some(0));

    // Clicking at Y=250 (within window 1, which starts at Y=200) should hit window 1
    assertions::assert_click_at_y_hits_window(&tc, 250.0, Some(1));

    // NOT the other way around (which would indicate flipped targets)
    assertions::assert_click_targets_not_flipped(&tc);
}

#[test]
fn windows_stack_correctly_after_terminals() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    // Simulate internal terminals taking up 100 pixels
    tc.set_terminal_height(100);

    // Add two external windows
    tc.add_external_window(200);
    tc.add_external_window(150);

    let render_pos = tc.render_positions();

    // With Y-flip (render_y = 720 - content_y - height):
    // But note: test harness terminal_total_height isn't included in content_y calculation
    // Window 0 (content_y=0, height=200): render_y = 720 - 0 - 200 = 520
    // Window 1 (content_y=200, height=150): render_y = 720 - 200 - 150 = 370
    assert_eq!(render_pos[0].0, 520, "window 0 at render Y=520");
    assert_eq!(render_pos[1].0, 370, "window 1 at render Y=370");

    // Click detection in screen coordinates (Y=0 at top of screen)
    // With windows at render Y 520-720 and 370-520:
    // Screen Y=0 → Render Y=720 → above all windows → None
    // Screen Y=100 → Render Y=620 → in window 0 (520-720) → Some(0)
    // Screen Y=300 → Render Y=420 → in window 0 (520-720)? No, 420 < 520 → in window 1 (370-520) → Some(1)
    assertions::assert_click_at_y_hits_window(&tc, 0.0, None);   // Above windows
    assertions::assert_click_at_y_hits_window(&tc, 100.0, Some(0)); // In window 0
    assertions::assert_click_at_y_hits_window(&tc, 300.0, Some(1)); // In window 1
}

#[test]
fn scroll_affects_both_render_and_click_detection() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    let win = tc.add_external_window(200);
    tc.add_external_window(200);

    // Scroll down by 100 pixels
    tc.scroll_terminal(&win, 100);

    // After scrolling, render and click should still match
    assertions::assert_render_matches_click_detection(&tc);
}

#[test]
fn window_order_preserved_with_many_windows() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    // Add 5 windows with different heights
    tc.add_external_window(100);
    tc.add_external_window(150);
    tc.add_external_window(200);
    tc.add_external_window(180);
    tc.add_external_window(120);

    assertions::assert_window_order_correct(&tc);
    assertions::assert_click_targets_not_flipped(&tc);
}

#[test]
fn zero_height_window_doesnt_break_positioning() {
    // This tests the fallback behavior when bbox returns 0
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    tc.add_external_window(0);   // Simulates bbox returning 0
    tc.add_external_window(200);

    let render_pos = tc.render_positions();

    // With Y-flip (height=720):
    // Window 0 (content_y=0, height=0): render_y = 720 - 0 - 0 = 720
    // Window 1 (content_y=0, height=200): render_y = 720 - 0 - 200 = 520
    assert_eq!(render_pos[0].0, 720, "zero-height window at render Y=720");
    assert_eq!(render_pos[1].0, 520, "window 1 at render Y=520");
}

#[test]
fn changing_window_height_updates_positions() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    tc.add_external_window(200);
    tc.add_external_window(200);

    // With Y-flip (height=720):
    // Window 0 (content_y=0, height=200): render_y = 720 - 0 - 200 = 520
    // Window 1 (content_y=200, height=200): render_y = 720 - 200 - 200 = 320
    assert_eq!(tc.render_positions()[0].0, 520);
    assert_eq!(tc.render_positions()[1].0, 320);

    // Resize window 0 to 400 pixels
    tc.set_window_height(0, 400);

    // Now with Y-flip:
    // Window 0 (content_y=0, height=400): render_y = 720 - 0 - 400 = 320
    // Window 1 (content_y=400, height=200): render_y = 720 - 400 - 200 = 120
    assert_eq!(tc.render_positions()[0].0, 320);
    assert_eq!(tc.render_positions()[1].0, 120);

    // Click detection in screen coords (Y=0 at top)
    // Screen Y=50 → Render Y=670 → in window 0 (320-720) → Some(0)
    // Screen Y=550 → Render Y=170 → in window 1 (120-320) → Some(1)
    assertions::assert_click_at_y_hits_window(&tc, 50.0, Some(0));
    assertions::assert_click_at_y_hits_window(&tc, 550.0, Some(1));
}

/// Test that OpenGL Y-flip calculation is correct.
/// In OpenGL, Y=0 is at the BOTTOM of the screen (Y-up coordinate system).
/// In screen coordinates, Y=0 is at the TOP (Y-down coordinate system).
/// When rendering to OpenGL, we must flip destination Y coordinates.
#[test]
fn opengl_y_flip_calculation_correct() {
    let output_height: i32 = 720;

    // Window at screen Y=100 with height 200
    // In screen coords: top at Y=100, bottom at Y=300
    // In OpenGL coords: bottom at Y=620 (720-100), top at Y=420 (720-100-200)
    // So the OpenGL destination should have loc.y = 420, size.h = 200

    let screen_y = 100;
    let window_height = 200;

    // The flipped Y position for OpenGL
    let opengl_y = output_height - screen_y - window_height;

    assert_eq!(opengl_y, 420, "OpenGL Y should be output_height - screen_y - height");

    // Verify the window occupies the correct screen region after flip
    let opengl_bottom = opengl_y;  // 420
    let opengl_top = opengl_y + window_height;  // 620

    // Convert back to screen coords to verify
    let screen_top_from_opengl = output_height - opengl_top;  // 720 - 620 = 100
    let screen_bottom_from_opengl = output_height - opengl_bottom;  // 720 - 420 = 300

    assert_eq!(screen_top_from_opengl, screen_y, "screen top should match");
    assert_eq!(screen_bottom_from_opengl, screen_y + window_height, "screen bottom should match");
}

/// Test that flipping destination Y is required for proper window rendering.
/// This test documents that when element.draw() is called, the destination
/// geometry must have its Y coordinate flipped for OpenGL's Y-up system.
#[test]
fn destination_y_must_be_flipped_for_opengl() {
    // This test verifies the flip formula used in rendering
    let output_height: i32 = 720;

    struct TestCase {
        screen_y: i32,
        height: i32,
        expected_opengl_y: i32,
    }

    let cases = [
        TestCase { screen_y: 0, height: 100, expected_opengl_y: 620 },    // top of screen
        TestCase { screen_y: 100, height: 200, expected_opengl_y: 420 },  // middle
        TestCase { screen_y: 520, height: 200, expected_opengl_y: 0 },    // bottom of screen
        TestCase { screen_y: 360, height: 360, expected_opengl_y: 0 },    // full height from middle
    ];

    for (i, case) in cases.iter().enumerate() {
        let flipped_y = output_height - case.screen_y - case.height;
        assert_eq!(
            flipped_y, case.expected_opengl_y,
            "case {}: screen_y={}, height={} should flip to opengl_y={}",
            i, case.screen_y, case.height, case.expected_opengl_y
        );
    }
}

// ============================================================================
// Multiple Windows Tests
// ============================================================================

#[test]
fn three_windows_stack_correctly() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    tc.add_external_window(200);
    tc.add_external_window(300);
    tc.add_external_window(150);

    let pos = tc.render_positions();

    // With Y-flip (height=720):
    // Window 0 (content_y=0, height=200): render_y = 720 - 0 - 200 = 520
    // Window 1 (content_y=200, height=300): render_y = 720 - 200 - 300 = 220
    // Window 2 (content_y=500, height=150): render_y = 720 - 500 - 150 = 70
    assert_eq!(pos[0], (520, 200), "window 0");
    assert_eq!(pos[1], (220, 300), "window 1");
    assert_eq!(pos[2], (70, 150), "window 2");

    // With Y-flip, contiguous means: window[i].end == window[i-1].start
    assert_eq!(pos[1].0 + pos[1].1, pos[0].0, "window 1 end == window 0 start");
    assert_eq!(pos[2].0 + pos[2].1, pos[1].0, "window 2 end == window 1 start");
}

#[test]
fn five_windows_with_varied_heights() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    let heights = [100, 250, 75, 400, 180];
    for h in heights {
        tc.add_external_window(h);
    }

    let pos = tc.render_positions();
    let screen_height = 720;

    // With Y-flip: render_y = screen_height - content_y - height
    let mut content_y = 0;
    for (i, &h) in heights.iter().enumerate() {
        let expected_render_y = screen_height - content_y - (h as i32);
        assert_eq!(pos[i].0, expected_render_y, "window {} render Y", i);
        assert_eq!(pos[i].1, h as i32, "window {} height", i);
        content_y += h as i32;
    }

    // Click detection in screen coordinates (heights sum to 1005, extends beyond screen)
    // Window 0: render Y 620-720 (content 0-100) → screen Y 0-100
    // Window 1: render Y 370-620 (content 100-350) → screen Y 100-350
    // Window 2: render Y 295-370 (content 350-425) → screen Y 350-425
    // Window 3: render Y -105 to 295 (content 425-825) → screen Y 425-825 (partially off-screen)
    // Window 4: render Y -285 to -105 (off-screen)
    assertions::assert_click_at_y_hits_window(&tc, 50.0, Some(0));   // in window 0
    assertions::assert_click_at_y_hits_window(&tc, 200.0, Some(1));  // in window 1
    assertions::assert_click_at_y_hits_window(&tc, 400.0, Some(2));  // in window 2
    assertions::assert_click_at_y_hits_window(&tc, 500.0, Some(3));  // in window 3
}

#[test]
#[allow(clippy::needless_range_loop)]
fn ten_small_windows() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    for _ in 0..10 {
        tc.add_external_window(50);
    }

    let pos = tc.render_positions();
    let screen_height = 720;

    // With Y-flip: render_y = screen_height - content_y - height
    // Total height = 10 * 50 = 500px, all windows visible
    for i in 0..10 {
        let content_y = i as i32 * 50;
        let expected_render_y = screen_height - content_y - 50;
        assert_eq!(pos[i].0, expected_render_y, "window {} render Y", i);
        assert_eq!(pos[i].1, 50, "window {} height", i);
    }

    // Click detection for each window
    // Window i is at render Y (720 - 50*i - 50) to (720 - 50*i)
    // In screen coords: Y = 50*i to 50*(i+1)
    for i in 0..10 {
        let screen_y = (i as f64 * 50.0) + 25.0; // middle of each window in screen coords
        assertions::assert_click_at_y_hits_window(&tc, screen_y, Some(i));
    }
}

// ============================================================================
// Scrolling Tests
// ============================================================================

#[test]
fn scroll_moves_all_windows_up() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(300);
    tc.add_external_window(300);
    tc.add_external_window(300);

    // With Y-flip (height=720):
    // Window 0 (content_y=0, h=300): render_y = 720 - 0 - 300 = 420
    // Window 1 (content_y=300, h=300): render_y = 720 - 300 - 300 = 120
    // Window 2 (content_y=600, h=300): render_y = 720 - 600 - 300 = -180 (partially off)
    let pos_before = tc.render_positions();
    assert_eq!(pos_before[0].0, 420);
    assert_eq!(pos_before[1].0, 120);
    assert_eq!(pos_before[2].0, -180);

    // Scroll down by 100px (content_y starts at -scroll_offset = -100)
    tc.scroll(100.0);

    // After scroll:
    // Window 0 (content_y=-100, h=300): render_y = 720 - (-100) - 300 = 520
    // Window 1 (content_y=200, h=300): render_y = 720 - 200 - 300 = 220
    // Window 2 (content_y=500, h=300): render_y = 720 - 500 - 300 = -80
    let pos_after = tc.render_positions();
    assert_eq!(pos_after[0].0, 520, "window 0 render Y after scroll");
    assert_eq!(pos_after[1].0, 220, "window 1 render Y after scroll");
    assert_eq!(pos_after[2].0, -80, "window 2 render Y after scroll");
}

#[test]
fn scroll_and_click_detection_stay_consistent() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Use larger windows so total > screen height, enabling scroll
    tc.add_external_window(400);
    tc.add_external_window(400);
    tc.add_external_window(400);
    // Total = 1200, screen = 720, max_scroll = 480

    // Before scroll: click at Y=100 should hit window 0
    assertions::assert_click_at_y_hits_window(&tc, 100.0, Some(0));
    assertions::assert_click_at_y_hits_window(&tc, 500.0, Some(1));

    // Scroll down by 300px
    tc.scroll(300.0);

    // After scroll: window 0 at Y=-300 to Y=100, window 1 at Y=100 to Y=500
    // Click at Y=150 should now hit window 1 (which spans Y=100 to Y=500)
    assertions::assert_click_at_y_hits_window(&tc, 150.0, Some(1));

    // Click at Y=50 should hit window 0 (still visible from Y=-300 to Y=100)
    assertions::assert_click_at_y_hits_window(&tc, 50.0, Some(0));

    // Verify render positions match
    assertions::assert_render_matches_click_detection(&tc);
}

#[test]
fn scroll_to_bottom_shows_last_window() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Total height = 1500, output = 720, max scroll = 780
    tc.add_external_window(500);
    tc.add_external_window(500);
    tc.add_external_window(500);

    // Scroll to max
    tc.set_scroll(780.0);

    let pos = tc.render_positions();

    // With Y-flip and scroll=780:
    // content_y starts at -scroll = -780
    // Window 0 (content_y=-780, h=500): render_y = 720 - (-780) - 500 = 1000 (off top)
    // Window 1 (content_y=-280, h=500): render_y = 720 - (-280) - 500 = 500
    // Window 2 (content_y=220, h=500): render_y = 720 - 220 - 500 = 0 (at bottom)
    assert_eq!(pos[2].0, 0, "window 2 at render Y=0 (bottom of screen)");
    assert!(tc.is_window_visible(2), "window 2 should be visible");

    // Window 0 at render Y=1000, which is above screen height 720
    assert_eq!(pos[0].0, 1000);
    assert!(!tc.is_window_visible(0), "window 0 should be off-screen (above)");
}

#[test]
fn scroll_preserves_window_order() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(200);
    tc.add_external_window(300);
    tc.add_external_window(250);

    // Test at various scroll positions
    for scroll in [0.0, 50.0, 100.0, 200.0, 300.0] {
        tc.set_scroll(scroll);
        assertions::assert_window_order_correct(&tc);
        assertions::assert_render_matches_click_detection(&tc);
    }
}

#[test]
fn partial_window_visibility_during_scroll() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Use windows that total more than screen height
    tc.add_external_window(500);
    tc.add_external_window(500);
    // Total = 1000, screen = 720, max_scroll = 280

    // Scroll so window 0 is partially visible
    tc.set_scroll(200.0);

    // With Y-flip and scroll=200:
    // Window 0 (content_y=-200, h=500): render_y = 720 - (-200) - 500 = 420, range 420-920
    // Window 1 (content_y=300, h=500): render_y = 720 - 300 - 500 = -80, range -80 to 420
    // Screen visible range: 0 to 720

    // Window 0: render Y 420-920, visible portion is 420-720
    let visible_0 = tc.visible_portion(0);
    assert_eq!(visible_0, Some((420, 720)), "window 0 partial visibility");

    // Window 1: render Y -80 to 420, visible portion is 0-420
    let visible_1 = tc.visible_portion(1);
    assert_eq!(visible_1, Some((0, 420)), "window 1 partial visibility");

    // Both should register as visible
    assert!(tc.is_window_visible(0));
    assert!(tc.is_window_visible(1));
}

#[test]
fn window_completely_scrolled_off_top() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Use larger windows so we can scroll enough to hide window 0
    tc.add_external_window(300);
    tc.add_external_window(500);
    tc.add_external_window(500);
    // Total = 1300, screen = 720, max_scroll = 580

    // Scroll so window 0 is completely off-screen (needs scroll > 300)
    tc.set_scroll(350.0);

    // Window 0 at Y=-350, height=300, bottom at Y=-50 (off-screen)
    assert!(!tc.is_window_visible(0), "window 0 should be off-screen");
    assert_eq!(tc.visible_portion(0), None);

    // Window 1 should be visible (at Y=-50 to Y=450)
    assert!(tc.is_window_visible(1));
}

#[test]
fn click_on_partially_visible_window() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Use larger windows so scrolling is possible
    tc.add_external_window(500);
    tc.add_external_window(500);
    // Total = 1000, screen = 720, max_scroll = 280

    // Scroll so window 0 is partially off-screen
    tc.set_scroll(200.0);

    // Window 0 is at Y=-200 to Y=300
    // Clicking at Y=50 should hit window 0 (the visible part)
    assertions::assert_click_at_y_hits_window(&tc, 50.0, Some(0));

    // Clicking at Y=400 should hit window 1 (at Y=300 to Y=800)
    assertions::assert_click_at_y_hits_window(&tc, 400.0, Some(1));
}

// ============================================================================
// Windows with Terminals Tests
// ============================================================================

#[test]
fn windows_after_terminals_with_scroll() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(300);
    tc.add_external_window(400);
    tc.add_external_window(400);
    // Note: terminal_height in test harness doesn't affect render_positions calculation

    // With Y-flip (height=720):
    // Window 0 (content_y=0, h=400): render_y = 720 - 0 - 400 = 320
    // Window 1 (content_y=400, h=400): render_y = 720 - 400 - 400 = -80
    let pos = tc.render_positions();
    assert_eq!(pos[0].0, 320, "window 0 at render Y=320");
    assert_eq!(pos[1].0, -80, "window 1 at render Y=-80");

    // Click detection in screen coords
    // Window 0: render 320-720 → screen 0-400
    // Window 1: render -80 to 320 → screen 400-800 (but clipped to screen 400-720)
    assertions::assert_click_at_y_hits_window(&tc, 100.0, Some(0)); // In window 0
    assertions::assert_click_at_y_hits_window(&tc, 500.0, Some(1)); // In window 1

    // Scroll down by 200px
    tc.scroll(200.0);

    // After scroll (content_y starts at -200):
    // Window 0: render_y = 720 - (-200) - 400 = 520
    // Window 1: render_y = 720 - 200 - 400 = 120
    let pos_scrolled = tc.render_positions();
    assert_eq!(pos_scrolled[0].0, 520);
    assert_eq!(pos_scrolled[1].0, 120);

    // Click detection after scroll
    // Window 0: render 520-920 → visible 520-720 → screen 0-200
    // Window 1: render 120-520 → screen 200-600
    assertions::assert_click_at_y_hits_window(&tc, 100.0, Some(0));
    assertions::assert_click_at_y_hits_window(&tc, 400.0, Some(1));
}

#[test]
fn large_terminal_with_small_windows() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(600);
    tc.add_external_window(100);
    tc.add_external_window(100);
    tc.add_external_window(100);

    // With Y-flip (height=720):
    // Window 0: render_y = 720 - 0 - 100 = 620
    // Window 1: render_y = 720 - 100 - 100 = 520
    // Window 2: render_y = 720 - 200 - 100 = 420
    let pos = tc.render_positions();
    assert_eq!(pos[0].0, 620);
    assert_eq!(pos[1].0, 520);
    assert_eq!(pos[2].0, 420);

    // All windows are visible (total height 300 < screen height 720)
    assert!(tc.is_window_visible(0));
    assert!(tc.is_window_visible(1));
    assert!(tc.is_window_visible(2));
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn empty_compositor_no_crash() {
    let tc = TestCompositor::new_headless(1280, 720);

    assert!(tc.render_positions().is_empty());
    assert!(tc.window_click_ranges().is_empty());
    assert_eq!(tc.window_at(100.0), None);
}

#[test]
fn single_large_window_bigger_than_screen() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(1500);

    // Window is 1500px, screen is 720px
    assert!(tc.is_window_visible(0));

    // Can scroll through the window
    tc.set_scroll(400.0);
    assert!(tc.is_window_visible(0));

    // Click anywhere on screen should hit window 0
    for y in [0.0, 100.0, 300.0, 500.0, 700.0] {
        assertions::assert_click_at_y_hits_window(&tc, y, Some(0));
    }
}

#[test]
fn max_scroll_boundary() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(500);
    tc.add_external_window(500);
    // Total = 1000, max scroll = 1000 - 720 = 280

    // Try to scroll past max
    tc.set_scroll(1000.0);
    assert_eq!(tc.scroll_offset(), 280.0, "scroll should clamp to max");

    // Scroll should not go negative
    tc.set_scroll(-100.0);
    assert_eq!(tc.scroll_offset(), 0.0, "scroll should clamp to 0");
}

#[test]
fn rapid_scroll_changes() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(200);
    tc.add_external_window(200);
    tc.add_external_window(200);

    // Rapidly scroll back and forth
    for _ in 0..100 {
        tc.scroll(50.0);
        assertions::assert_render_matches_click_detection(&tc);

        tc.scroll(-30.0);
        assertions::assert_render_matches_click_detection(&tc);
    }
}

#[test]
fn window_at_exact_boundaries() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window(200);
    tc.add_external_window(200);

    // With Y-flip (height=720):
    // Window 0: render_y = 720 - 0 - 200 = 520, range [520, 720)
    // Window 1: render_y = 720 - 200 - 200 = 320, range [320, 520)

    // Screen coordinates (Y=0 at top):
    // Window 0: screen [0, 200) → note: screen Y=0 maps to render Y=720 (exclusive end)
    // Window 1: screen [200, 400] → screen Y=400 maps to render Y=320 (inclusive start)

    assertions::assert_click_at_y_hits_window(&tc, 0.1, Some(0));    // just inside top = window 0
    assertions::assert_click_at_y_hits_window(&tc, 199.9, Some(0));  // just before window 1
    assertions::assert_click_at_y_hits_window(&tc, 200.1, Some(1));  // just inside window 1
    assertions::assert_click_at_y_hits_window(&tc, 399.9, Some(1));  // just before end
    assertions::assert_click_at_y_hits_window(&tc, 400.0, Some(1));  // exactly at end (inclusive)
    assertions::assert_click_at_y_hits_window(&tc, 400.1, None);     // past all windows
}

// ============================================================================
// OpenGL Rendering Flip Tests
// ============================================================================

/// Test that multiple windows get correct OpenGL flip positions.
/// This verifies the actual rendering calculation that converts screen Y
/// to OpenGL Y coordinates. Each window should have a unique flipped position.
#[test]
fn multiple_windows_opengl_flip_no_overlap() {
    let output_height: i32 = 720;

    // Simulate two windows at different screen positions
    struct Window {
        screen_y: i32,
        height: i32,
    }

    let windows = [
        Window { screen_y: 0, height: 200 },     // window 0 at top
        Window { screen_y: 200, height: 300 },   // window 1 below
        Window { screen_y: 500, height: 220 },   // window 2 further down
    ];

    // Calculate flipped positions (this is what the rendering code should do)
    let flipped: Vec<(i32, i32)> = windows.iter()
        .map(|w| {
            let opengl_y = output_height - w.screen_y - w.height;
            (opengl_y, w.height)
        })
        .collect();

    // Window 0: screen 0-200 => OpenGL 520-720
    assert_eq!(flipped[0], (520, 200), "window 0 flipped position");

    // Window 1: screen 200-500 => OpenGL 220-520
    assert_eq!(flipped[1], (220, 300), "window 1 flipped position");

    // Window 2: screen 500-720 => OpenGL 0-220
    assert_eq!(flipped[2], (0, 220), "window 2 flipped position");

    // CRITICAL: Verify no overlap in flipped positions
    // Window 0 OpenGL range: 520 to 720
    // Window 1 OpenGL range: 220 to 520
    // Window 2 OpenGL range: 0 to 220
    // These should be contiguous and non-overlapping

    assert_eq!(flipped[2].0 + flipped[2].1, flipped[1].0,
        "window 2 end should meet window 1 start");
    assert_eq!(flipped[1].0 + flipped[1].1, flipped[0].0,
        "window 1 end should meet window 0 start");
    assert_eq!(flipped[0].0 + flipped[0].1, output_height,
        "window 0 end should be at output height");
}

/// Test that using window_y (our calculated position) vs geo.loc.y matters.
/// This documents the bug: if geo.loc.y is always 0 (relative to window),
/// all windows would flip to the same position, causing overlap.
#[test]
fn bug_detection_all_windows_same_flip_if_wrong_y_used() {
    let output_height: i32 = 720;

    // If we incorrectly use geo.loc.y = 0 for all windows (relative to window origin)
    // instead of the actual screen position, we get:
    let buggy_flip = |_screen_y: i32, geo_loc_y: i32, height: i32| {
        output_height - geo_loc_y - height
    };

    // Two windows at different screen positions, but geo.loc.y is 0 for both
    let window_0_buggy = buggy_flip(0, 0, 200);    // screen_y=0 ignored, uses geo.loc.y=0
    let window_1_buggy = buggy_flip(200, 0, 300);  // screen_y=200 ignored, uses geo.loc.y=0

    // BUG: Both would render at the same OpenGL Y if using wrong value!
    assert_eq!(window_0_buggy, 520); // 720 - 0 - 200 = 520
    assert_eq!(window_1_buggy, 420); // 720 - 0 - 300 = 420

    // They overlap! Window 0 at 520-720, Window 1 at 420-720
    // Window 1's top (420) is below Window 0's bottom (520)? No wait...
    // Window 1 range: 420 to 720 (420 + 300)
    // Window 0 range: 520 to 720 (520 + 200)
    // They overlap from 520 to 720!
    let w0_bottom = window_0_buggy;
    let w0_top = window_0_buggy + 200;
    let w1_bottom = window_1_buggy;
    let w1_top = window_1_buggy + 300;

    // Check for overlap: ranges [w0_bottom, w0_top] and [w1_bottom, w1_top]
    let overlap = w0_bottom < w1_top && w1_bottom < w0_top;
    assert!(overlap, "using geo.loc.y=0 causes overlap - this is the bug!");

    // The FIX: use screen_y (our calculated window_y) instead
    let correct_flip = |screen_y: i32, height: i32| {
        output_height - screen_y - height
    };

    let window_0_correct = correct_flip(0, 200);
    let window_1_correct = correct_flip(200, 300);

    assert_eq!(window_0_correct, 520); // 720 - 0 - 200 = 520
    assert_eq!(window_1_correct, 220); // 720 - 200 - 300 = 220

    // No overlap: Window 0 at 520-720, Window 1 at 220-520
    let w1c_top = window_1_correct + 300;  // 520

    assert_eq!(w1c_top, window_0_correct, "correct: window 1 ends where window 0 starts");
}

/// Test that element geometry must include location offset for correct rendering.
/// When we call window.render_elements(renderer, location, scale, alpha),
/// the returned elements should have geometry.loc offset by location.
#[test]
fn element_geometry_must_include_location_offset() {
    let output_height: i32 = 720;

    // Simulate what render_elements should return
    struct Element {
        loc_y: i32,  // geometry().loc.y
        height: i32,
    }

    // If render_elements correctly adds location offset:
    // Window 0 at screen_y=0: elements have geo.loc.y = 0
    // Window 1 at screen_y=200: elements have geo.loc.y = 200

    let element_0 = Element { loc_y: 0, height: 200 };
    let element_1 = Element { loc_y: 200, height: 300 };

    let flip_0 = output_height - element_0.loc_y - element_0.height;
    let flip_1 = output_height - element_1.loc_y - element_1.height;

    assert_eq!(flip_0, 520);
    assert_eq!(flip_1, 220);

    // Verify no overlap
    assert_eq!(flip_1 + element_1.height, flip_0,
        "element 1 top should meet element 0 bottom");
}

// ============================================================================
// Multi-Element Window Tests (gnome-maps scenario)
// ============================================================================

/// Test: gnome-maps style window with multiple elements doesn't overlap terminal
/// This simulates the bug in the screenshot where gnome-maps appears both
/// above and below the terminal.
#[test]
fn gnome_maps_style_multi_element_window_no_overlap() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Internal terminal takes 200 pixels (not included in render_positions calculation)
    tc.set_terminal_height(200);

    // Add a simple terminal window (foot)
    tc.add_external_window(150);

    // Add gnome-maps style window with multiple elements:
    // - toolbar at internal y=0
    // - map content at internal y=50
    // - zoom controls at internal y=300
    tc.add_window_with_elements(400, vec![
        (0, 50),    // toolbar
        (50, 250),  // map content
        (300, 100), // zoom controls
    ]);

    // Verify no overlaps between windows
    assertions::assert_no_element_overlaps(&tc);

    // Verify elements stay within window bounds
    assertions::assert_elements_within_window_bounds(&tc);

    // Check specific positions using rendered_elements (content coordinates)
    // Note: rendered_elements includes terminal_total_height offset
    let elements = tc.rendered_elements();

    // Window 0 (foot) at content_y = terminal_height + 0 = 200
    let foot_elem = elements.iter().find(|e| e.window_index == 0).expect("foot element");
    assert_eq!(foot_elem.screen_y, 200, "foot starts after terminal (y=200)");

    // Window 1 (gnome-maps) at content_y = terminal_height + foot_height = 200 + 150 = 350
    let maps_toolbar = elements.iter()
        .find(|e| e.window_index == 1 && e.element_index == 0)
        .expect("maps toolbar element");
    assert_eq!(maps_toolbar.screen_y, 350, "maps toolbar starts after foot (y=350)");
}

/// Test: Elements with negative internal Y offset (like dropdowns/popups)
/// These should NOT cause overlap with previous windows
#[test]
fn negative_element_offset_detected_as_overlap() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(200);

    // First window
    tc.add_external_window(200);

    // Second window with a popup that extends ABOVE the window
    // This simulates a dropdown menu at internal y=-50
    tc.add_window_with_elements(300, vec![
        (-50, 50),  // popup ABOVE window origin - should cause overlap!
        (0, 300),   // main content
    ]);

    // This SHOULD detect an overlap because the popup extends into window 0's space
    let overlaps = tc.find_element_overlaps();
    assert!(!overlaps.is_empty(),
        "Expected overlap from negative element offset, but none detected");

    // The popup element should fail bounds check
    let elements = tc.rendered_elements();
    let popup = elements.iter()
        .find(|e| e.window_index == 1 && e.element_index == 0)
        .expect("popup element");

    // Popup at window_y=400, internal_y=-50 => screen_y=350
    // Window 1 starts at 400, so 350 < 400 = out of bounds
    assert!(popup.screen_y < 400, "popup should be positioned above window start");
}

/// Test: Multiple windows with multiple elements each - no overlaps
#[test]
fn multiple_complex_windows_no_overlap() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(100);

    // Three complex windows
    tc.add_window_with_elements(150, vec![
        (0, 50),   // header
        (50, 100), // content
    ]);

    tc.add_window_with_elements(200, vec![
        (0, 30),   // toolbar
        (30, 140), // main area
        (170, 30), // footer
    ]);

    tc.add_window_with_elements(180, vec![
        (0, 180),  // single full-height element
    ]);

    // All elements should be non-overlapping (uses content coordinates)
    assertions::assert_no_element_overlaps(&tc);
    assertions::assert_elements_within_window_bounds(&tc);

    // Verify element count
    let elements = tc.rendered_elements();
    assert_eq!(elements.len(), 6, "should have 2 + 3 + 1 = 6 elements");
}

/// Test simulating the exact scenario from the screenshot
/// Terminal in middle, with gnome-maps elements appearing both above and below
#[test]
fn screenshot_scenario_terminal_split_by_maps() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Internal terminal
    tc.set_terminal_height(150);

    // gnome-maps is the first external window
    // If it has elements at bad positions, they could appear split around terminal
    tc.add_window_with_elements(400, vec![
        (0, 200),   // top portion (toolbar + some map)
        (200, 200), // bottom portion (rest of map + controls)
    ]);

    // foot is the second external window
    tc.add_external_window(200);

    // Elements should render in order without splits
    let elements = tc.rendered_elements();

    // Sort by screen_y to see actual render order
    let mut sorted: Vec<_> = elements.iter().collect();
    sorted.sort_by_key(|e| e.screen_y);

    // Verify order: all gnome-maps elements should be contiguous
    // They should NOT be split around foot
    let maps_elements: Vec<_> = sorted.iter()
        .filter(|e| e.window_index == 0)
        .collect();

    if maps_elements.len() >= 2 {
        for i in 1..maps_elements.len() {
            let prev_end = maps_elements[i-1].screen_y + maps_elements[i-1].height;
            let curr_start = maps_elements[i].screen_y;

            // Elements from same window should be contiguous (no gaps from other windows)
            assert!(prev_end <= curr_start,
                "gnome-maps elements should not overlap themselves");
        }
    }

    // No inter-window overlaps
    assertions::assert_no_element_overlaps(&tc);
}

// ============================================================================
// Rendering Destination Calculation Tests
// ============================================================================

/// Test that simulates the EXACT calculation done in main.rs rendering code.
/// This verifies the window_y accumulation and dest calculation.
#[test]
fn rendering_dest_calculation_matches_expected() {
    // Simulate the exact calculation from main.rs
    let terminal_total_height: i32 = 100;
    let scroll_offset: i32 = 0;
    let window_heights: Vec<i32> = vec![300, 200]; // gnome-maps, foot

    // This mirrors the map() closure in main.rs that calculates window positions
    let mut window_y = -scroll_offset + terminal_total_height;
    let window_positions: Vec<i32> = window_heights.iter().map(|&h| {
        let y = window_y;
        window_y += h;
        y
    }).collect();

    // Verify window positions are calculated correctly
    assert_eq!(window_positions[0], 100, "gnome-maps should start at terminal_height");
    assert_eq!(window_positions[1], 400, "foot should start after gnome-maps (100 + 300)");

    // Now simulate rendering: dest.y = geo.loc.y + window_y
    // Assuming geo.loc.y = 0 for main surface elements
    let geo_loc_y = 0;

    let dest_y_window0 = geo_loc_y + window_positions[0];
    let dest_y_window1 = geo_loc_y + window_positions[1];

    assert_eq!(dest_y_window0, 100, "gnome-maps dest should be 100");
    assert_eq!(dest_y_window1, 400, "foot dest should be 400");

    // Verify no overlap
    let window0_end = dest_y_window0 + window_heights[0]; // 100 + 300 = 400
    let window1_start = dest_y_window1; // 400

    assert!(window0_end <= window1_start,
        "foot (y={}) should not overlap gnome-maps (ends at {})",
        window1_start, window0_end);
}

/// Test that catches the bug: if window_y is NOT advanced correctly,
/// subsequent windows will overlap.
#[test]
fn window_y_must_advance_by_cached_height() {
    // Bug scenario: if window_y doesn't advance, all windows render at same position
    let terminal_total_height: i32 = 100;
    let scroll_offset: i32 = 0;
    let window_heights: Vec<i32> = vec![300, 200, 150];

    // CORRECT: window_y advances by each window's height
    let mut window_y = -scroll_offset + terminal_total_height;
    let correct_positions: Vec<i32> = window_heights.iter().map(|&h| {
        let y = window_y;
        window_y += h;
        y
    }).collect();

    assert_eq!(correct_positions, vec![100, 400, 600]);

    // BUGGY: if window_y doesn't advance (e.g., using wrong variable)
    let buggy_positions: Vec<i32> = window_heights.iter().map(|_| {
        // Bug: always returns terminal_total_height, doesn't advance
        terminal_total_height
    }).collect();

    // All windows would render at same position - OVERLAP!
    assert_eq!(buggy_positions, vec![100, 100, 100]);

    // This would cause the exact symptom: foot overlapping gnome-maps
}

/// Test the specific scenario: foot overlapping gnome-maps
/// This test should FAIL if the rendering logic is buggy
#[test]
fn foot_must_render_after_gnome_maps_not_overlapping() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(100);

    // gnome-maps: large window (400px)
    tc.add_external_window(400);

    // foot: smaller window (200px)
    tc.add_external_window(200);

    let positions = tc.render_positions();

    // With Y-flip:
    // Window 0 (h=400): render_y = 720 - 0 - 400 = 320
    // Window 1 (h=200): render_y = 720 - 400 - 200 = 120
    assert_eq!(positions[0].0, 320, "gnome-maps at render Y=320");
    assert_eq!(positions[0].1, 400, "gnome-maps height should be 400");
    assert_eq!(positions[1].0, 120, "foot at render Y=120");
    assert_eq!(positions[1].1, 200, "foot height should be 200");

    // With Y-flip, windows are contiguous when: foot.end == gnome-maps.start
    let foot_end = positions[1].0 + positions[1].1; // 120 + 200 = 320
    let gnome_maps_start = positions[0].0; // 320
    assert_eq!(foot_end, gnome_maps_start, "foot should end where gnome-maps starts");
}

/// Test that mirrors EXACTLY the main.rs rendering code structure.
/// This is the definitive test for window overlap bugs.
#[test]
fn exact_main_rs_rendering_iteration_pattern() {
    // This test replicates the exact logic from main.rs lines 293-320 and 369-392

    // Mock data (simulating compositor state)
    let terminal_total_height: i32 = 100;
    let scroll_offset: f64 = 0.0;
    let cached_window_heights: Vec<i32> = vec![400, 200]; // gnome-maps, foot

    // Simulated windows (just indices for this test)
    let windows: Vec<usize> = vec![0, 1];

    // === PART 1: Build window_elements (mirrors lines 293-320) ===
    let mut window_y = -(scroll_offset as i32) + terminal_total_height;

    struct MockElements {
        window_index: usize,
        geo_loc_y: i32,
        height: i32,
    }

    let window_elements: Vec<(i32, i32, Vec<MockElements>)> = windows
        .iter()
        .zip(cached_window_heights.iter())
        .map(|(&window_index, &cached_height)| {
            let window_height = cached_height;
            let y = window_y;
            window_y += window_height;

            // Simulate elements with geo.loc.y = 0 (main surface at window origin)
            let elements = vec![MockElements {
                window_index,
                geo_loc_y: 0,
                height: cached_height,
            }];

            (y, window_height, elements)
        })
        .collect();

    // Verify positions after map()
    assert_eq!(window_elements[0].0, 100, "window 0 position");
    assert_eq!(window_elements[1].0, 500, "window 1 position");

    // === PART 2: Rendering iteration (mirrors lines 369-392) ===
    let mut rendered_dests: Vec<(usize, i32, i32)> = Vec::new();

    for (window_y, _window_height, elements) in window_elements {
        for element in elements {
            // This is the exact calculation from main.rs line 380
            let dest_y = element.geo_loc_y + window_y;

            rendered_dests.push((element.window_index, dest_y, element.height));
        }
    }

    // Verify destinations
    assert_eq!(rendered_dests.len(), 2);

    let (win0_idx, win0_dest_y, win0_h) = rendered_dests[0];
    let (win1_idx, win1_dest_y, _win1_h) = rendered_dests[1];

    assert_eq!(win0_idx, 0, "first element from window 0");
    assert_eq!(win1_idx, 1, "second element from window 1");

    assert_eq!(win0_dest_y, 100, "window 0 dest_y = geo.loc.y(0) + window_y(100)");
    assert_eq!(win1_dest_y, 500, "window 1 dest_y = geo.loc.y(0) + window_y(500)");

    // Verify no overlap
    let win0_end = win0_dest_y + win0_h;
    assert!(
        win0_end <= win1_dest_y,
        "OVERLAP BUG: window 0 ends at {} but window 1 starts at {}",
        win0_end, win1_dest_y
    );
}

/// Test what happens if cached heights don't match actual rendering
/// This could happen if heights change between caching and rendering
#[test]
fn cached_heights_mismatch_could_cause_overlap() {
    // Scenario: bbox() returns different value than what we cached

    // Cached heights (from update_cached_window_heights at frame start)
    let cached: Vec<i32> = vec![400, 200];

    // But actual rendered heights are different (bbox changed)
    let actual: Vec<i32> = vec![600, 200]; // gnome-maps grew!

    // Window positions based on CACHED heights
    let mut y = 0;
    let positions_from_cache: Vec<i32> = cached.iter().map(|&h| {
        let pos = y;
        y += h;
        pos
    }).collect();

    // Window 0 at y=0, Window 1 at y=400 (based on cached height 400)
    assert_eq!(positions_from_cache, vec![0, 400]);

    // But if window 0 actually renders with height 600...
    // Window 0: y=0 to y=600
    // Window 1: y=400 (OVERLAP! window 1 starts before window 0 ends)

    let window0_actual_end = positions_from_cache[0] + actual[0];
    let window1_start = positions_from_cache[1];

    // This demonstrates the overlap scenario
    assert!(
        window0_actual_end > window1_start,
        "This test demonstrates that cached/actual height mismatch causes overlap"
    );
}

// ============================================================================
// Surface Hit Detection Tests
// ============================================================================

/// Test: Hit detection uses DESTINATION geometry, not SOURCE texture coordinates.
/// When we flip the SOURCE during rendering, element POSITIONS remain unchanged.
/// So click detection should use relative_y = click_y - window_y (no flip needed).
#[test]
fn hit_detection_uses_destination_geometry_not_source() {
    // When rendering with flipped source:
    // - SOURCE coordinates are flipped (texture content read upside-down)
    // - DESTINATION coordinates are unchanged (element positions on screen)
    //
    // Hit detection uses destination geometry to find which element is clicked.
    // The source flip only affects texture content WITHIN each element.

    // Window at screen Y=100, height=400
    let window_y = 100.0;

    // Click at screen Y=150 (50 pixels into the window)
    let click_screen_y = 150.0;

    // Relative position for hit detection (no flip needed)
    let relative_y = click_screen_y - window_y;
    assert_eq!(relative_y, 50.0, "relative Y is simply click_y - window_y");

    // An element at geo.loc.y = 0 with height 100 covers Y range 0-100
    // relative_y = 50 is in range 0-100, so we hit the element
    let element_start = 0.0;
    let element_end = 100.0;
    let hit = relative_y >= element_start && relative_y < element_end;
    assert!(hit, "relative_y=50 should hit element at 0-100");
}

/// Test: Source flip affects texture content, not element positions
#[test]
fn source_flip_only_affects_texture_not_geometry() {
    // Consider an element:
    // - geo.loc.y = 0 (destination position in window)
    // - geo.size.h = 100 (destination size)
    // - src covers the element's texture

    // Without source flip:
    // - Texture Y=0 appears at destination Y=0
    // - Texture Y=99 appears at destination Y=99

    // With source flip:
    // - Texture Y=99 appears at destination Y=0 (content inverted)
    // - Texture Y=0 appears at destination Y=99
    // BUT the element is still AT destination Y=0 to Y=99

    // For hit detection, we only care about destination Y range
    let element_dest_start = 0;
    let element_dest_end = 100;

    // Click at destination Y=50 should hit the element
    let click_y = 50;
    let hit = click_y >= element_dest_start && click_y < element_dest_end;
    assert!(hit, "click at dest Y=50 hits element regardless of source flip");
}

// ============================================================================
// Cached vs Actual Height Mismatch Tests (FIXED BEHAVIOR)
// ============================================================================

/// FIXED: Click detection now uses actual heights, matching rendering.
/// When actual > cached, clicking at visually correct position returns correct window.
#[test]
fn click_detection_uses_actual_heights_after_fix() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Window 0: cached=200, actual=400 (element is larger than bbox reports)
    tc.add_external_window_with_mismatch(200, 400);

    // Window 1: cached=200, actual=200 (normal)
    tc.add_external_window(200);

    // With Y-flip and actual heights:
    // Window 0 (content_y=0, h=400): render_y = 720 - 0 - 400 = 320
    // Window 1 (content_y=400, h=200): render_y = 720 - 400 - 200 = 120
    let render_pos = tc.render_positions();
    assert_eq!(render_pos[0], (320, 400), "window 0 render position");
    assert_eq!(render_pos[1], (120, 200), "window 1 render position");

    // Click ranges now match render positions (render Y coordinates)
    let click_ranges = tc.window_click_ranges();
    assert_eq!(click_ranges[0], (320.0, 720.0), "window 0 click range (actual)");
    assert_eq!(click_ranges[1], (120.0, 320.0), "window 1 click range (actual)");

    // Clicking at render Y=500 (in window 0's range 320-720) should hit window 0
    let clicked_at_500 = tc.window_at(500.0);
    assert_eq!(clicked_at_500, Some(0), "render Y=500 should hit window 0");

    // Clicking at render Y=200 (in window 1's range 120-320) should hit window 1
    let clicked_at_200 = tc.window_at(200.0);
    assert_eq!(clicked_at_200, Some(1), "render Y=200 should hit window 1");
}

/// FIXED: Clicking in window 1's render range now correctly hits window 1
#[test]
fn click_in_actual_range_works_after_fix() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Window 0: cached=200, actual=400
    tc.add_external_window_with_mismatch(200, 400);

    // Window 1: cached=200, actual=200
    tc.add_external_window(200);

    // With Y-flip:
    // Window 0: render_y = 720 - 0 - 400 = 320, range 320-720
    // Window 1: render_y = 720 - 400 - 200 = 120, range 120-320
    let render_pos = tc.render_positions();
    assert_eq!(render_pos[0], (320, 400));
    assert_eq!(render_pos[1], (120, 200));

    // Click at render Y=200 (in window 1's range 120-320)
    let clicked = tc.window_at(200.0);
    assert_eq!(clicked, Some(1), "render Y=200 should hit window 1");
}

/// FIXED: Click detection matches render positions when using actual heights
#[test]
fn click_detection_matches_render_positions_after_fix() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Create mismatch between cached and actual
    tc.add_external_window_with_mismatch(200, 400); // Window 0
    tc.add_external_window(200); // Window 1

    let render_pos = tc.render_positions();
    let click_ranges = tc.window_click_ranges();

    // Now they match!
    let render_0_end = render_pos[0].0 + render_pos[0].1;
    let click_0_end = click_ranges[0].1 as i32;

    assert_eq!(render_0_end, click_0_end,
        "window 0 render end ({}) should match click end ({})",
        render_0_end, click_0_end);

    // Window 1 should also match
    let render_1_start = render_pos[1].0;
    let click_1_start = click_ranges[1].0 as i32;
    assert_eq!(render_1_start, click_1_start,
        "window 1 render start should match click start");
}

/// FIXED: Render_positions and window_click_ranges now use same heights
#[test]
fn render_and_click_ranges_match_with_actual_heights() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.add_external_window_with_mismatch(100, 300); // cached=100, actual=300
    tc.add_external_window_with_mismatch(100, 200); // cached=100, actual=200
    tc.add_external_window(150); // cached=actual=150

    // With Y-flip:
    // Window 0 (h=300): render_y = 720 - 0 - 300 = 420, range 420-720
    // Window 1 (h=200): render_y = 720 - 300 - 200 = 220, range 220-420
    // Window 2 (h=150): render_y = 720 - 500 - 150 = 70, range 70-220
    let render = tc.render_positions();
    assert_eq!(render[0], (420, 300));
    assert_eq!(render[1], (220, 200));
    assert_eq!(render[2], (70, 150));

    // Click ranges now match render positions
    let click = tc.window_click_ranges();
    assert_eq!(click[0], (420.0, 720.0));
    assert_eq!(click[1], (220.0, 420.0));
    assert_eq!(click[2], (70.0, 220.0));

    // Click at render Y=350 (in window 1's range 220-420)
    assert_eq!(tc.window_at(350.0), Some(1), "render Y=350 should hit window 1");

    // Click at render Y=500 (in window 0's range 420-720)
    assert_eq!(tc.window_at(500.0), Some(0), "render Y=500 should hit window 0");

    // Click at render Y=100 (in window 2's range 70-220)
    assert_eq!(tc.window_at(100.0), Some(2), "render Y=100 should hit window 2");
}

/// KEY TEST: The rendering code must use ACTUAL element heights to position
/// subsequent windows, not just cached heights. If element.geometry().size.h
/// is larger than cached_height, using cached_height causes overlap.
#[test]
fn must_use_actual_element_heights_for_positioning() {
    // This simulates what SHOULD happen in main.rs

    let terminal_height = 100;
    let cached_heights = [400, 200]; // What bbox() returned
    let actual_heights = [600, 200]; // What geo.size.h actually is

    // BUGGY approach: advance by cached_height
    let mut y = terminal_height;
    let buggy_positions: Vec<i32> = cached_heights.iter().map(|&h| {
        let pos = y;
        y += h;
        pos
    }).collect();
    // Window 0 at 100, Window 1 at 500

    // Check for overlap with buggy approach
    let win0_end_actual = buggy_positions[0] + actual_heights[0]; // 100 + 600 = 700
    let win1_start_buggy = buggy_positions[1]; // 500
    let has_overlap = win0_end_actual > win1_start_buggy; // 700 > 500 = OVERLAP!
    assert!(has_overlap, "Buggy approach causes overlap");

    // CORRECT approach: advance by ACTUAL height from elements
    let mut y = terminal_height;
    let correct_positions: Vec<i32> = actual_heights.iter().map(|&h| {
        let pos = y;
        y += h;
        pos
    }).collect();
    // Window 0 at 100, Window 1 at 700

    // Check no overlap with correct approach
    let win0_end = correct_positions[0] + actual_heights[0]; // 100 + 600 = 700
    let win1_start = correct_positions[1]; // 700
    assert!(win0_end <= win1_start, "Correct approach: no overlap");
}

// ============================================================================
// Frame-to-Frame State Bug Tests
// ============================================================================

/// This test demonstrates the EXACT bug in main.rs:
/// - update_cached_window_heights() at start of frame uses bbox()
/// - After rendering, we store actual heights
/// - But next frame, update_cached_window_heights() OVERWRITES actual heights with bbox again!
///
/// This causes click detection to always use bbox values, not actual element heights.
#[test]
fn bug_bbox_overwrites_actual_heights_each_frame() {
    // Simulate the main.rs frame loop bug

    struct FrameState {
        cached_window_heights: Vec<i32>,
    }

    impl FrameState {
        // Simulates update_cached_window_heights() - uses bbox()
        fn update_cached_window_heights(&mut self, bbox_heights: &[i32]) {
            self.cached_window_heights = bbox_heights.to_vec();
        }

        // Simulates storing actual heights after rendering
        fn store_actual_heights(&mut self, actual_heights: Vec<i32>) {
            self.cached_window_heights = actual_heights;
        }
    }

    let bbox_heights = vec![200, 200]; // What bbox() returns
    let actual_heights = vec![400, 200]; // What elements actually render at

    let mut state = FrameState {
        cached_window_heights: Vec::new(),
    };

    // === Frame 1 ===
    // Start of frame: update with bbox
    state.update_cached_window_heights(&bbox_heights);
    assert_eq!(state.cached_window_heights, vec![200, 200], "Frame 1 start: using bbox");

    // Input processing uses cached_window_heights (bbox values)
    // Click detection would use window 0: 0-200, window 1: 200-400

    // End of frame: store actual heights after rendering
    state.store_actual_heights(actual_heights.clone());
    assert_eq!(state.cached_window_heights, vec![400, 200], "Frame 1 end: stored actual");

    // === Frame 2 ===
    // Start of frame: update with bbox AGAIN - THIS IS THE BUG!
    state.update_cached_window_heights(&bbox_heights);

    // The actual heights we stored are GONE!
    assert_eq!(state.cached_window_heights, vec![200, 200],
        "BUG: Frame 2 start overwrote actual heights with bbox again!");

    // This means click detection in frame 2 STILL uses bbox values,
    // not the actual element heights computed in frame 1.
    // Window 0 is VISUALLY at 0-400, but click detection thinks it's at 0-200!
}

/// Test that shows the correct behavior: actual heights should persist
/// This mirrors the FIXED behavior in state.rs update_cached_window_heights()
#[test]
fn fix_actual_heights_should_persist_across_frames() {
    struct FrameState {
        cached_window_heights: Vec<i32>,
        window_count: usize,
    }

    impl FrameState {
        // FIXED version: only sync count, preserve existing heights
        fn update_cached_window_heights_fixed(&mut self, bbox_heights: &[i32]) {
            let cached_count = self.cached_window_heights.len();

            if cached_count > self.window_count {
                // Windows removed
                self.cached_window_heights.truncate(self.window_count);
            } else if cached_count < self.window_count {
                // Windows added - only append new entries from bbox
                for &h in bbox_heights.iter().skip(cached_count) {
                    self.cached_window_heights.push(h);
                }
            }
            // If counts match, preserve existing heights
        }

        fn store_actual_heights(&mut self, actual_heights: Vec<i32>) {
            self.cached_window_heights = actual_heights;
        }
    }

    let bbox_heights = vec![200, 200];
    let actual_heights = vec![400, 200];

    let mut state = FrameState {
        cached_window_heights: Vec::new(),
        window_count: 2,
    };

    // === Frame 1 ===
    state.update_cached_window_heights_fixed(&bbox_heights);
    assert_eq!(state.cached_window_heights, vec![200, 200], "Frame 1: initialized with bbox");

    state.store_actual_heights(actual_heights.clone());
    assert_eq!(state.cached_window_heights, vec![400, 200], "Frame 1: stored actual");

    // === Frame 2 ===
    state.update_cached_window_heights_fixed(&bbox_heights);

    // Actual heights should PERSIST!
    assert_eq!(state.cached_window_heights, vec![400, 200],
        "FIXED: Frame 2 preserves actual heights from frame 1");
}

/// Test coordinate calculation: relative_point should equal element's geo.loc
/// when clicking at the element's rendered screen position
#[test]
fn relative_point_matches_element_geometry() {
    // This test verifies the CRITICAL relationship:
    // If an element renders at screen_y = window_y + geo.loc.y,
    // then clicking at screen_y should give relative_y = geo.loc.y

    let terminal_height = 100;
    let window_y = terminal_height; // Window starts right after terminals
    let element_geo_loc_y = 50; // Element is 50px down from window top

    // Element renders at: screen_y = window_y + geo.loc.y = 100 + 50 = 150
    let rendered_screen_y = window_y + element_geo_loc_y;
    assert_eq!(rendered_screen_y, 150);

    // When we click at screen_y = 150:
    let click_screen_y = 150.0;
    let relative_y = click_screen_y - window_y as f64;

    // relative_y should equal geo.loc.y so surface_under finds the element
    assert_eq!(relative_y, element_geo_loc_y as f64,
        "relative_y should match element's geo.loc.y");
}

/// Test that click at element center gives correct relative point
#[test]
fn click_at_element_center_gives_correct_relative() {
    let terminal_height = 200;
    let window_0_height = 300;
    let scroll_offset = 0.0;

    // Window 0: Y=200 to Y=500 (height 300)
    // Element at geo.loc.y=0, height=300

    let window_y = terminal_height as f64 - scroll_offset;
    assert_eq!(window_y, 200.0);

    // Click at center of window 0: screen_y = 350
    let click_y = 350.0;
    let relative_y = click_y - window_y;

    // relative_y = 150, which is in the middle of the window (0-300)
    assert_eq!(relative_y, 150.0);
    assert!(relative_y >= 0.0 && relative_y < window_0_height as f64,
        "relative_y should be within window bounds");
}

/// Test with scrolling - relative point should still be correct
#[test]
fn relative_point_correct_with_scroll() {
    let terminal_height = 200;
    let window_height = 400;
    let scroll_offset = 100.0;

    // With scroll, window_y = terminal_height - scroll_offset = 100
    let window_y = terminal_height as f64 - scroll_offset;
    assert_eq!(window_y, 100.0);

    // Window is now at screen Y=100 to Y=500
    // Click at screen Y=200:
    let click_y = 200.0;
    let relative_y = click_y - window_y;

    // relative_y = 100, which means we clicked 100px into the window
    assert_eq!(relative_y, 100.0);
    assert!(relative_y >= 0.0 && relative_y < window_height as f64);
}

/// Test that window_at and relative_point calculation are consistent
#[test]
fn window_at_and_relative_point_consistent() {
    let mut tc = TestCompositor::new_headless(1280, 720);

    tc.set_terminal_height(100);
    tc.add_external_window(200); // Window 0
    tc.add_external_window(200); // Window 1

    // With Y-flip (height=720):
    // Window 0: render_y = 720 - 0 - 200 = 520, range 520-720
    // Window 1: render_y = 720 - 200 - 200 = 320, range 320-520

    // Screen coordinates (Y=0 at top):
    // Window 0: screen 0-200
    // Window 1: screen 200-400

    // Click at screen Y=100 (middle of window 0)
    // Screen Y=100 → Render Y=620 → in window 0 range (520-720)
    let screen_y = 100.0;
    let render_y = 720.0 - screen_y;
    let hit = tc.window_at(render_y);
    assert_eq!(hit, Some(0), "should hit window 0");

    // Click at screen Y=300 (middle of window 1)
    // Screen Y=300 → Render Y=420 → in window 1 range (320-520)
    let screen_y = 300.0;
    let render_y = 720.0 - screen_y;
    let hit = tc.window_at(render_y);
    assert_eq!(hit, Some(1), "should hit window 1");
}

/// Test that new windows get initialized with bbox, but existing are preserved
#[test]
fn fix_new_window_gets_bbox_existing_preserved() {
    struct FrameState {
        cached_window_heights: Vec<i32>,
        window_count: usize,
    }

    impl FrameState {
        fn update_cached_window_heights_fixed(&mut self, bbox_heights: &[i32]) {
            let cached_count = self.cached_window_heights.len();

            if cached_count > self.window_count {
                self.cached_window_heights.truncate(self.window_count);
            } else if cached_count < self.window_count {
                for &h in bbox_heights.iter().skip(cached_count) {
                    self.cached_window_heights.push(h);
                }
            }
        }

        fn store_actual_heights(&mut self, actual_heights: Vec<i32>) {
            self.cached_window_heights = actual_heights;
        }
    }

    let mut state = FrameState {
        cached_window_heights: Vec::new(),
        window_count: 1,
    };

    // === Frame 1: One window ===
    let bbox_heights_1 = vec![200];
    state.update_cached_window_heights_fixed(&bbox_heights_1);
    assert_eq!(state.cached_window_heights, vec![200]);

    // After rendering, actual height is 400
    state.store_actual_heights(vec![400]);

    // === Frame 2: Add second window ===
    state.window_count = 2;
    let bbox_heights_2 = vec![200, 150]; // Window 0 bbox still 200, new window bbox 150

    state.update_cached_window_heights_fixed(&bbox_heights_2);

    // Window 0 should keep actual height (400), window 1 gets bbox (150)
    assert_eq!(state.cached_window_heights, vec![400, 150],
        "Existing window keeps actual height, new window gets bbox");
}

// ============================================================================
// Cached Heights Insertion Tests (external window insertion)
// ============================================================================

/// BUG TEST: When inserting a cell, SET vs INSERT causes different behavior.
/// SET at index overwrites existing, INSERT shifts existing elements.
///
/// Scenario: Terminal is tall (1000px), external window (200px) inserted ABOVE it.
/// After insertion: [window(200), terminal(1000)]
///
/// With SET (buggy): cached_heights[0] = 200 overwrites terminal's height
/// With INSERT (correct): cached_heights becomes [200, 1000]
#[test]
fn bug_cached_heights_set_vs_insert_on_cell_insertion() {
    // Initial state: one tall terminal
    let cached_heights = vec![1000]; // Terminal at index 0, height 1000

    // External window inserted at index 0 (above terminal)
    // Terminal shifts to index 1

    let window_idx = 0;
    let window_height = 200;

    // BUGGY: using SET overwrites terminal's height
    let mut buggy_heights = cached_heights.clone();
    if window_idx < buggy_heights.len() {
        buggy_heights[window_idx] = window_height; // SET - WRONG!
    }
    assert_eq!(buggy_heights, vec![200],
        "BUG: SET overwrites terminal height, loses the 1000");

    // CORRECT: using INSERT preserves terminal's height
    let mut correct_heights = cached_heights.clone();
    correct_heights.insert(window_idx, window_height); // INSERT - CORRECT!
    assert_eq!(correct_heights, vec![200, 1000],
        "CORRECT: INSERT shifts terminal, preserves both heights");
}

/// Test scroll calculation with correct INSERT behavior.
/// After inserting window above tall terminal, scroll to terminal's bottom.
#[test]
fn scroll_to_terminal_bottom_after_window_insertion() {
    let screen_height = 720;

    // Initial: tall terminal at index 0
    let terminal_height = 1000;
    let mut cached_heights = vec![terminal_height];
    let focused_idx = 0;

    // External window (200px) inserted at index 0, terminal shifts to index 1
    let window_height = 200;
    cached_heights.insert(0, window_height); // CORRECT: INSERT
    let focused_idx = focused_idx + 1; // Terminal is now at index 1

    // Calculate scroll to show terminal's bottom
    let y: i32 = cached_heights.iter().take(focused_idx).sum(); // y = 200 (window above)
    let height = cached_heights[focused_idx]; // height = 1000
    let bottom_y = y + height; // bottom_y = 1200

    let total_height: i32 = cached_heights.iter().sum(); // 200 + 1000 = 1200
    let max_scroll = (total_height - screen_height).max(0); // 1200 - 720 = 480
    let min_scroll_for_bottom = (bottom_y - screen_height).max(0); // 1200 - 720 = 480

    let scroll_offset = min_scroll_for_bottom.min(max_scroll); // 480

    // Verify terminal bottom is at screen bottom
    // Content Y of terminal = y - scroll_offset = 200 - 480 = -280
    // Render Y = screen_height - content_y - height = 720 - (-280) - 1000 = 0
    // So terminal bottom is at render Y = 0 + 1000 = 1000? No wait...
    //
    // Actually with scroll_offset=480:
    // content_y starts at -scroll_offset = -480
    // Window: content_y = -480, render_y = 720 - (-480) - 200 = 1000 (off screen top)
    // Terminal: content_y = -480 + 200 = -280, render_y = 720 - (-280) - 1000 = 0
    // Terminal spans render Y 0 to 1000, visible portion is 0 to 720
    //
    // Terminal bottom at render Y=0+1000=1000, but screen only goes to 720
    // So terminal bottom is at content coordinate: y + height = 200 + 1000 = 1200
    // With scroll_offset=480, viewport shows content 480 to 1200
    // So terminal bottom (content 1200) is exactly at viewport bottom (480 + 720 = 1200)

    assert_eq!(scroll_offset, 480, "scroll should be 480 to show terminal bottom");

    // Double check: viewport_bottom = scroll_offset + screen_height = 480 + 720 = 1200
    let viewport_bottom = scroll_offset + screen_height;
    assert_eq!(viewport_bottom, bottom_y, "terminal bottom should be at viewport bottom");
}

/// BUG REPRODUCTION: Using SET instead of INSERT causes wrong scroll.
#[test]
fn bug_wrong_scroll_with_set_instead_of_insert() {
    let screen_height = 720;

    // Initial: tall terminal
    let terminal_height = 1000;
    let mut cached_heights = [terminal_height];

    // BUGGY: Use SET instead of INSERT
    let window_height = 200;
    cached_heights[0] = window_height; // SET - overwrites terminal height!

    // Now cached_heights = [200], terminal height is LOST
    let focused_idx = 1; // Terminal should be at index 1

    // Calculate scroll - but focused_idx is out of bounds!
    let y: i32 = cached_heights.iter().take(focused_idx).sum(); // y = 200
    let height = cached_heights.get(focused_idx).copied().unwrap_or(0); // NONE - returns 0!
    let bottom_y = y + height; // bottom_y = 200 (WRONG - should be 1200)

    // Scroll calculation gives wrong result
    let min_scroll_for_bottom = (bottom_y - screen_height).max(0); // (200 - 720).max(0) = 0

    assert_eq!(min_scroll_for_bottom, 0,
        "BUG: wrong scroll because terminal height was lost");
    // With scroll=0, terminal at y=200 is only partially visible (200 to 720)
    // Terminal bottom at 200+1000=1200 is NOT visible
}

// ============================================================================
// Main.rs Frame Loop Simulation Tests
// ============================================================================

/// This test simulates the ACTUAL main.rs frame loop behavior.
/// It tests that when an external window is inserted, the scroll calculation
/// correctly shows the focused terminal's bottom.
///
/// The test simulates these steps:
/// 1. Frame N: Terminal exists with tall content (1000px)
/// 2. Frame N: Wayland dispatch adds gnome-maps, sets new_external_window_index
/// 3. Frame N+1: Handle new_external_window_index, then recalculate heights
/// 4. Verify scroll shows terminal bottom
#[test]
fn frame_loop_external_window_insertion_scroll() {
    let screen_height = 720;

    // Simulate compositor state
    struct SimState {
        cells: Vec<&'static str>,  // "terminal" or "external"
        cached_cell_heights: Vec<i32>,
        focused_index: Option<usize>,
        new_external_window_index: Option<usize>,
        scroll_offset: f64,
    }

    let mut state = SimState {
        cells: vec!["terminal"],
        cached_cell_heights: vec![1000],  // Terminal is 1000px tall (from seq 1 60)
        focused_index: Some(0),
        new_external_window_index: None,
        scroll_offset: 480.0,  // Already scrolled to show terminal bottom
    };

    // === Simulate Wayland dispatch adding gnome-maps ===
    // This happens in add_window():
    let insert_index = state.focused_index.unwrap_or(state.cells.len());
    state.cells.insert(insert_index, "external");  // gnome-maps at index 0
    // Terminal is now at index 1
    state.focused_index = Some(state.focused_index.map(|i| i + 1).unwrap_or(insert_index));
    state.new_external_window_index = Some(insert_index);

    // Verify state after add_window
    assert_eq!(state.cells, vec!["external", "terminal"]);
    assert_eq!(state.focused_index, Some(1));
    assert_eq!(state.new_external_window_index, Some(0));
    // NOTE: cached_cell_heights is still [1000] - not updated yet!
    assert_eq!(state.cached_cell_heights, vec![1000]);

    // === Frame N+1: Handle new_external_window_index ===
    if let Some(window_idx) = state.new_external_window_index.take() {
        let window_height = 200;  // gnome-maps initial height

        // INSERT into cached heights
        if window_idx <= state.cached_cell_heights.len() {
            state.cached_cell_heights.insert(window_idx, window_height);
        }

        // Scroll to show focused cell
        if let Some(focused_idx) = state.focused_index {
            let y: i32 = state.cached_cell_heights.iter().take(focused_idx).sum();
            let height = state.cached_cell_heights.get(focused_idx).copied().unwrap_or(200);
            let bottom_y = y + height;
            let total_height: i32 = state.cached_cell_heights.iter().sum();
            let max_scroll = (total_height - screen_height).max(0) as f64;
            let min_scroll_for_bottom = (bottom_y - screen_height).max(0) as f64;

            state.scroll_offset = min_scroll_for_bottom.min(max_scroll);
        }
    }

    // Verify correct behavior
    assert_eq!(state.cached_cell_heights, vec![200, 1000],
        "cached heights should have window at 0, terminal at 1");

    // y = sum of heights before focused_idx (1) = 200
    // height = 1000
    // bottom_y = 1200
    // min_scroll = 1200 - 720 = 480
    assert_eq!(state.scroll_offset, 480.0,
        "scroll should be 480 to show terminal bottom");

    // Verify terminal bottom is at viewport bottom
    let terminal_y = 200;  // After window (200px)
    let terminal_bottom = terminal_y + 1000;  // 1200
    let viewport_bottom = state.scroll_offset as i32 + screen_height;  // 480 + 720 = 1200
    assert_eq!(terminal_bottom, viewport_bottom,
        "terminal bottom should be at viewport bottom");
}

/// Test the actual height recalculation that happens in main.rs
/// This tests what happens AFTER we handle new_external_window_index
#[test]
fn frame_loop_height_recalculation_after_insert() {
    // Simulate state after INSERT but before height recalculation
    let cached_cell_heights = [200, 1000];  // [window, terminal]
    let cells = ["external", "terminal"];

    // Simulate main.rs height recalculation:
    // For each cell, use cached height if available
    let new_heights: Vec<i32> = cells.iter().enumerate().map(|(i, _cell)| {
        if let Some(&cached) = cached_cell_heights.get(i) {
            if cached > 0 {
                return cached;
            }
        }
        200  // default
    }).collect();

    // Heights should be preserved
    assert_eq!(new_heights, vec![200, 1000],
        "height recalculation should preserve existing heights");
}

/// THE EXACT BUG SCENARIO:
/// 1. User runs `seq 1 60` → command terminal grows tall
/// 2. Command exits → command terminal stays, parent is unhidden
/// 3. User runs `gnome-maps` → inserted between command and parent
/// 4. Scroll should show parent terminal bottom
///
/// cells: [command_terminal (tall, 1000px), parent_terminal (small, 200px)]
/// After gnome-maps: [command (1000px), gnome-maps (200px), parent (200px)]
/// Focus is on parent (index 2)
/// Scroll should show parent bottom (y=1400)
#[test]
fn exact_bug_scenario_seq_then_gnome_maps() {
    let screen_height = 720;

    // Initial state after `seq 1 60` finishes:
    // - Command terminal at index 0 (tall, 1000px from seq output)
    // - Parent terminal at index 1 (small, 200px)
    // - Focus is on parent (index 1)
    struct SimState {
        cells: Vec<&'static str>,
        cached_cell_heights: Vec<i32>,
        focused_index: Option<usize>,
        new_external_window_index: Option<usize>,
        scroll_offset: f64,
    }

    let mut state = SimState {
        cells: vec!["command_terminal", "parent_terminal"],
        cached_cell_heights: vec![1000, 200],  // command is tall, parent is small
        focused_index: Some(1),  // Focus on parent
        new_external_window_index: None,
        scroll_offset: 480.0,  // Scrolled to show parent bottom
    };

    // === User launches gnome-maps (via termstack from parent) ===
    // add_window inserts at focused_index
    let insert_index = state.focused_index.unwrap_or(state.cells.len());  // 1
    state.cells.insert(insert_index, "gnome-maps");  // Insert at index 1
    // Now: [command_terminal, gnome-maps, parent_terminal]
    state.focused_index = Some(state.focused_index.map(|i| i + 1).unwrap_or(insert_index));  // 2
    state.new_external_window_index = Some(insert_index);  // 1

    // Verify state after add_window
    assert_eq!(state.cells, vec!["command_terminal", "gnome-maps", "parent_terminal"]);
    assert_eq!(state.focused_index, Some(2));  // Parent is now at index 2
    assert_eq!(state.new_external_window_index, Some(1));  // gnome-maps was inserted at 1
    // cached_cell_heights is still [1000, 200] - NOT updated yet!

    // === Handle new_external_window_index ===
    if let Some(window_idx) = state.new_external_window_index.take() {
        let window_height = 200;  // gnome-maps initial height

        // INSERT into cached heights
        state.cached_cell_heights.insert(window_idx, window_height);

        // Now cached_cell_heights = [1000, 200, 200]
        // Indices: command=0, gnome-maps=1, parent=2

        // Scroll to show focused cell (parent at index 2)
        if let Some(focused_idx) = state.focused_index {
            let y: i32 = state.cached_cell_heights.iter().take(focused_idx).sum();
            let height = state.cached_cell_heights.get(focused_idx).copied().unwrap_or(200);
            let bottom_y = y + height;
            let total_height: i32 = state.cached_cell_heights.iter().sum();
            let max_scroll = (total_height - screen_height).max(0) as f64;
            let min_scroll_for_bottom = (bottom_y - screen_height).max(0) as f64;

            state.scroll_offset = min_scroll_for_bottom.min(max_scroll);

            // Debug output
            eprintln!("focused_idx={}, y={}, height={}, bottom_y={}, total_height={}, max_scroll={}, min_scroll={}, scroll={}",
                focused_idx, y, height, bottom_y, total_height, max_scroll, min_scroll_for_bottom, state.scroll_offset);
        }
    }

    // Verify cached heights after INSERT
    assert_eq!(state.cached_cell_heights, vec![1000, 200, 200],
        "cached heights should be [command=1000, gnome-maps=200, parent=200]");

    // Calculate expected scroll:
    // y = 1000 + 200 = 1200 (sum of heights before parent)
    // height = 200 (parent height)
    // bottom_y = 1400
    // total_height = 1400
    // max_scroll = 1400 - 720 = 680
    // min_scroll_for_bottom = 1400 - 720 = 680
    // scroll = 680

    assert_eq!(state.scroll_offset, 680.0,
        "scroll should be 680 to show parent terminal bottom");

    // Verify parent bottom is at viewport bottom
    let parent_y = 1000 + 200;  // After command (1000) and gnome-maps (200)
    let parent_bottom = parent_y + 200;  // 1400
    let viewport_bottom = state.scroll_offset as i32 + screen_height;  // 680 + 720 = 1400
    assert_eq!(parent_bottom, viewport_bottom,
        "parent terminal bottom should be at viewport bottom");
}

/// THE ACTUAL BUG: External window resize without scroll update.
///
/// When gnome-maps first appears, it's 200px and we scroll correctly.
/// But then gnome-maps resizes to 400px (or more), pushing the parent
/// terminal down by 200px. The scroll_offset doesn't update, so the
/// parent terminal is now only partially visible.
///
/// This test documents the bug - it shows current (buggy) behavior
/// and what the correct behavior should be.
#[test]
fn bug_external_window_resize_without_scroll_update() {
    let screen_height = 720;

    // State after gnome-maps is added and initial scroll is set
    // cells: [command (1000), gnome-maps (200), parent (200)]
    // scroll = 680 to show parent bottom
    let mut cached_cell_heights = [1000, 200, 200];
    let focused_index = 2;  // Parent terminal
    let scroll_offset = 680.0;

    // Verify initial scroll is correct
    let parent_y = 1000 + 200;  // 1200
    let parent_bottom = parent_y + 200;  // 1400
    let viewport_bottom = scroll_offset as i32 + screen_height;  // 680 + 720 = 1400
    assert_eq!(parent_bottom, viewport_bottom, "initial scroll correct");

    // === gnome-maps resizes from 200 to 400 ===
    // This happens in handle_commit
    let new_gnome_maps_height = 400;
    cached_cell_heights[1] = new_gnome_maps_height;  // gnome-maps at index 1

    // BUG: scroll_offset is NOT updated in current code!
    // The layout is recalculated, but scroll stays at 680

    // Now let's check if parent bottom is still at viewport bottom
    let parent_y_after = 1000 + 400;  // 1400 (gnome-maps is now 400)
    let parent_bottom_after = parent_y_after + 200;  // 1600
    let viewport_bottom_after = scroll_offset as i32 + screen_height;  // 680 + 720 = 1400

    // Parent bottom (1600) is NOT at viewport bottom (1400)!
    // This is the bug - parent terminal is pushed off-screen
    assert_ne!(parent_bottom_after, viewport_bottom_after,
        "BUG: parent bottom ({}) != viewport bottom ({}) after gnome-maps resize",
        parent_bottom_after, viewport_bottom_after);

    // To verify: where is gnome-maps now relative to screen?
    // gnome-maps is at content y = 1000, height = 400
    // With scroll = 680, visible content is 680 to 1400
    // gnome-maps occupies content y 1000-1400, which is mostly visible
    // Screen y of gnome-maps = 1000 - 680 = 320 (from top)
    // That means gnome-maps bottom is at screen y 320 + 400 = 720 (screen bottom)
    // And gnome-maps middle is at screen y 320 + 200 = 520
    // This matches "scrolls only down to the middle of gnome-maps"!
    let gnome_maps_content_y = 1000;
    let gnome_maps_screen_top = gnome_maps_content_y - scroll_offset as i32;  // 320
    let gnome_maps_screen_bottom = gnome_maps_screen_top + new_gnome_maps_height;  // 720
    assert_eq!(gnome_maps_screen_bottom, screen_height,
        "gnome-maps bottom at screen bottom = we're at 'middle' of gnome-maps");

    // The FIX: scroll should be updated when external window resizes
    let total_height: i32 = cached_cell_heights.iter().sum();  // 1600
    let max_scroll = (total_height - screen_height).max(0) as f64;  // 880
    let y: i32 = cached_cell_heights.iter().take(focused_index).sum();  // 1400
    let height = cached_cell_heights[focused_index];  // 200
    let bottom_y = y + height;  // 1600
    let min_scroll_for_bottom = (bottom_y - screen_height).max(0) as f64;  // 880

    let correct_scroll = min_scroll_for_bottom.min(max_scroll);  // 880
    assert_eq!(correct_scroll, 880.0, "correct scroll should be 880 to show parent bottom");
}

/// This test verifies the fix for external window resize scroll adjustment.
/// When an external window resizes, if the focused cell is at or after the
/// resized window, scroll is updated to keep the focused cell visible.
#[test]
fn fix_external_window_resize_updates_scroll() {
    let screen_height = 720;

    // Initial state after gnome-maps added
    // cells: [command (1000), gnome-maps (200), parent (200)]
    let mut cached_cell_heights = [1000, 200, 200];
    let focused_index = 2;  // Parent terminal
    let mut scroll_offset = 680.0;

    // Verify initial scroll is correct
    let parent_y = 1000 + 200;
    let parent_bottom = parent_y + 200;
    let viewport_bottom = scroll_offset as i32 + screen_height;
    assert_eq!(parent_bottom, viewport_bottom, "initial scroll correct");

    // === gnome-maps resizes from 200 to 400 ===
    // This triggers external_window_resized in handle_commit
    let resized_idx = 1;  // gnome-maps
    let new_height = 400;

    // Update cached height (as main.rs does)
    cached_cell_heights[resized_idx] = new_height;

    // Apply the fix logic from main.rs:
    // If focused_index >= resized_idx, update scroll to show focused cell
    if focused_index >= resized_idx {
        let y: i32 = cached_cell_heights.iter().take(focused_index).sum();
        let height = cached_cell_heights.get(focused_index).copied().unwrap_or(200);
        let bottom_y = y + height;
        let visible_height = screen_height;
        let total_height: i32 = cached_cell_heights.iter().sum();
        let max_scroll = (total_height - visible_height).max(0) as f64;
        let min_scroll_for_bottom = (bottom_y - visible_height).max(0) as f64;

        scroll_offset = min_scroll_for_bottom.min(max_scroll);
    }

    // After the fix: scroll should now be 880
    assert_eq!(scroll_offset, 880.0, "scroll updated after external window resize");

    // Verify parent is now visible at bottom of screen
    let parent_y_after = 1000 + 400;  // gnome-maps is now 400
    let parent_bottom_after = parent_y_after + 200;  // 1600
    let viewport_bottom_after = scroll_offset as i32 + screen_height;  // 880 + 720 = 1600
    assert_eq!(parent_bottom_after, viewport_bottom_after,
        "parent bottom now visible at viewport bottom");
}

/// Test positioning with extremely small window (1px height)
#[test]
fn single_pixel_height_window() {
    let mut tc = TestCompositor::new_headless(800, 600);
    tc.add_external_window(1);  // 1px height window

    let positions = tc.render_positions();
    assert_eq!(positions.len(), 1);

    // Should be positioned at top of screen despite being tiny
    assert_eq!(positions[0].1, 1, "1px window should report 1px height");

    // Should be clickable despite being tiny
    tc.simulate_click(400.0, 0.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "1px window should be focusable");
}

/// Test positioning when window height exceeds screen height
#[test]
fn window_height_exceeds_screen() {
    let mut tc = TestCompositor::new_headless(800, 600);
    tc.add_external_window(1000);  // Window taller than 600px screen

    let positions = tc.render_positions();
    assert_eq!(positions.len(), 1);
    assert_eq!(positions[0].1, 1000, "tall window should report full height");

    // Window should be clickable at top
    tc.simulate_click(400.0, 0.0);
    assert_eq!(tc.snapshot().focused_index, Some(0), "tall window clickable at top");

    // Scroll to bottom to see rest of window
    tc.simulate_scroll(500.0);
    assert_eq!(tc.scroll_offset(), 400.0, "max scroll = 1000 - 600 = 400");

    // Should still be focused after scrolling
    assert_eq!(tc.snapshot().focused_index, Some(0), "focus persists after scroll");
}

/// Test positioning when all windows are scrolled offscreen
#[test]
fn all_windows_offscreen_after_scroll() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add windows at top (total 400px of content)
    tc.add_external_window(200);
    tc.add_external_window(200);

    // Scroll past all content (max scroll would be 0 since 400 < 600)
    tc.simulate_scroll(1000.0);

    // Should clamp to 0 (can't scroll when content fits in viewport)
    assert_eq!(tc.scroll_offset(), 0.0, "scroll clamped when content < viewport");

    // Now make content taller than viewport
    tc.add_external_window(500);  // Now total = 900px, viewport = 600px

    // Scroll to max
    tc.simulate_scroll(1000.0);
    assert_eq!(tc.scroll_offset(), 300.0, "max scroll = 900 - 600 = 300");

    // All windows are partially or fully offscreen
    // First window (200px) is at content_y=0, completely above viewport (viewport starts at scroll=300)
    // Second window (200px) is at content_y=200, completely above viewport
    // Third window (500px) is at content_y=400, partially visible (100px showing at bottom)

    let positions = tc.render_positions();

    // With scroll=300, render positions (Y=0 at bottom):
    // Window 0: content_y=0 -> render_y = 600 - (0-300) - 200 = 700 (offscreen above)
    // Window 1: content_y=200 -> render_y = 600 - (200-300) - 200 = 500
    // Window 2: content_y=400 -> render_y = 600 - (400-300) - 500 = 0

    assert_eq!(positions[0].0, 700, "window 0 rendered above screen");
    assert_eq!(positions[1].0, 500, "window 1 partially visible at top");
    assert_eq!(positions[2].0, 0, "window 2 at bottom of screen");
}
