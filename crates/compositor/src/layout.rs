//! Column layout algorithm
//!
//! Pure functions for calculating window positions in a vertical column.
//! Key learning from v1: keep layout calculation pure with no side effects.
//!
//! # Responsibilities
//!
//! - Pure layout calculation (window positions from heights)
//! - Visibility detection (which windows are in viewport)
//! - Scroll offset calculations
//! - Total height computation
//!
//! # Design Contract
//!
//! - All functions are **pure** - no side effects, no state mutation
//! - Deterministic - same inputs always produce same outputs
//! - Testable with property-based testing
//!
//! # NOT Responsible For
//!
//! - Window state (see `state.rs`)
//! - Terminal dimensions (see `terminal_manager/`)
//! - Rendering (see `render.rs`)

use std::ops::Range;

use crate::title_bar::TITLE_BAR_HEIGHT;

/// Focus indicator width in pixels (also used as left margin for content)
pub const FOCUS_INDICATOR_WIDTH: i32 = 2;

/// Calculate the visual/render height for a terminal.
///
/// This is the total height including the title bar (if shown).
/// The title bar is shown when the terminal is visible and has `show_title_bar` set.
///
/// # Arguments
/// * `content_height` - Height of the terminal content in pixels (may be 0)
/// * `show_title_bar` - Whether the terminal should show a title bar
/// * `is_visible` - Whether the terminal is visible (hidden terminals have 0 height)
///
/// # Returns
/// The total visual height including title bar if applicable.
pub fn calculate_terminal_render_height(
    content_height: i32,
    show_title_bar: bool,
    is_visible: bool,
) -> i32 {
    if !is_visible {
        return 0;
    }
    if show_title_bar {
        content_height + TITLE_BAR_HEIGHT as i32
    } else {
        content_height
    }
}

/// Check if any cell heights changed significantly (affecting scroll)
pub fn heights_changed_significantly(
    cached: &[i32],
    actual: &[i32],
    focused_index: Option<usize>,
) -> bool {
    cached.iter()
        .zip(actual.iter())
        .enumerate()
        .any(|(i, (&cached_h, &actual_h))| {
            if let Some(focused) = focused_index {
                if i <= focused && actual_h != cached_h && (actual_h - cached_h).abs() > 10 {
                    return true;
                }
            }
            false
        })
}

/// Calculated layout for all windows
#[derive(Debug, Clone)]
pub struct ColumnLayout {
    /// Position and size of each window
    pub window_positions: Vec<WindowPosition>,

    /// Total height of all windows combined
    pub total_height: u32,

    /// Range of Y coordinates visible in viewport
    pub visible_range: Range<u32>,
}

/// Position and visibility of a single window
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPosition {
    /// Y coordinate (can be negative if scrolled off top)
    pub y: i32,

    /// Height of this window
    pub height: u32,

    /// Whether any part of the window is visible
    pub visible: bool,
}

impl ColumnLayout {
    /// Create an empty layout
    pub fn empty() -> Self {
        Self {
            window_positions: Vec::new(),
            total_height: 0,
            visible_range: 0..0,
        }
    }

    /// Calculate layout from an iterator of heights.
    ///
    /// This is the core pure function: same inputs always produce same outputs.
    /// No side effects, no state mutation. Can be tested without Wayland types.
    pub fn calculate_from_heights(
        heights: impl IntoIterator<Item = u32>,
        output_height: u32,
        scroll_offset: f64,
    ) -> Self {
        let mut y_accumulator: i32 = 0;
        let mut positions = Vec::new();

        for height in heights {
            let y = y_accumulator - scroll_offset as i32;
            let visible = y < output_height as i32 && y + height as i32 > 0;

            positions.push(WindowPosition { y, height, visible });
            y_accumulator += height as i32;
        }

        if positions.is_empty() {
            return Self::empty();
        }

        let total_height = y_accumulator as u32;

        Self {
            window_positions: positions,
            total_height,
            visible_range: scroll_offset as u32..scroll_offset as u32 + output_height,
        }
    }

    /// Calculate scroll offset to show the bottom of a window
    ///
    /// Returns Some(new_offset) if scrolling is needed, None if already visible.
    pub fn scroll_to_show_bottom(&self, window_index: usize, output_height: u32) -> Option<f64> {
        let pos = self.window_positions.get(window_index)?;
        let window_bottom = pos.y + pos.height as i32;

        if window_bottom > output_height as i32 {
            // Window extends below viewport - scroll to show bottom
            Some((window_bottom - output_height as i32) as f64)
        } else {
            None
        }
    }

