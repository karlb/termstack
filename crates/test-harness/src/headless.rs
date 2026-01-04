//! Headless compositor wrapper for testing

use std::time::{Duration, Instant};

use compositor::coords::{RenderPoint, ScreenPoint};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TestError {
    #[error("timeout waiting for condition")]
    Timeout,

    #[error("compositor error: {0}")]
    Compositor(String),
}

/// Snapshot of compositor state for assertions
#[derive(Debug, Clone)]
pub struct CompositorSnapshot {
    /// Number of windows
    pub window_count: usize,

    /// Heights of each window
    pub window_heights: Vec<u32>,

    /// Current scroll offset
    pub scroll_offset: f64,

    /// Total content height
    pub total_height: u32,

    /// Currently focused window index
    pub focused_index: Option<usize>,
}

/// Handle to a terminal window in tests
#[derive(Debug, Clone, Copy)]
pub struct TerminalHandle {
    /// Index in the compositor's window list
    pub index: usize,
}

/// Test compositor wrapper
pub struct TestCompositor {
    /// Output dimensions
    output_size: (u32, u32),

    /// Mock window data for testing
    windows: Vec<MockWindow>,

    /// Mock popups for testing
    popups: Vec<MockPopup>,

    /// Scroll offset
    scroll_offset: f64,

    /// Focused index
    focused_index: Option<usize>,

    /// Total height of terminals (before external windows)
    terminal_total_height: i32,

    /// Cached window heights (mirrors real compositor behavior)
    cached_window_heights: Vec<i32>,

    /// Current pointer location in render coordinates (Y=0 at bottom)
    pointer_location: RenderPoint,
}

struct MockWindow {
    /// The "cached" height - what bbox() would return
    /// This is used by click detection in the real compositor
    cached_height: u32,
    content: String,
    /// Elements within this window (internal_y_offset, element_height)
    /// For simple windows, this is just [(0, height)]
    /// For complex windows like gnome-maps, this could be multiple elements
    /// The ACTUAL rendered height is max(elem.y + elem.height) for all elements
    elements: Vec<(i32, i32)>,
}

/// Mock popup for testing popup behavior
#[derive(Debug, Clone)]
pub struct MockPopup {
    /// Parent window index
    pub parent_index: usize,
    /// Offset from parent window (x, y)
    pub offset: (i32, i32),
    /// Size (width, height)
    pub size: (i32, i32),
    /// Whether this popup has an active grab
    pub has_grab: bool,
}

/// Represents a rendered element's final screen position
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderedElement {
    pub window_index: usize,
    pub element_index: usize,
    pub screen_y: i32,
    pub height: i32,
}

impl TestCompositor {
    /// Create a new headless test compositor
    pub fn new_headless(width: u32, height: u32) -> Self {
        Self {
            output_size: (width, height),
            windows: Vec::new(),
            popups: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
            terminal_total_height: 0,
            cached_window_heights: Vec::new(),
            pointer_location: RenderPoint::new(0.0, 0.0),
        }
    }

    /// Set terminal total height (for testing window positioning after terminals)
    pub fn set_terminal_height(&mut self, height: i32) {
        self.terminal_total_height = height;
    }

    /// Update cached window heights (mirrors real compositor's update_cached_window_heights)
    /// Uses the cached_height field, NOT the actual element heights
    pub fn update_cached_window_heights(&mut self) {
        self.cached_window_heights = self.windows.iter().map(|w| w.cached_height as i32).collect();
    }

    /// Get actual height for a window based on its elements
    /// This is max(elem.y + elem.height) for all elements
    fn actual_height(&self, window_idx: usize) -> i32 {
        self.windows.get(window_idx)
            .map(|w| {
                w.elements.iter()
                    .map(|&(y, h)| y + h)
                    .max()
                    .unwrap_or(w.cached_height as i32)
            })
            .unwrap_or(0)
    }

    /// Get actual heights for all windows (what rendering actually uses)
    pub fn actual_heights(&self) -> Vec<i32> {
        (0..self.windows.len())
            .map(|i| self.actual_height(i))
            .collect()
    }

