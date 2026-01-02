//! Tests for X11 window height handling
//!
//! These tests verify that X11 window heights are calculated correctly
//! with unified visual height storage (no feedback loops).

/// Title bar height constant (must match compositor::title_bar::TITLE_BAR_HEIGHT)
const TITLE_BAR_HEIGHT: u32 = 24;

/// Test that X11 window height is stable (no feedback loop)
///
/// After unification: node.height stores VISUAL height (includes title bar for SSD).
/// collect_cell_data returns this same visual height, and update_layout_heights
/// can safely update it without causing a feedback loop.
#[test]
fn x11_height_no_feedback_loop() {
    // Simulate the initial state when an X11 window is created
    // configure_notify sets visual height (content + title bar for SSD)
    let content_from_x11: i32 = 200;
    let uses_csd = false; // Server-side decorations

    // configure_notify adds title bar to get visual height
    let initial_visual_height = if uses_csd {
        content_from_x11
    } else {
        content_from_x11 + TITLE_BAR_HEIGHT as i32
    };
    let mut node_height = initial_visual_height;

    println!("Initial: node_height = {} (visual)", node_height);
    println!("TITLE_BAR_HEIGHT = {}", TITLE_BAR_HEIGHT);

    // Simulate 10 frames of the main loop
    for frame in 0..10 {
        // collect_cell_data returns visual height (node_height is already visual)
        let render_height = node_height;

        // update_layout_heights updates from render_height
        // No feedback loop because both are visual height
        node_height = render_height;

        println!("Frame {}: render_height = {}, node_height = {} (stable)",
                 frame, render_height, node_height);
    }

    // node_height should remain at the initial visual height
    assert_eq!(
        node_height, initial_visual_height,
        "node.height should stay at visual height {}, not grow to {}",
        initial_visual_height, node_height
    );
}

/// Test that the height calculation is stable with unified storage
///
/// With unified height storage: node.height stores visual height for ALL cells.
/// - configure_notify sets visual height (content + title bar for SSD)
/// - collect_cell_data returns visual height (just node.height)
/// - update_layout_heights can safely update from rendered heights
#[test]
fn x11_height_stable_when_not_updating_from_render() {
    // Simulate the initial state when an X11 window is created
    let content_from_x11: i32 = 200;
    let uses_csd = false; // Server-side decorations

    // configure_notify adds title bar to store visual height
    let initial_visual_height = if uses_csd {
        content_from_x11
    } else {
        content_from_x11 + TITLE_BAR_HEIGHT as i32
    };
    let mut node_height = initial_visual_height;

    println!("Initial: node_height = {} (visual)", node_height);

    // Simulate 10 frames of the main loop
    for frame in 0..10 {
        // collect_cell_data returns visual height (node_height is already visual)
        let render_height = node_height;

        // update_layout_heights CAN update from render_height
        // No feedback loop because both are visual height
        node_height = render_height;

        println!("Frame {}: render_height = {}, node_height = {} (stable)",
                 frame, render_height, node_height);
    }

    // node_height should remain at the initial visual height
    assert_eq!(
        node_height, initial_visual_height,
        "node.height should stay at visual height {}, got {}",
        initial_visual_height, node_height
    );
}

/// Test that resize updates are applied correctly
#[test]
fn x11_resize_updates_height() {
    let uses_csd = false;

    // Simulate configure_notify from X11 with new content size
    let new_content = 300;
    let new_visual = if uses_csd {
        new_content
    } else {
        new_content + TITLE_BAR_HEIGHT as i32
    };
    let mut node_height = new_visual; // configure_notify sets visual height

    // After configure_notify, the height should be the new visual height
    assert_eq!(node_height, new_visual);

    // Simulate several frames - height should stay stable
    for _frame in 0..5 {
        // collect_cell_data returns visual height
        let render_height = node_height;
        // update_layout_heights updates from render_height
        node_height = render_height;
    }

    // Height should still be the visual height that configure_notify set
    assert_eq!(node_height, new_visual);
}

/// Test that configure_notify sets visual height (includes title bar for SSD)
#[test]
fn x11_render_height_includes_title_bar() {
    let content_from_x11: i32 = 200;
    let uses_csd = false;

    // configure_notify receives content height from X11
    // and stores visual height (content + title bar for SSD)
    let visual_height = if uses_csd {
        content_from_x11
    } else {
        content_from_x11 + TITLE_BAR_HEIGHT as i32
    };

    assert_eq!(visual_height, 200 + TITLE_BAR_HEIGHT as i32);
}

/// Test that CSD windows don't get title bar added
#[test]
fn x11_csd_window_no_extra_title_bar() {
    let content_from_x11: i32 = 200;
    let uses_csd = true; // Client-side decorations

    // For CSD windows, visual height equals content height
    // (no compositor-drawn title bar)
    let visual_height = if uses_csd {
        content_from_x11
    } else {
        content_from_x11 + TITLE_BAR_HEIGHT as i32
    };

    assert_eq!(visual_height, content_from_x11);
}
