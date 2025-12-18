//! Test fixtures for common test scenarios

use crate::headless::{TestCompositor, TerminalHandle};

/// Standard test output dimensions
pub const TEST_WIDTH: u32 = 1280;
pub const TEST_HEIGHT: u32 = 720;

/// Create a compositor with a single terminal
pub fn single_terminal() -> (TestCompositor, TerminalHandle) {
    let mut compositor = TestCompositor::new_headless(TEST_WIDTH, TEST_HEIGHT);
    let terminal = compositor.spawn_terminal();
    (compositor, terminal)
}

/// Create a compositor with multiple terminals
pub fn multiple_terminals(count: usize) -> (TestCompositor, Vec<TerminalHandle>) {
    let mut compositor = TestCompositor::new_headless(TEST_WIDTH, TEST_HEIGHT);
    let terminals: Vec<_> = (0..count).map(|_| compositor.spawn_terminal()).collect();
    (compositor, terminals)
}

/// Shell command to generate numbered lines
pub fn seq_command(start: u32, end: u32) -> String {
    format!("for i in $(seq {} {}); do echo \"line $i\"; done\n", start, end)
}

/// Shell command to generate rapid output with delays
pub fn rapid_output_command(count: u32, delay_ms: u32) -> String {
    format!(
        r#"for i in $(seq 1 {}); do echo "line $i: $(date +%s%N)"; sleep 0.{}; done"#,
        count,
        delay_ms.to_string().pad_to_width(3)
    )
}

trait PadToWidth {
    fn pad_to_width(&self, width: usize) -> String;
}

impl PadToWidth for String {
    fn pad_to_width(&self, width: usize) -> String {
        format!("{:0>width$}", self, width = width)
    }
}
