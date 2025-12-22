//! Tests for terminal window growth

use std::time::Duration;
use test_harness::{assertions, fixtures};

/// NOTE: Requires live terminal with real shell, not mock
#[test]
#[ignore = "requires live terminal infrastructure"]
fn terminal_grows_with_content() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .expect("terminal should appear");

    let initial_height = tc.snapshot().window_heights[0];

    // Generate content
    tc.send_input(&term, &fixtures::seq_command(1, 50));

    tc.wait_for(
        |c| c.snapshot().window_heights[0] > initial_height + 500,
        Duration::from_secs(5),
    )
    .expect("terminal should grow");
}

#[test]
fn multiple_terminals_stack_vertically() {
    let (mut tc, _terminals) = fixtures::multiple_terminals(3);

    tc.wait_for(
        |c| c.snapshot().window_count == 3,
        Duration::from_secs(2),
    )
    .expect("terminals should appear");

    let snapshot = tc.snapshot();
    assertions::assert_windows_dont_overlap(&snapshot);
}
