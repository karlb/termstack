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

/// Create a compositor with mixed windows (terminals and external windows)
///
/// Useful for testing window interaction, focus management, and layout with heterogeneous content.
///
/// Returns: (compositor, terminal_handles)
pub fn compositor_with_mixed_windows() -> (TestCompositor, Vec<TerminalHandle>) {
    let mut compositor = TestCompositor::new_headless(TEST_WIDTH, TEST_HEIGHT);
    let term1 = compositor.spawn_terminal();
    compositor.add_external_window(200); // External window at index 1
    let term2 = compositor.spawn_terminal();
    compositor.add_external_window(300); // External window at index 3
    (compositor, vec![term1, term2])
}

/// Create a compositor with scrollable content (total height > viewport)
///
/// Useful for testing scroll behavior, visibility calculations, and autoscroll.
/// Total content height: ~1200px, viewport: 720px
///
/// Returns: compositor with 3 external windows (400px each)
pub fn compositor_with_scrollable_content() -> TestCompositor {
    let mut compositor = TestCompositor::new_headless(TEST_WIDTH, TEST_HEIGHT);
    compositor.add_external_window(400);
    compositor.add_external_window(400);
    compositor.add_external_window(400);
    compositor
}

/// Create a compositor scrolled to maximum offset
///
/// Useful for testing max scroll behavior, scroll bounds, and bottom edge cases.
///
/// Returns: compositor scrolled to show bottom of content
pub fn compositor_at_max_scroll() -> TestCompositor {
    let mut tc = compositor_with_scrollable_content();
    // Scroll to maximum (total height - viewport height)
    // Total: 1200px, viewport: 720px, max_scroll: 480px
    tc.simulate_scroll(-480.0); // Negative scroll moves content up
    tc
}

/// Create a compositor with a focused external window at specified index
///
/// Useful for testing focus-dependent behavior like keyboard input routing.
///
/// Arguments:
/// - window_count: Total number of external windows to create
/// - focused_index: Which window should have focus (0-indexed)
///
/// Returns: compositor with `window_count` external windows, focused at `focused_index`
pub fn compositor_with_focused_window(window_count: usize, focused_index: usize) -> TestCompositor {
    assert!(focused_index < window_count, "focused_index must be < window_count");
    let mut compositor = TestCompositor::new_headless(TEST_WIDTH, TEST_HEIGHT);

    for _ in 0..window_count {
        compositor.add_external_window(200);
    }

    // Click on the focused window to give it focus
    // Windows stack from top to bottom, each 200px tall
    let click_y = (focused_index * 200 + 100) as f64; // Middle of target window
    compositor.simulate_click(100.0, click_y);

    compositor
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
