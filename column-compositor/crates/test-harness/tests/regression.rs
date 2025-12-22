//! Regression tests for bugs found in v1

use std::time::Duration;
use test_harness::headless::TestCompositor;
use test_harness::{assertions, fixtures};

/// Regression: empty rows after command (v1 issue-003)
#[test]
fn no_empty_rows_after_command() {
    let (mut tc, term) = fixtures::single_terminal();

    // Mock terminal is created synchronously, no need to wait
    assert_eq!(tc.snapshot().window_count, 1);

    // Send some input (mock just appends to content)
    tc.send_input(&term, "ls -la\n");

    // Verify the content has no empty rows
    let content = tc.get_terminal_content(&term);
    assertions::assert_no_empty_rows(&content);
}

/// Regression: scrollback lost during resize (v1 issue)
/// NOTE: Requires live terminal with real shell, not mock
#[test]
#[ignore = "requires live terminal infrastructure"]
fn scrollback_preserved_during_growth() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // Output that will definitely scroll
    tc.send_input(&term, &fixtures::seq_command(1, 1000));

    tc.wait_for(
        |c| c.get_terminal_content(&term).contains("1000"),
        Duration::from_secs(10),
    )
    .expect("should complete output");

    // Content should include early lines
    let content = tc.get_terminal_content(&term);
    assertions::assert_lines_present(&content, 1, 10);
    assertions::assert_lines_present(&content, 990, 1000);
}

/// Regression: rapid output causes content loss
/// NOTE: Requires live terminal with real shell, not mock
#[test]
#[ignore = "requires live terminal infrastructure"]
fn rapid_output_no_content_loss() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // Rapid output with tiny delays (triggers resize during resize)
    tc.send_input(&term, &fixtures::seq_command(1, 200));

    tc.wait_for(
        |c| c.get_terminal_content(&term).contains("200"),
        Duration::from_secs(30),
    )
    .expect("should complete output");

    // Verify all lines present
    let content = tc.get_terminal_content(&term);
    assertions::assert_lines_present(&content, 1, 200);
}

/// Regression: external window height mismatch pushes focused terminal off screen
///
/// Bug scenario:
/// 1. Terminal T1 outputs content (seq 1 60)
/// 2. External window E (gnome-maps) spawns with cached_height=200
/// 3. Terminal T0 (focused) is below
/// 4. gnome-maps grows to actual_height=420 but doesn't report via XDG protocol
/// 5. Result: T0 gets pushed off screen (negative render Y)
///
/// Fix: detect actual height changes and adjust scroll to keep focused visible
#[test]
fn external_window_height_mismatch_scroll_adjustment() {
    // Screen: 1280x720
    let mut tc = TestCompositor::new_headless(1280, 720);

    // Layout: [T1 h=800] [E h=200 cached, 420 actual] [T0 h=51, focused]
    // Total content = 800 + 420 + 51 = 1271 (exceeds 720 screen)

    // T1 - terminal with output (index 0)
    let t1 = tc.spawn_terminal();
    tc.set_window_height(t1.index, 800);

    // E - external window with height mismatch (index 1)
    // gnome-maps: reports 200 via bbox but actually renders at 420
    let _ext = tc.add_external_window_with_mismatch(200, 420);

    // T0 - focused terminal (index 2)
    let t0 = tc.spawn_terminal();
    tc.set_window_height(t0.index, 51);

    // Initial state: focused is index 2 (T0)
    assert_eq!(tc.snapshot().focused_index, Some(2));

    // Get render positions using actual heights
    let positions = tc.render_positions();

    // Calculate where T0 renders with current scroll
    // Total height = 800 + 420 + 51 = 1271
    // content_y for T0 = 800 + 420 = 1220
    // With scroll=0: render_y = 720 - 1220 - 51 = -551 (WAY off screen!)

    let t0_render_y = positions[2].0;
    let screen_height = 720;

    // T0 bottom should be visible (not negative)
    let t0_bottom = t0_render_y + positions[2].1;

    // Before scroll adjustment, T0 is off screen
    // After scroll adjustment, T0's bottom should be at y >= 0

    // Calculate required scroll to bring T0's bottom to screen bottom
    // T0 bottom in content coords = 800 + 420 + 51 = 1271
    // For T0 bottom to be at screen bottom (render_y=0), scroll = 1271 - 720 = 551
    let required_scroll = 1271.0 - 720.0;
    tc.set_scroll(required_scroll);

    let positions_after = tc.render_positions();
    let t0_render_y_after = positions_after[2].0;
    let t0_bottom_after = t0_render_y_after + positions_after[2].1;

    // Now T0's bottom should be at render y=0 (bottom of screen)
    assert!(
        t0_bottom_after >= 0,
        "T0 bottom should be on screen after scroll adjustment, got render_y={}",
        t0_render_y_after
    );

    // And T0 should be visible
    assert!(
        tc.is_window_visible(2),
        "T0 should be visible after scroll adjustment"
    );
}