    /// Calculate scroll offset to show a window (top or bottom depending on direction)
    pub fn scroll_to_show(&self, window_index: usize, output_height: u32) -> Option<f64> {
        let pos = self.window_positions.get(window_index)?;

        // If window top is above viewport, scroll to show top
        if pos.y < 0 {
            // Calculate what scroll offset would put this window's top at viewport top
            // Current: y = accumulated_y - scroll_offset
            // Want: y = 0, so scroll_offset = accumulated_y
            let accumulated_y: i32 = self.window_positions[..=window_index]
                .iter()
                .map(|p| p.height as i32)
                .sum::<i32>()
                - pos.height as i32;
            return Some(accumulated_y as f64);
        }

        // If window bottom is below viewport, scroll to show bottom
        let window_bottom = pos.y + pos.height as i32;
        if window_bottom > output_height as i32 {
            let accumulated_y: i32 = self.window_positions[..=window_index]
                .iter()
                .map(|p| p.height as i32)
                .sum::<i32>();
            return Some((accumulated_y - output_height as i32).max(0) as f64);
        }

        None
    }

    /// Get indices of visible windows
    pub fn visible_windows(&self) -> impl Iterator<Item = usize> + '_ {
        self.window_positions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.visible)
            .map(|(i, _)| i)
    }

    /// Check invariants (for testing)
    pub fn check_invariants(&self) -> Result<(), String> {
        // Windows should not overlap
        for i in 1..self.window_positions.len() {
            let prev = &self.window_positions[i - 1];
            let curr = &self.window_positions[i];

            // Previous window's bottom should be at current window's top
            let prev_bottom = prev.y + prev.height as i32;
            if prev_bottom != curr.y {
                return Err(format!(
                    "Gap or overlap between windows {} and {}: prev_bottom={}, curr_y={}",
                    i - 1,
                    i,
                    prev_bottom,
                    curr.y
                ));
            }
        }

        // Total height should equal sum of window heights
        let sum: u32 = self.window_positions.iter().map(|p| p.height).sum();
        if sum != self.total_height {
            return Err(format!(
                "Total height mismatch: sum={}, total={}",
                sum, self.total_height
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_layout() {
        let layout = ColumnLayout::empty();
        assert_eq!(layout.total_height, 0);
        assert!(layout.window_positions.is_empty());
    }

    #[test]
    fn empty_heights_produces_empty_layout() {
        let layout = ColumnLayout::calculate_from_heights([], 720, 0.0);
        assert_eq!(layout.total_height, 0);
        assert!(layout.window_positions.is_empty());
    }

    #[test]
    fn single_window_at_origin() {
        let layout = ColumnLayout::calculate_from_heights([200], 720, 0.0);

        assert_eq!(layout.total_height, 200);
        assert_eq!(layout.window_positions.len(), 1);
        assert_eq!(
            layout.window_positions[0],
            WindowPosition { y: 0, height: 200, visible: true }
        );
    }

    #[test]
    fn multiple_windows_stack_vertically() {
        let layout = ColumnLayout::calculate_from_heights([100, 200, 150], 720, 0.0);

        assert_eq!(layout.total_height, 450);
        assert_eq!(layout.window_positions.len(), 3);

        assert_eq!(layout.window_positions[0], WindowPosition { y: 0, height: 100, visible: true });
        assert_eq!(layout.window_positions[1], WindowPosition { y: 100, height: 200, visible: true });
        assert_eq!(layout.window_positions[2], WindowPosition { y: 300, height: 150, visible: true });
    }

    #[test]
    fn windows_never_overlap() {
        let layout = ColumnLayout::calculate_from_heights([100, 200, 300, 150], 720, 0.0);

        for i in 1..layout.window_positions.len() {
            let prev = &layout.window_positions[i - 1];
            let curr = &layout.window_positions[i];
            let prev_bottom = prev.y + prev.height as i32;

            assert_eq!(
                prev_bottom, curr.y,
                "window {} bottom ({}) should equal window {} top ({})",
                i - 1, prev_bottom, i, curr.y
            );
        }
    }

    #[test]
    fn scroll_offset_shifts_all_positions() {
        let no_scroll = ColumnLayout::calculate_from_heights([100, 200], 720, 0.0);
        let with_scroll = ColumnLayout::calculate_from_heights([100, 200], 720, 50.0);

        // All Y positions should be shifted by -50
        assert_eq!(no_scroll.window_positions[0].y - 50, with_scroll.window_positions[0].y);
        assert_eq!(no_scroll.window_positions[1].y - 50, with_scroll.window_positions[1].y);

        // Heights unchanged
        assert_eq!(no_scroll.window_positions[0].height, with_scroll.window_positions[0].height);
        assert_eq!(no_scroll.window_positions[1].height, with_scroll.window_positions[1].height);

        // Total height unchanged
        assert_eq!(no_scroll.total_height, with_scroll.total_height);
    }

    #[test]
    fn visibility_when_fully_on_screen() {
        let layout = ColumnLayout::calculate_from_heights([200], 720, 0.0);
        assert!(layout.window_positions[0].visible);
    }

    #[test]
    fn visibility_when_partially_scrolled_off_top() {
        // Window at y=-50 with height 200 is partially visible (150px showing)
        let layout = ColumnLayout::calculate_from_heights([200], 720, 50.0);
        assert_eq!(layout.window_positions[0].y, -50);
        assert!(layout.window_positions[0].visible);
    }

    #[test]
    fn visibility_when_fully_scrolled_off_top() {
        // Window at y=-250 with height 200 is not visible (ends at -50)
        let layout = ColumnLayout::calculate_from_heights([200], 720, 250.0);
        assert_eq!(layout.window_positions[0].y, -250);
        assert!(!layout.window_positions[0].visible);
    }

    #[test]
    fn visibility_when_partially_below_viewport() {
        // Window starts at y=600, viewport is 720, so 120px visible
        let layout = ColumnLayout::calculate_from_heights([100, 100, 100, 100, 100, 100, 200], 720, 0.0);
        let last = layout.window_positions.last().unwrap();
        assert_eq!(last.y, 600);
        assert!(last.visible);
    }

    #[test]
    fn visibility_when_fully_below_viewport() {
        // Window starts at y=800, viewport is 720
        let layout = ColumnLayout::calculate_from_heights([800, 200], 720, 0.0);
        assert!(!layout.window_positions[1].visible);
    }

    #[test]
    fn total_height_is_sum_of_heights() {
        let heights = [100, 200, 300, 50, 150];
        let layout = ColumnLayout::calculate_from_heights(heights, 720, 0.0);
        let expected: u32 = heights.iter().sum();
        assert_eq!(layout.total_height, expected);
    }

    #[test]
    fn visible_range_tracks_viewport() {
        let layout = ColumnLayout::calculate_from_heights([100, 200, 300], 720, 100.0);
        assert_eq!(layout.visible_range, 100..820);
    }

    #[test]
    fn layout_is_deterministic() {
        let heights = vec![100, 200, 150, 300];

        let layout1 = ColumnLayout::calculate_from_heights(heights.clone(), 720, 50.0);
        let layout2 = ColumnLayout::calculate_from_heights(heights, 720, 50.0);

        assert_eq!(layout1.total_height, layout2.total_height);
        assert_eq!(layout1.window_positions, layout2.window_positions);
    }

    #[test]
    fn invariants_pass_for_valid_layout() {
        let layout = ColumnLayout::calculate_from_heights([100, 200, 300], 720, 0.0);
        assert!(layout.check_invariants().is_ok());
    }

    #[test]
    fn scroll_to_show_when_window_below_viewport() {
        let layout = ColumnLayout::calculate_from_heights([400, 400, 400], 720, 0.0);
        // Window 2 starts at 800, ends at 1200 - need to scroll to see bottom
        let scroll = layout.scroll_to_show(2, 720);
        assert!(scroll.is_some());
        assert_eq!(scroll.unwrap(), 480.0); // 1200 - 720 = 480
    }

    #[test]
    fn scroll_to_show_when_window_above_viewport() {
        let layout = ColumnLayout::calculate_from_heights([400, 400, 400], 720, 500.0);
        // Window 0 is at y=-500, need to scroll up
        let scroll = layout.scroll_to_show(0, 720);
        assert!(scroll.is_some());
        assert_eq!(scroll.unwrap(), 0.0); // Scroll to top to show window 0
    }

    #[test]
    fn scroll_to_show_returns_none_when_visible() {
        let layout = ColumnLayout::calculate_from_heights([200, 200], 720, 0.0);
        // Window 0 is fully visible
        let scroll = layout.scroll_to_show(0, 720);
        assert!(scroll.is_none());
    }

    #[test]
    fn visible_windows_iterator() {
        let layout = ColumnLayout::calculate_from_heights([300, 300, 300, 300], 720, 200.0);
        // Scroll=200, viewport=720, so visible range is 200..920
        // Window 0: y=-200..100 (visible)
        // Window 1: y=100..400 (visible)
        // Window 2: y=400..700 (visible)
        // Window 3: y=700..1000 (partially visible)
        let visible: Vec<_> = layout.visible_windows().collect();
        assert_eq!(visible, vec![0, 1, 2, 3]);
    }

    #[test]
    fn visible_windows_excludes_scrolled_off() {
        let layout = ColumnLayout::calculate_from_heights([100, 100, 100, 100], 720, 250.0);
        // Window 0: y=-250..-150 (not visible)
        // Window 1: y=-150..-50 (not visible)
        // Window 2: y=-50..50 (visible - partial)
        // Window 3: y=50..150 (visible)
        let visible: Vec<_> = layout.visible_windows().collect();
        assert_eq!(visible, vec![2, 3]);
    }
}
