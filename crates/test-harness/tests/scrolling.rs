//! Tests for scrolling behavior

use std::time::Duration;
use test_harness::fixtures;

/// NOTE: Requires live terminal with real shell, not mock
#[test]
#[ignore = "requires live terminal infrastructure"]
fn auto_scroll_on_growth() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // Fill screen and trigger scroll
    tc.send_input(&term, &fixtures::seq_command(1, 100));

    tc.wait_for(
        |c| c.snapshot().scroll_offset > 0.0,
        Duration::from_secs(5),
    )
    .expect("should auto-scroll");

    // Verify bottom of terminal is visible
    let snapshot = tc.snapshot();
    let term_bottom = snapshot.window_heights[0] as f64;
    let output_height = tc.output_size().1 as f64;
    let viewport_bottom = snapshot.scroll_offset + output_height;

    assert!(
        (term_bottom - viewport_bottom).abs() < 50.0,
        "terminal bottom ({}) should be near viewport bottom ({})",
        term_bottom,
        viewport_bottom
    );
}

#[test]
fn scroll_keeps_window_visible() {
    let (mut tc, terms) = fixtures::multiple_terminals(5);

    tc.wait_for(
        |c| c.snapshot().window_count == 5,
        Duration::from_secs(2),
    )
    .unwrap();

    // Grow terminals to exceed viewport
    for term in &terms {
        tc.send_input(term, &fixtures::seq_command(1, 30));
    }

    let output_height = tc.output_size().1;
    tc.wait_for(
        |c| c.snapshot().total_height > output_height,
        Duration::from_secs(5),
    )
    .expect("should exceed viewport");

    let snapshot = tc.snapshot();

    // At least one window should be visible
    let output_height = tc.output_size().1;
    let mut any_visible = false;

    let mut y = 0i32;
    for &height in &snapshot.window_heights {
        let window_y = y - snapshot.scroll_offset as i32;
        let window_bottom = window_y + height as i32;

        if window_y < output_height as i32 && window_bottom > 0 {
            any_visible = true;
            break;
        }

        y += height as i32;
    }

    assert!(any_visible, "at least one window should be visible");
}
