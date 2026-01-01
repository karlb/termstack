//! Tests for popup behavior
//!
//! These tests verify popup positioning, grab state tracking,
//! and basic popup lifecycle in the compositor.

use test_harness::TestCompositor;

/// Verify that popups are positioned relative to their parent window
#[test]
fn popup_positioned_relative_to_parent() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add a parent window
    tc.add_external_window(300);

    // Add a popup at offset (50, 100) from parent
    let popup_id = tc.add_popup(0, (50, 100), (200, 150));

    // Get popup screen position
    let (px, py) = tc.popup_screen_position(popup_id).unwrap();

    // Parent is at top of screen (render Y = 600 - 0 - 300 = 300)
    // Popup offset Y=100 relative to parent top means popup Y = 300 + 100 = 400
    assert_eq!(px, 50, "popup X should match offset");
    assert_eq!(py, 400, "popup Y should be parent_y + offset_y");
}

/// Verify that popup position updates when parent window is scrolled
#[test]
fn popup_position_updates_with_scroll() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add windows to create scrollable content
    tc.add_external_window(400);
    tc.add_external_window(400);

    // Add popup to first window
    let popup_id = tc.add_popup(0, (50, 50), (200, 150));

    let (_, before_scroll_y) = tc.popup_screen_position(popup_id).unwrap();

    // Scroll down
    tc.set_scroll(100.0);

    let (_, after_scroll_y) = tc.popup_screen_position(popup_id).unwrap();

    // After scrolling down 100px, popup render Y should increase by 100
    // (parent moved up in content space, so render Y increases)
    assert_eq!(
        after_scroll_y - before_scroll_y,
        100,
        "popup should move with parent when scrolling"
    );
}

/// Verify popup grab state can be set and queried
#[test]
fn popup_grab_state_tracking() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add parent and popup
    tc.add_external_window(300);
    let popup_id = tc.add_popup(0, (50, 50), (200, 150));

    // Initially no grab
    assert!(!tc.has_popup_grab(), "should have no grab initially");
    assert!(!tc.popups()[popup_id].has_grab, "popup should not have grab initially");

    // Set grab
    tc.set_popup_grab(popup_id, true);

    assert!(tc.has_popup_grab(), "should have active popup grab");
    assert!(tc.popups()[popup_id].has_grab, "popup should have grab");

    // Clear grab
    tc.set_popup_grab(popup_id, false);

    assert!(!tc.has_popup_grab(), "should have no grab after clearing");
}

/// Verify popup_at() correctly identifies popup under pointer
#[test]
fn popup_hit_detection() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add parent window
    tc.add_external_window(300);

    // Add popup at offset (100, 50) with size (200, 150)
    let popup_id = tc.add_popup(0, (100, 50), (200, 150));

    // Get popup screen position
    let (px, py) = tc.popup_screen_position(popup_id).unwrap();

    // Click inside popup
    let inside_x = px + 50;
    let inside_y = py + 50;
    assert_eq!(
        tc.popup_at(inside_x, inside_y),
        Some(popup_id),
        "should detect popup when clicking inside"
    );

    // Click outside popup
    let outside_x = px - 10;
    let outside_y = py - 10;
    assert_eq!(
        tc.popup_at(outside_x, outside_y),
        None,
        "should not detect popup when clicking outside"
    );
}

/// Verify nested popups: topmost popup takes precedence
#[test]
fn nested_popup_detection() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add parent window
    tc.add_external_window(400);

    // Add first popup (larger, underneath)
    let popup1 = tc.add_popup(0, (50, 50), (300, 200));

    // Add second popup (smaller, on top, overlapping)
    let popup2 = tc.add_popup(0, (100, 100), (100, 100));

    // Get positions
    let (px1, py1) = tc.popup_screen_position(popup1).unwrap();
    let (px2, py2) = tc.popup_screen_position(popup2).unwrap();

    // Click in overlapping area - should hit popup2 (on top)
    let overlap_x = px2 + 10;
    let overlap_y = py2 + 10;
    assert_eq!(
        tc.popup_at(overlap_x, overlap_y),
        Some(popup2),
        "should detect topmost popup in overlapping area"
    );

    // Click in popup1-only area (outside popup2)
    let popup1_only_x = px1 + 10;
    let popup1_only_y = py1 + 10;
    assert_eq!(
        tc.popup_at(popup1_only_x, popup1_only_y),
        Some(popup1),
        "should detect popup1 when not overlapped"
    );
}

/// Verify popup can be removed
#[test]
fn popup_removal() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add parent and popup
    tc.add_external_window(300);
    let popup_id = tc.add_popup(0, (50, 50), (200, 150));

    assert_eq!(tc.popups().len(), 1, "should have one popup");

    // Get position before removal for hit testing
    let (px, py) = tc.popup_screen_position(popup_id).unwrap();

    // Remove popup
    tc.remove_popup(popup_id);

    assert_eq!(tc.popups().len(), 0, "should have no popups after removal");

    // Should not detect popup at old position
    assert_eq!(
        tc.popup_at(px + 10, py + 10),
        None,
        "should not detect removed popup"
    );
}

/// Verify multiple popups on different windows
#[test]
fn popups_on_different_windows() {
    let mut tc = TestCompositor::new_headless(800, 600);

    // Add two windows
    tc.add_external_window(250);
    tc.add_external_window(250);

    // Add popup to each window
    let popup1 = tc.add_popup(0, (50, 50), (150, 100));
    let popup2 = tc.add_popup(1, (50, 50), (150, 100));

    // Get positions
    let (_, py1) = tc.popup_screen_position(popup1).unwrap();
    let (_, py2) = tc.popup_screen_position(popup2).unwrap();

    // Popups should be at different Y positions (different parents)
    assert_ne!(py1, py2, "popups on different windows should have different positions");

    // Each popup should be detectable at its position
    assert_eq!(tc.popup_at(100, py1 + 50), Some(popup1));
    assert_eq!(tc.popup_at(100, py2 + 50), Some(popup2));
}