    /// Get window under a point - mirrors real compositor's window_at()
    /// NOW USES ACTUAL HEIGHTS (like the fixed compositor)
    pub fn window_at(&self, y: f64) -> Option<usize> {
        // Match the real window_at() implementation with Y-flip
        let screen_height = self.output_size.1 as f64;
        let mut content_y = -self.scroll_offset;
        let actual_heights = self.actual_heights();

        for (i, &height) in actual_heights.iter().enumerate() {
            let cell_height = height as f64;

            // Calculate render Y for this cell (same formula as state.rs window_at)
            let render_y = screen_height - content_y - cell_height;
            let render_end = render_y + cell_height;

            if y >= render_y && y < render_end {
                return Some(i);
            }
            content_y += cell_height;
        }
        None
    }

    /// Get window under a point using CACHED heights (OLD buggy behavior)
    pub fn window_at_cached(&self, y: f64) -> Option<usize> {
        let terminal_height = self.terminal_total_height as f64;
        let mut window_y = terminal_height - self.scroll_offset;

        for (i, &height) in self.cached_window_heights.iter().enumerate() {
            let window_height = height as f64;
            let window_screen_end = window_y + window_height;

            if y >= window_y && y < window_screen_end {
                return Some(i);
            }
            window_y += window_height;
        }
        None
    }

    /// Get render positions - mirrors real compositor's render position calculation
    /// Returns Vec of (y_position, height) for each window
    /// NOW USES Y-FLIP (like the fixed main.rs rendering code)
    pub fn render_positions(&self) -> Vec<(i32, i32)> {
        let screen_height = self.output_size.1 as i32;
        let mut content_y = -(self.scroll_offset as i32);
        let actual_heights = self.actual_heights();
        actual_heights
            .iter()
            .map(|&height| {
                // Apply Y-flip: render_y = screen_height - content_y - height
                let render_y = screen_height - content_y - height;
                content_y += height;
                (render_y, height)
            })
            .collect()
    }

    /// Get render positions using CACHED heights (OLD buggy behavior)
    /// This shows where click detection THINKS windows are
    pub fn render_positions_cached(&self) -> Vec<(i32, i32)> {
        let mut window_y = -(self.scroll_offset as i32) + self.terminal_total_height;
        self.cached_window_heights
            .iter()
            .map(|&height| {
                let y = window_y;
                window_y += height;
                (y, height)
            })
            .collect()
    }

    /// Get window Y ranges for click detection
    /// NOW USES ACTUAL HEIGHTS (like the fixed compositor)
    pub fn window_click_ranges(&self) -> Vec<(f64, f64)> {
        // Match the real window_at() implementation with Y-flip
        let screen_height = self.output_size.1 as f64;
        let mut content_y = -self.scroll_offset;
        let actual_heights = self.actual_heights();

        actual_heights
            .iter()
            .map(|&height| {
                let cell_height = height as f64;
                // Apply Y-flip: render_y = screen_height - content_y - height
                let render_y = screen_height - content_y - cell_height;
                let render_end = render_y + cell_height;
                content_y += cell_height;
                (render_y, render_end)
            })
            .collect()
    }

    /// Get window Y ranges using CACHED heights (OLD buggy behavior)
    pub fn window_click_ranges_cached(&self) -> Vec<(f64, f64)> {
        let terminal_height = self.terminal_total_height as f64;
        let mut window_y = terminal_height - self.scroll_offset;

        self.cached_window_heights
            .iter()
            .map(|&height| {
                let start = window_y;
                let end = window_y + height as f64;
                window_y = end;
                (start, end)
            })
            .collect()
    }

    /// Spawn a terminal and return a handle
    pub fn spawn_terminal(&mut self) -> TerminalHandle {
        let index = self.windows.len();

        self.windows.push(MockWindow {
            cached_height: 200,
            content: String::new(),
            elements: vec![(0, 200)], // Single element spanning the window
        });

        self.focused_index = Some(index);
        self.update_cached_window_heights();

        TerminalHandle { index }
    }

    /// Add an external window with specified height (for testing window positioning)
    /// The cached_height and actual element height are the same
    pub fn add_external_window(&mut self, height: u32) -> TerminalHandle {
        let index = self.windows.len();

        self.windows.push(MockWindow {
            cached_height: height,
            content: String::new(),
            elements: vec![(0, height as i32)], // Single element spanning the window
        });

        self.focused_index = Some(index);
        self.update_cached_window_heights();

        TerminalHandle { index }
    }

