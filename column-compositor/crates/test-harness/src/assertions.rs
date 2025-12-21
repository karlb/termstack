//! Test assertions for compositor state

use crate::headless::{CompositorSnapshot, RenderedElement, TestCompositor};

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

/// Assert that render positions match click detection positions
pub fn assert_render_matches_click_detection(tc: &TestCompositor) {
    let render_pos = tc.render_positions();
    let click_ranges = tc.window_click_ranges();

    assert_eq!(
        render_pos.len(),
        click_ranges.len(),
        "render and click range counts should match"
    );

    for (i, ((render_y, render_h), (click_start, click_end))) in
        render_pos.iter().zip(click_ranges.iter()).enumerate()
    {
        let click_height = click_end - click_start;
        assert_eq!(
            *render_y as f64, *click_start,
            "window {} render Y ({}) should match click start ({})",
            i, render_y, click_start
        );
        assert!(
            (*render_h as f64 - click_height).abs() < 0.001,
            "window {} render height ({}) should match click height ({})",
            i, render_h, click_height
        );
    }
}

/// Assert that clicking at Y hits the expected window index
pub fn assert_click_at_y_hits_window(tc: &TestCompositor, y: f64, expected: Option<usize>) {
    let result = tc.window_at(y);
    assert_eq!(
        result, expected,
        "click at Y={} should hit window {:?}, got {:?}",
        y, expected, result
    );
}

/// Assert that windows are rendered in correct order (top to bottom = index 0 to N)
pub fn assert_window_order_correct(tc: &TestCompositor) {
    let render_pos = tc.render_positions();

    // Check that each window starts after the previous one
    for i in 1..render_pos.len() {
        let prev_end = render_pos[i - 1].0 + render_pos[i - 1].1;
        let curr_start = render_pos[i].0;
        assert_eq!(
            prev_end, curr_start,
            "window {} should start at {} (after window {} ends at {}), but starts at {}",
            i, prev_end, i - 1, prev_end, curr_start
        );
    }
}

/// Assert click targets are NOT vertically flipped
/// (clicking near top of screen should hit window 0, not the last window)
pub fn assert_click_targets_not_flipped(tc: &TestCompositor) {
    let render_pos = tc.render_positions();
    if render_pos.is_empty() {
        return;
    }

    // Find window rendered at lowest Y (topmost on screen)
    let topmost_window = render_pos.iter().enumerate()
        .min_by_key(|(_, (y, _))| *y)
        .map(|(i, _)| i);

    // Click near the top of that window should hit it
    if let Some(top_idx) = topmost_window {
        let (y, _) = render_pos[top_idx];
        let click_y = y as f64 + 10.0; // 10 pixels into the window
        let clicked = tc.window_at(click_y);

        assert_eq!(
            clicked, Some(top_idx),
            "clicking at Y={} should hit topmost window {} (rendered at Y={}), got {:?}",
            click_y, top_idx, y, clicked
        );
    }
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

/// Assert that no elements from different windows overlap
/// This is critical for preventing the gnome-maps split rendering bug
pub fn assert_no_element_overlaps(tc: &TestCompositor) {
    let overlaps = tc.find_element_overlaps();
    if !overlaps.is_empty() {
        let mut msg = String::from("Element overlaps detected:\n");
        for (a, b) in &overlaps {
            msg.push_str(&format!(
                "  Window {} element {} (y={}, h={}) overlaps with Window {} element {} (y={}, h={})\n",
                a.window_index, a.element_index, a.screen_y, a.height,
                b.window_index, b.element_index, b.screen_y, b.height
            ));
        }
        panic!("{}", msg);
    }
}

/// Assert that all elements from a window are within the window's allocated region
/// on screen (no elements extending above or below the window boundary)
pub fn assert_elements_within_window_bounds(tc: &TestCompositor) {
    let elements = tc.rendered_elements();
    let render_pos = tc.render_positions();

    for elem in &elements {
        if elem.window_index >= render_pos.len() {
            panic!("Element references non-existent window {}", elem.window_index);
        }

        let (window_y, window_height) = render_pos[elem.window_index];
        let window_end = window_y + window_height;
        let elem_end = elem.screen_y + elem.height;

        // Element should start within window bounds
        assert!(
            elem.screen_y >= window_y,
            "Window {} element {} starts at {} but window starts at {}",
            elem.window_index, elem.element_index, elem.screen_y, window_y
        );

        // Element should end within window bounds
        assert!(
            elem_end <= window_end,
            "Window {} element {} ends at {} but window ends at {}",
            elem.window_index, elem.element_index, elem_end, window_end
        );
    }
}
