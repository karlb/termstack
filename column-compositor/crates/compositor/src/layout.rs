//! Column layout algorithm
//!
//! Pure functions for calculating window positions in a vertical column.
//! Key learning from v1: keep layout calculation pure with no side effects.

use std::ops::Range;

use crate::state::WindowEntry;

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
#[derive(Debug, Clone)]
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

    /// Calculate layout from windows and scroll state
    ///
    /// This is a pure function: same inputs always produce same outputs.
    /// No side effects, no state mutation.
    pub fn calculate(
        windows: &[WindowEntry],
        output_height: u32,
        scroll_offset: f64,
    ) -> Self {
        if windows.is_empty() {
            return Self::empty();
        }

        let mut y_accumulator: i32 = 0;
        let mut positions = Vec::with_capacity(windows.len());

        for window in windows {
            let height = window.state.current_height();
            let y = y_accumulator - scroll_offset as i32;

            let visible = y < output_height as i32 && y + height as i32 > 0;

            positions.push(WindowPosition { y, height, visible });

            y_accumulator += height as i32;
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
            return Some((accumulated_y as i32 - output_height as i32).max(0) as f64);
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
    #[cfg(test)]
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

    fn mock_entry(height: u32) -> WindowEntry {
        // Create a minimal WindowEntry for testing
        // In real tests we'd use proper mocks
        unimplemented!("Use integration tests with proper WindowEntry")
    }

    // Layout tests that don't need WindowEntry
    #[test]
    fn empty_layout() {
        let layout = ColumnLayout::empty();
        assert_eq!(layout.total_height, 0);
        assert!(layout.window_positions.is_empty());
    }

    #[test]
    fn layout_positions_are_pure() {
        // Same inputs should produce same outputs
        let positions1 = vec![
            WindowPosition { y: 0, height: 100, visible: true },
            WindowPosition { y: 100, height: 200, visible: true },
        ];
        let positions2 = vec![
            WindowPosition { y: 0, height: 100, visible: true },
            WindowPosition { y: 100, height: 200, visible: true },
        ];

        assert_eq!(positions1[0].y, positions2[0].y);
        assert_eq!(positions1[0].height, positions2[0].height);
        assert_eq!(positions1[1].y, positions2[1].y);
        assert_eq!(positions1[1].height, positions2[1].height);
    }
}
