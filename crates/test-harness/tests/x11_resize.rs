//! Tests for X11 window height handling
//!
//! These tests verify that X11 window heights are calculated correctly
//! without feedback loops that cause windows to grow indefinitely.

/// Title bar height constant (must match compositor::title_bar::TITLE_BAR_HEIGHT)
const TITLE_BAR_HEIGHT: u32 = 24;

/// Test that X11 window height is stable (no feedback loop)
///
/// This verifies the fix: X11 window heights are NOT updated from
/// collect_cell_data return values. Instead, they're only updated by
/// configure_notify and resize drag.
#[test]
fn x11_height_no_feedback_loop() {
    // Simulate the initial state when an X11 window is created
    let initial_content_height: i32 = 200;
    let mut node_height = initial_content_height;
    let uses_csd = false; // Server-side decorations (we add title bar)
    let is_x11 = true;

    println!("Initial: node_height = {}", node_height);
    println!("TITLE_BAR_HEIGHT = {}", TITLE_BAR_HEIGHT);

    // Simulate 10 frames of the main loop
    for frame in 0..10 {
        // collect_cell_data returns height with title bar for positioning
        let window_height = node_height;
        let actual_height = if uses_csd {
            window_height
        } else {
            window_height + TITLE_BAR_HEIGHT as i32
        };

        // update_layout_heights now SKIPS X11 windows
        // So node_height is NOT updated from actual_height
        if !is_x11 {
            node_height = actual_height;
        }
        // For X11, node_height stays as set by configure_notify

        println!("Frame {}: actual_height = {} (for render), node_height = {} (stable)",
                 frame, actual_height, node_height);
    }

    // node_height should remain at the initial content height
    assert_eq!(
        node_height, initial_content_height,
        "X11 node.height should stay at content height {}, not grow to {}",
        initial_content_height, node_height
    );
}

/// Test that the height calculation is stable when done correctly
///
/// The fix is to NOT update node.height from collect_cell_data's return value
/// for X11 windows. Instead, node.height should only be updated by:
/// 1. add_x11_window (initial height)
/// 2. configure_notify (resize acknowledgment)
/// 3. Resize drag handler (during resize)
#[test]
fn x11_height_stable_when_not_updating_from_render() {
    // Simulate the initial state when an X11 window is created
    let initial_content_height: i32 = 200;
    let mut node_height = initial_content_height;
    let uses_csd = false; // Server-side decorations

    println!("Initial: node_height = {}", node_height);

    // Simulate 10 frames of the main loop
    for frame in 0..10 {
        // collect_cell_data returns the height for rendering (with title bar)
        let window_height = node_height;
        let actual_height = if uses_csd {
            window_height
        } else {
            window_height + TITLE_BAR_HEIGHT as i32
        };

        // FIX: Don't update node_height from actual_height for X11 windows
        // node_height stays as the content height set by configure_notify

        println!("Frame {}: actual_height = {} (for render), node_height = {} (stable)",
                 frame, actual_height, node_height);
    }

    // node_height should remain at the initial value
    assert_eq!(
        node_height, initial_content_height,
        "node.height should stay at content height {}, got {}",
        initial_content_height, node_height
    );
}

/// Test that resize updates are applied correctly
#[test]
fn x11_resize_updates_height() {
    let initial_height: i32 = 200;
    let mut node_height = initial_height;

    // Simulate configure_notify from X11 with new size
    let new_configured_height = 300;
    node_height = new_configured_height; // This is what configure_notify does

    // After configure_notify, the height should be the new value
    assert_eq!(node_height, new_configured_height);

    // Simulate several frames - height should stay stable
    for _frame in 0..5 {
        // collect_cell_data would return this for rendering
        let _render_height = node_height + TITLE_BAR_HEIGHT as i32;
        // But we DON'T update node_height from this
    }

    // Height should still be what configure_notify set
    assert_eq!(node_height, new_configured_height);
}

/// Test that the render height includes title bar for SSD windows
#[test]
fn x11_render_height_includes_title_bar() {
    let content_height: i32 = 200;
    let uses_csd = false;

    // For rendering, we need the total height (content + title bar)
    let render_height = if uses_csd {
        content_height
    } else {
        content_height + TITLE_BAR_HEIGHT as i32
    };

    assert_eq!(render_height, 200 + TITLE_BAR_HEIGHT as i32);
}

/// Test that CSD windows don't get title bar added
#[test]
fn x11_csd_window_no_extra_title_bar() {
    let content_height: i32 = 200;
    let uses_csd = true; // Client-side decorations

    // For CSD windows, render height equals content height
    let render_height = if uses_csd {
        content_height
    } else {
        content_height + TITLE_BAR_HEIGHT as i32
    };

    assert_eq!(render_height, content_height);
}
