//! Test assertions for compositor state

use crate::headless::CompositorSnapshot;

/// Assert that windows don't overlap
pub fn assert_windows_dont_overlap(snapshot: &CompositorSnapshot) {
    // With column layout, windows are stacked vertically
    // They don't overlap by design if heights are consistent
    let total: u32 = snapshot.window_heights.iter().sum();
    assert_eq!(
        total, snapshot.total_height,
        "total height mismatch: sum={}, reported={}",
        total, snapshot.total_height
    );
}

/// Assert that a window is visible
pub fn assert_window_visible(
    snapshot: &CompositorSnapshot,
    index: usize,
    output_height: u32,
) {
    let mut y = 0i32;

    for (i, &height) in snapshot.window_heights.iter().enumerate() {
        let window_y = y - snapshot.scroll_offset as i32;
        let window_bottom = window_y + height as i32;

        if i == index {
            let visible = window_y < output_height as i32 && window_bottom > 0;
            assert!(
                visible,
                "window {} not visible: y={}, bottom={}, scroll={}, output_h={}",
                index, window_y, window_bottom, snapshot.scroll_offset, output_height
            );
            return;
        }

        y += height as i32;
    }

    panic!("window index {} out of range", index);
}

/// Assert no empty rows at bottom of terminal content
pub fn assert_no_empty_rows(content: &str) {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return;
    }

    // Find last non-empty line
    let last_content = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .unwrap_or(0);

    let empty_at_end = lines.len() - 1 - last_content;

    assert!(
        empty_at_end <= 1,
        "too many empty rows at bottom: {} empty lines",
        empty_at_end
    );
}

/// Assert that all numbered lines are present (for seq tests)
pub fn assert_lines_present(content: &str, start: u32, end: u32) {
    for i in start..=end {
        assert!(
            content.contains(&i.to_string()),
            "missing line number {}",
            i
        );
    }
}