    /// Add an external window where cached_height differs from actual element height
    /// This simulates the bug where bbox() returns different value than rendered size
    pub fn add_external_window_with_mismatch(
        &mut self,
        cached_height: u32,
        actual_height: u32,
    ) -> TerminalHandle {
        let index = self.windows.len();

        self.windows.push(MockWindow {
            cached_height,
            content: String::new(),
            elements: vec![(0, actual_height as i32)], // Actual rendered height differs!
        });

        self.focused_index = Some(index);
        self.update_cached_window_heights();

        TerminalHandle { index }
    }

    /// Add a window with multiple elements (simulates complex apps like gnome-maps)
    /// elements: Vec of (internal_y_offset, element_height)
    /// cached_height is set to total_height (may differ from max element extent)
    pub fn add_window_with_elements(&mut self, total_height: u32, elements: Vec<(i32, i32)>) -> TerminalHandle {
        let index = self.windows.len();

        self.windows.push(MockWindow {
            cached_height: total_height,
            content: String::new(),
            elements,
        });

        self.focused_index = Some(index);
        self.update_cached_window_heights();

        TerminalHandle { index }
    }

    /// Get all rendered elements with their final screen positions
    /// This simulates what the rendering code does: window_y + geo.loc.y
    /// NOW USES ACTUAL HEIGHTS for window_y advancement (like fixed main.rs)
    pub fn rendered_elements(&self) -> Vec<RenderedElement> {
        let mut result = Vec::new();
        let mut window_y = -(self.scroll_offset as i32) + self.terminal_total_height;
        let actual_heights = self.actual_heights();

        for (window_idx, window) in self.windows.iter().enumerate() {
            for (elem_idx, &(internal_y, elem_height)) in window.elements.iter().enumerate() {
                // This mirrors the rendering code: screen_y = window_y + geo.loc.y
                let screen_y = window_y + internal_y;
                result.push(RenderedElement {
                    window_index: window_idx,
                    element_index: elem_idx,
                    screen_y,
                    height: elem_height,
                });
            }
            // Advance by ACTUAL height (like fixed main.rs)
            window_y += actual_heights[window_idx];
        }

        result
    }

    /// Check if any elements from different windows overlap
    pub fn find_element_overlaps(&self) -> Vec<(RenderedElement, RenderedElement)> {
        let elements = self.rendered_elements();
        let mut overlaps = Vec::new();

        for i in 0..elements.len() {
            for j in (i + 1)..elements.len() {
                let a = &elements[i];
                let b = &elements[j];

                // Only check elements from different windows
                if a.window_index == b.window_index {
                    continue;
                }

                // Check for overlap: ranges [a.screen_y, a.screen_y + a.height) and [b.screen_y, b.screen_y + b.height)
                let a_end = a.screen_y + a.height;
                let b_end = b.screen_y + b.height;

                if a.screen_y < b_end && b.screen_y < a_end {
                    overlaps.push((*a, *b));
                }
            }
        }

        overlaps
    }

    /// Set a specific window's cached height (simulates bbox returning different values)
    pub fn set_window_height(&mut self, index: usize, height: u32) {
        if let Some(window) = self.windows.get_mut(index) {
            window.cached_height = height;
            // Also update elements to match (for simple cases)
            window.elements = vec![(0, height as i32)];
        }
        self.update_cached_window_heights();
    }

    /// Set window's cached height WITHOUT updating elements
    /// This creates a mismatch between cached and actual heights
    pub fn set_window_cached_height_only(&mut self, index: usize, cached_height: u32) {
        if let Some(window) = self.windows.get_mut(index) {
            window.cached_height = cached_height;
            // Elements stay the same - creates mismatch!
        }
        self.update_cached_window_heights();
    }

