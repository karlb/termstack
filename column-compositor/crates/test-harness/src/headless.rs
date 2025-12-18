//! Headless compositor wrapper for testing

use std::time::{Duration, Instant};

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

    /// Scroll offset
    scroll_offset: f64,

    /// Focused index
    focused_index: Option<usize>,
}

struct MockWindow {
    height: u32,
    content: String,
}

impl TestCompositor {
    /// Create a new headless test compositor
    pub fn new_headless(width: u32, height: u32) -> Self {
        Self {
            output_size: (width, height),
            windows: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
        }
    }

    /// Spawn a terminal and return a handle
    pub fn spawn_terminal(&mut self) -> TerminalHandle {
        let index = self.windows.len();

        self.windows.push(MockWindow {
            height: 200,
            content: String::new(),
        });

        self.focused_index = Some(index);

        TerminalHandle { index }
    }

    /// Send input to a terminal
    pub fn send_input(&mut self, handle: &TerminalHandle, input: &str) {
        if let Some(window) = self.windows.get_mut(handle.index) {
            // Simulate output from command
            window.content.push_str(input);

            // Count newlines and grow window
            let newlines = input.chars().filter(|&c| c == '\n').count();
            let line_height = 16u32; // Approximate
            window.height += (newlines as u32) * line_height;
        }
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
            window_heights: self.windows.iter().map(|w| w.height).collect(),
            scroll_offset: self.scroll_offset,
            total_height: self.windows.iter().map(|w| w.height).sum(),
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
        let max_scroll = self
            .windows
            .iter()
            .map(|w| w.height)
            .sum::<u32>()
            .saturating_sub(self.output_size.1) as f64;

        self.scroll_offset = (self.scroll_offset + delta as f64).clamp(0.0, max_scroll);
    }

    /// Get output size
    pub fn output_size(&self) -> (u32, u32) {
        self.output_size
    }
}
