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

    // Window 0 (gnome-maps) should be at Y=0
    // Window 1 (foot) should be at Y=400, NOT Y=0
    assert_eq!(render_pos[0].0, 0, "window 0 should start at Y=0");
    assert_eq!(render_pos[0].1, 400, "window 0 should have height 400");
    assert_eq!(render_pos[1].0, 400, "window 1 should start at Y=400 (after window 0)");
    assert_eq!(render_pos[1].1, 200, "window 1 should have height 200");

    // They should not overlap
    let window_0_end = render_pos[0].0 + render_pos[0].1;
    let window_1_start = render_pos[1].0;
    assert!(window_0_end <= window_1_start,
        "windows overlap: window 0 ends at {}, window 1 starts at {}",
        window_0_end, window_1_start);
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

    // Windows should start after terminals
    assert_eq!(render_pos[0].0, 100, "first window should start at terminal height");
    assert_eq!(render_pos[1].0, 300, "second window should start after first");

    // Click detection should also account for terminal height
    assertions::assert_click_at_y_hits_window(&tc, 50.0, None); // On terminal area
    assertions::assert_click_at_y_hits_window(&tc, 150.0, Some(0)); // On window 0
    assertions::assert_click_at_y_hits_window(&tc, 350.0, Some(1)); // On window 1
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

    // Even with height 0, positions should be calculated
    // (In real code, we'd use fallback height, but this tests edge case)
    assert_eq!(render_pos[1].0, 0, "window 1 starts at window 0's end (0)");
}

#[test]
fn changing_window_height_updates_positions() {
    let mut tc = TestCompositor::new_headless(fixtures::TEST_WIDTH, fixtures::TEST_HEIGHT);

    tc.add_external_window(200);
    tc.add_external_window(200);

    // Initial: window 1 at Y=200
    assert_eq!(tc.render_positions()[1].0, 200);

    // Resize window 0 to 400 pixels
    tc.set_window_height(0, 400);

    // Window 1 should now be at Y=400
    assert_eq!(tc.render_positions()[1].0, 400);

    // Click detection should also update
    assertions::assert_click_at_y_hits_window(&tc, 250.0, Some(0)); // Now within enlarged window 0
    assertions::assert_click_at_y_hits_window(&tc, 450.0, Some(1)); // Window 1 moved down
}
