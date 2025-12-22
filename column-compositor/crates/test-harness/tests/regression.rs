//! Regression tests for bugs found in v1

use std::time::Duration;
use test_harness::{TestCompositor, assertions, fixtures};

/// Regression: empty rows after command (v1 issue-003)
#[test]
fn no_empty_rows_after_command() {
    let (mut tc, term) = fixtures::single_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    tc.send_input(&term, "ls -la\n");

    tc.wait_for(
        |c| c.get_terminal_content(&term).contains("$"),
        Duration::from_secs(5),
    )
    .ok(); // May not have $ in mock

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