    /// Send input to a terminal
    pub fn send_input(&mut self, handle: &TerminalHandle, input: &str) {
        if let Some(window) = self.windows.get_mut(handle.index) {
            // Simulate output from command
            window.content.push_str(input);

            // Count newlines and grow window
            let newlines = input.chars().filter(|&c| c == '\n').count();
            let line_height = 16u32; // Approximate
            let growth = (newlines as u32) * line_height;
            window.cached_height += growth;
            // Also grow elements
            if let Some((_, ref mut h)) = window.elements.last_mut() {
                *h += growth as i32;
            }
        }
        self.update_cached_window_heights();
    }

    /// Wait for a condition with timeout
    pub fn wait_for<F>(&mut self, condition: F, timeout: Duration) -> Result<(), TestError>
    where
        F: Fn(&Self) -> bool,
    {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            self.dispatch_events(Duration::from_millis(10))?;
            if condition(self) {
                return Ok(());
            }
        }

        Err(TestError::Timeout)
    }

    /// Dispatch pending events
    pub fn dispatch_events(&mut self, _duration: Duration) -> Result<(), TestError> {
        // In headless mode, just yield briefly
        std::thread::sleep(Duration::from_millis(1));
        Ok(())
    }

    /// Get current state snapshot
    pub fn snapshot(&self) -> CompositorSnapshot {
        CompositorSnapshot {
            window_count: self.windows.len(),
            window_heights: self.windows.iter().map(|w| w.cached_height).collect(),
            scroll_offset: self.scroll_offset,
            total_height: self.windows.iter().map(|w| w.cached_height).sum(),
            focused_index: self.focused_index,
        }
    }

    /// Get terminal content
    pub fn get_terminal_content(&self, handle: &TerminalHandle) -> String {
        self.windows
            .get(handle.index)
            .map(|w| w.content.clone())
            .unwrap_or_default()
    }

    /// Scroll the terminal view
    pub fn scroll_terminal(&mut self, _handle: &TerminalHandle, delta: i32) {
        self.scroll(delta as f64);
    }

    /// Scroll by a delta amount
    pub fn scroll(&mut self, delta: f64) {
        // Use actual heights for scroll calculation (matches rendering)
        let actual_total: i32 = self.actual_heights().iter().sum();
        let total_height = self.terminal_total_height as u32 + actual_total as u32;
        let max_scroll = total_height.saturating_sub(self.output_size.1) as f64;
        self.scroll_offset = (self.scroll_offset + delta).clamp(0.0, max_scroll);
    }

    /// Set scroll offset directly
    pub fn set_scroll(&mut self, offset: f64) {
        // Use actual heights for scroll calculation (matches rendering)
        let actual_total: i32 = self.actual_heights().iter().sum();
        let total_height = self.terminal_total_height as u32 + actual_total as u32;
        let max_scroll = total_height.saturating_sub(self.output_size.1) as f64;
        self.scroll_offset = offset.clamp(0.0, max_scroll);
    }

    /// Get current scroll offset
    pub fn scroll_offset(&self) -> f64 {
        self.scroll_offset
    }

    /// Get total content height (terminals + windows) using actual heights
    pub fn total_content_height(&self) -> i32 {
        self.terminal_total_height + self.actual_heights().iter().sum::<i32>()
    }

    /// Get output size
    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }

    /// Get terminal info (scroll_offset, terminal_total_height)
    pub fn terminal_info(&self) -> (f64, i32) {
        (self.scroll_offset, self.terminal_total_height)
    }

    /// Check if a window is visible on screen (considering scroll)
    pub fn is_window_visible(&self, index: usize) -> bool {
        let render_pos = self.render_positions();
        if let Some(&(y, height)) = render_pos.get(index) {
            let window_bottom = y + height;
            let screen_height = self.output_size.1 as i32;
            // Window is visible if any part is on screen
            window_bottom > 0 && y < screen_height
        } else {
            false
        }
    }

    /// Get the visible portion of a window (start_y, end_y) in screen coordinates
    /// Returns None if window is not visible
    pub fn visible_portion(&self, index: usize) -> Option<(i32, i32)> {
        let render_pos = self.render_positions();
        if let Some(&(y, height)) = render_pos.get(index) {
            let window_bottom = y + height;
            let screen_height = self.output_size.1 as i32;

            if window_bottom <= 0 || y >= screen_height {
                None
            } else {
                let visible_top = y.max(0);
                let visible_bottom = window_bottom.min(screen_height);
                Some((visible_top, visible_bottom))
            }
        } else {
            None
        }
    }

    // ===== Input Simulation Methods =====

    /// Simulate a pointer motion event in screen coordinates (Y=0 at top)
    ///
    /// This mirrors the real compositor's handle_pointer_motion_absolute:
    /// it converts screen coordinates to render coordinates (Y=0 at bottom).
    pub fn simulate_pointer_motion(&mut self, screen_x: f64, screen_y: f64) {
        let screen_point = ScreenPoint::new(screen_x, screen_y);
        self.pointer_location = screen_point.to_render(self.output_size.1 as i32);
    }

    /// Simulate a click at screen coordinates (Y=0 at top)
    ///
    /// Moves the pointer to the location and then simulates a click.
    /// Updates focus based on what's under the pointer.
    pub fn simulate_click(&mut self, screen_x: f64, screen_y: f64) {
        // Move pointer (converts screen to render coordinates)
        self.simulate_pointer_motion(screen_x, screen_y);

        // Find what's under the pointer (using render coordinates)
        let render_y = self.pointer_location.y.value();
        if let Some(index) = self.window_at(render_y) {
            self.focused_index = Some(index);
        }
    }

    /// Simulate a scroll event
    ///
    /// Positive delta scrolls down (content moves up, showing lower content).
    /// This mirrors the real compositor's handle_pointer_axis behavior.
    pub fn simulate_scroll(&mut self, delta_y: f64) {
        // In the real compositor, scroll_requested is negated because
        // we flip Y coordinates for OpenGL compatibility
        // Here we just apply the delta directly
        self.scroll(delta_y);
    }

    /// Get current pointer location in render coordinates (Y=0 at bottom)
    pub fn pointer_location(&self) -> RenderPoint {
        self.pointer_location
    }

    /// Get current pointer location as a tuple (x, y) in render coordinates
    pub fn pointer_location_tuple(&self) -> (f64, f64) {
        (self.pointer_location.x, self.pointer_location.y.value())
    }

    // ===== Popup Methods =====

    /// Add a popup attached to a parent window
    ///
    /// Returns the popup index for later reference.
    pub fn add_popup(&mut self, parent_index: usize, offset: (i32, i32), size: (i32, i32)) -> usize {
        let id = self.popups.len();
        self.popups.push(MockPopup {
            parent_index,
            offset,
            size,
            has_grab: false,
        });
        id
    }

    /// Remove a popup by index
    pub fn remove_popup(&mut self, popup_id: usize) {
        if popup_id < self.popups.len() {
            self.popups.remove(popup_id);
        }
    }

    /// Set grab state for a popup
    pub fn set_popup_grab(&mut self, popup_id: usize, has_grab: bool) {
        if let Some(popup) = self.popups.get_mut(popup_id) {
            popup.has_grab = has_grab;
        }
    }

    /// Get popup's screen position considering parent window position and scroll
    ///
    /// Returns (x, y) in screen coordinates, or None if parent doesn't exist.
    pub fn popup_screen_position(&self, popup_id: usize) -> Option<(i32, i32)> {
        let popup = self.popups.get(popup_id)?;
        let render_positions = self.render_positions();
        let (parent_y, _) = render_positions.get(popup.parent_index)?;

        // Popup position is parent position + offset
        // Note: popup offset Y is relative to parent's top (in content coords)
        Some((popup.offset.0, parent_y + popup.offset.1))
    }

    /// Find popup at a screen position (considering popups render on top)
    ///
    /// Returns popup index if found.
    pub fn popup_at(&self, x: i32, y: i32) -> Option<usize> {
        // Check popups in reverse order (last added = on top)
        for (i, popup) in self.popups.iter().enumerate().rev() {
            if let Some((px, py)) = self.popup_screen_position(i) {
                if x >= px && x < px + popup.size.0 &&
                   y >= py && y < py + popup.size.1 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Get all popups
    pub fn popups(&self) -> &[MockPopup] {
        &self.popups
    }

    /// Check if any popup has an active grab
    pub fn has_popup_grab(&self) -> bool {
        self.popups.iter().any(|p| p.has_grab)
    }
}
