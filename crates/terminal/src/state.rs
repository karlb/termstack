//! Terminal state and management
//!
//! Wraps alacritty_terminal with PTY and sizing state machine.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::vte::ansi;

use crate::pty::{Pty, PtyError};
use crate::render::{Theme, TerminalRenderer};
use crate::sizing::{SizingAction, TerminalSizingState};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum TerminalError {
    #[error("PTY error: {0}")]
    Pty(#[from] PtyError),

    #[error("terminal initialization failed")]
    Init,
}

/// Event listener for terminal events
pub struct TerminalEventProxy {
    /// Channel for sending events
    sender: std::sync::mpsc::Sender<TerminalEvent>,
}

impl EventListener for TerminalEventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.sender.send(TerminalEvent::Alacritty(event));
    }
}

/// Events from the terminal
#[derive(Debug)]
pub enum TerminalEvent {
    /// Event from alacritty_terminal
    Alacritty(Event),

    /// Sizing action needed
    Sizing(SizingAction),

    /// Terminal exited
    Exited,
}

/// Simple size struct implementing Dimensions
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

/// A terminal window
pub struct Terminal {
    /// Terminal state from alacritty
    term: Arc<FairMutex<Term<TerminalEventProxy>>>,

    /// VTE parser
    parser: ansi::Processor,

    /// PTY handle
    pty: Pty,

    /// Sizing state machine
    sizing: TerminalSizingState,

    /// Renderer
    renderer: TerminalRenderer,

    /// Event receiver
    events: std::sync::mpsc::Receiver<TerminalEvent>,

    /// Columns
    cols: u16,

    /// Grid rows (internal alacritty grid size, stays large)
    /// Note: This field is stored for documentation/debugging but not read;
    /// actual grid size is queried via grid_rows() which calls term.screen_lines()
    _grid_rows: u16,

    /// PTY rows (what programs see via tcgetwinsize)
    pty_rows: u16,

    /// Viewport offset for scrollback navigation
    /// 0 = showing live output (cursor at bottom)
    /// >0 = scrolled into history (showing older content)
    viewport_offset: usize,

    /// Visual rows from last render (for scroll clamping)
    /// This is updated during render() and used by scroll_display()
    /// to properly clamp the viewport offset to the visual maximum.
    last_visual_rows: usize,
}

impl Terminal {
    /// Create a new terminal running an interactive shell
    pub fn new(cols: u16, rows: u16) -> Result<Self, TerminalError> {
        Self::new_with_options(cols, rows, Theme::default(), 14.0)
    }

    /// Create a new terminal running an interactive shell with theme
    pub fn new_with_theme(cols: u16, rows: u16, theme: Theme) -> Result<Self, TerminalError> {
        Self::new_with_options(cols, rows, theme, 14.0)
    }

    /// Create a new terminal running an interactive shell with theme and font size
    pub fn new_with_options(cols: u16, rows: u16, theme: Theme, font_size: f32) -> Result<Self, TerminalError> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        // Use 1000 rows for PTY and grid to prevent internal scrolling
        // The sizing state uses `rows` for growth triggers
        let pty_rows = 1000u16;

        // Create PTY with large size so shell doesn't scroll internally
        let pty = Pty::spawn(&shell, cols, pty_rows)?;

        // Create event channel
        let (sender, receiver) = std::sync::mpsc::channel();
        let event_proxy = TerminalEventProxy { sender };

        // Create terminal grid with large size to store all output
        let config = TermConfig::default();
        let size = Size {
            cols: cols as usize,
            rows: pty_rows as usize,
        };

        let term = Term::new(config, &size, event_proxy);
        let term = Arc::new(FairMutex::new(term));

        // Create VTE parser
        let parser = ansi::Processor::new();

        // Create renderer with font and theme
        let font_config = crate::render::FontConfig::default_font_with_size(font_size);
        let renderer = TerminalRenderer::with_font_and_theme(font_config, theme);

        // Create sizing state with visual row count (will grow as content arrives)
        let sizing = TerminalSizingState::new(rows);

        Ok(Self {
            term,
            parser,
            pty,
            sizing,
            renderer,
            events: receiver,
            cols,
            _grid_rows: pty_rows,
            pty_rows,
            viewport_offset: 0,
            last_visual_rows: rows as usize,
        })
    }

    /// Create a new terminal running a specific command
    ///
    /// The command is run via `/bin/sh -c "command"` with the given
    /// working directory and environment variables.
    ///
    /// - `pty_rows`: Size reported to the PTY (program sees this many rows)
    /// - `visual_rows`: Initial visual size (sizing state uses this for growth triggers)
    ///
    /// Using a large pty_rows with small visual_rows prevents programs from
    /// scrolling while keeping the terminal visually minimal.
    pub fn new_with_command(
        cols: u16,
        pty_rows: u16,
        visual_rows: u16,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
    ) -> Result<Self, TerminalError> {
        Self::new_with_command_options(cols, pty_rows, visual_rows, command, working_dir, env, Theme::default(), 14.0)
    }

    /// Create a new terminal running a specific command with theme
    pub fn new_with_command_and_theme(
        cols: u16,
        pty_rows: u16,
        visual_rows: u16,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        theme: Theme,
    ) -> Result<Self, TerminalError> {
        Self::new_with_command_options(cols, pty_rows, visual_rows, command, working_dir, env, theme, 14.0)
    }

    /// Create a new terminal running a specific command with theme and font size
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_command_options(
        cols: u16,
        pty_rows: u16,
        visual_rows: u16,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        theme: Theme,
        font_size: f32,
    ) -> Result<Self, TerminalError> {
        // Create PTY with large size (no scrolling)
        let pty = Pty::spawn_command(command, working_dir, env, cols, pty_rows)?;

        // Create event channel
        let (sender, receiver) = std::sync::mpsc::channel();
        let event_proxy = TerminalEventProxy { sender };

        // Create terminal grid with large size to store all output
        let config = TermConfig::default();
        let size = Size {
            cols: cols as usize,
            rows: pty_rows as usize,
        };

        let term = Term::new(config, &size, event_proxy);
        let term = Arc::new(FairMutex::new(term));

        // Create VTE parser
        let parser = ansi::Processor::new();

        // Create renderer with font and theme
        let font_config = crate::render::FontConfig::default_font_with_size(font_size);
        let renderer = TerminalRenderer::with_font_and_theme(font_config, theme);

        // Create sizing state with VISUAL rows (triggers growth based on visual size)
        let sizing = TerminalSizingState::new(visual_rows);

        Ok(Self {
            term,
            parser,
            pty,
            sizing,
            renderer,
            events: receiver,
            cols,
            _grid_rows: pty_rows,
            pty_rows,
            viewport_offset: 0,
            last_visual_rows: visual_rows as usize,
        })
    }

    /// Process PTY output and terminal events
    pub fn process_pty(&mut self) -> Vec<SizingAction> {
        self.process_pty_with_count().0
    }

    /// Process PTY output and terminal events, returning (actions, bytes_read)
    pub fn process_pty_with_count(&mut self) -> (Vec<SizingAction>, usize) {
        let mut actions = Vec::new();
        let mut buf = [0u8; 4096];
        let mut total_read = 0;

        loop {
            match self.pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total_read += n;
                    let mut term = self.term.lock();

                    // Check if in alternate screen BEFORE processing (for logging)
                    let was_alt = term.mode().contains(TermMode::ALT_SCREEN);

                    // Process bytes through VTE parser
                    for byte in &buf[..n] {
                        self.parser.advance(&mut *term, *byte);
                    }

                    // Check if in alternate screen AFTER processing
                    let is_alt = term.mode().contains(TermMode::ALT_SCREEN);

                    // Use last non-empty line for growth decisions
                    // This avoids showing empty rows when cursor is on an empty line
                    if !is_alt && !was_alt {
                        let cursor_line = term.grid().cursor.point.line.0 as u16;
                        let visual_rows = self.sizing.current_rows();

                        // Find last non-empty line for content-based sizing
                        let last_content = {
                            let grid = term.grid();
                            let mut last = 0u16;
                            for line_idx in (0..=cursor_line).rev() {
                                let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
                                let has_content = line.into_iter().any(|cell| {
                                    let c = cell.c;
                                    c != ' ' && c != '\0'
                                });
                                if has_content {
                                    last = line_idx;
                                    break;
                                }
                            }
                            last
                        };

                        // Update content_rows to last content line + 1 (0-indexed)
                        let content_line = (last_content + 1) as u32;
                        if content_line > self.sizing.content_rows() {
                            while self.sizing.content_rows() < content_line {
                                self.sizing.on_new_line();
                            }
                        }

                        // Request growth if content exceeds visual rows
                        if last_content >= visual_rows {
                            let target_rows = last_content + 1;
                            tracing::debug!(cursor_line, last_content, visual_rows, target_rows, "content exceeded visual size, requesting growth");
                            actions.push(SizingAction::RequestGrowth { target_rows });
                        }
                    }

                    drop(term);
                }
                Err(_) => break,
            }
        }

        // Process terminal events (e.g., PtyWrite for terminal query responses)
        for event in self.events.try_iter() {
            if let TerminalEvent::Alacritty(Event::PtyWrite(text)) = event {
                tracing::debug!(len = text.len(), "writing terminal response to PTY");
                if let Err(e) = self.pty.write(text.as_bytes()) {
                    tracing::warn!("failed to write terminal response: {:?}", e);
                }
            }
        }

        (actions, total_read)
    }

    /// Write input to terminal (non-blocking)
    ///
    /// Returns the number of bytes written. May return less than `data.len()`
    /// if the PTY buffer is full. Caller should buffer any unwritten data.
    pub fn write(&mut self, data: &[u8]) -> Result<usize, TerminalError> {
        Ok(self.pty.write(data)?)
    }

    /// Directly process bytes through terminal emulator (for testing)
    ///
    /// Unlike process_pty, this doesn't read from PTY but directly feeds
    /// the given bytes to the VTE parser. Useful for simulating terminal
    /// output in tests.
    pub fn inject_bytes(&mut self, data: &[u8]) {
        let mut term = self.term.lock();
        let was_alt = term.mode().contains(TermMode::ALT_SCREEN);

        // Count only \n for line endings - \r is cursor control
        let newlines = data.iter().filter(|&&b| b == b'\n').count();

        for byte in data {
            self.parser.advance(&mut *term, *byte);
        }

        let is_alt = term.mode().contains(TermMode::ALT_SCREEN);
        drop(term);

        // Only count line endings when NOT in alternate screen mode
        if !is_alt && !was_alt && newlines > 0 {
            for _ in 0..newlines {
                self.sizing.on_new_line();
            }
        }
    }

    /// Handle compositor configure (resize)
    ///
    /// Resizes the PTY (what programs see).
    /// For primary screen: grid stays large (1000 rows) to preserve scrollback.
    /// For alternate screen: grid MUST match PTY size for correct TUI rendering.
    pub fn configure(&mut self, rows: u16) -> SizingAction {
        let action = self.sizing.on_configure(rows);

        if let SizingAction::ApplyResize { rows } = action {
            // Resize PTY so programs see the new size for cursor positioning
            let _ = self.pty.resize(self.cols, rows);
            // Track the actual PTY size (what programs see)
            self.pty_rows = rows;

            // CRITICAL: In alternate screen mode, grid MUST match PTY size
            // TUI apps draw expecting the grid size to match what PTY reports
            // Only keep large grid for primary screen (scrollback preservation)
            if self.is_alternate_screen() {
                let size = Size {
                    cols: self.cols as usize,
                    rows: rows as usize,
                };
                let mut term = self.term.lock();
                term.resize(size);
            }
        }

        action
    }

    /// Complete resize (called after terminal processes the resize)
    pub fn complete_resize(&mut self) -> SizingAction {
        self.sizing.on_resize_complete()
    }

    /// Resize columns (width change from compositor resize)
    ///
    /// This resizes both the PTY and the alacritty terminal grid to the new column count.
    pub fn resize_cols(&mut self, cols: u16) {
        if cols == self.cols {
            return;
        }

        self.cols = cols;

        // Resize PTY so programs see new width
        let _ = self.pty.resize(cols, self.pty_rows);

        // Resize alacritty terminal grid
        let size = Size {
            cols: cols as usize,
            rows: self.grid_rows() as usize,
        };
        let mut term = self.term.lock();
        term.resize(size);
    }

    /// Request growth (transition from stable to growth requested)
    pub fn request_growth(&mut self, target_rows: u16) {
        self.sizing.request_growth(target_rows);
    }

    /// Render to pixel buffer
    pub fn render(&mut self, width: u32, height: u32, show_cursor: bool) {
        // Update last_visual_rows for scroll clamping
        let (_, cell_height) = self.renderer.cell_size();
        self.last_visual_rows = (height / cell_height).max(1) as usize;

        let term = self.term.lock();
        self.renderer.render(&term, width, height, show_cursor, self.viewport_offset);
    }

    /// Get rendered pixel buffer
    pub fn buffer(&self) -> &[u32] {
        self.renderer.buffer()
    }

    /// Get cell size
    pub fn cell_size(&self) -> (u32, u32) {
        self.renderer.cell_size()
    }

    /// Get current PTY dimensions (what programs see via tcgetwinsize)
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.pty_rows)
    }

    /// Get actual grid rows from alacritty terminal
    /// This is the number of rows the terminal grid can display
    pub fn grid_rows(&self) -> u16 {
        let term = self.term.lock();
        term.screen_lines() as u16
    }

    /// Check if terminal is in alternate screen mode (used by TUI apps like vim, fzf, mc)
    pub fn is_alternate_screen(&self) -> bool {
        let term = self.term.lock();
        term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Get cursor line (0-indexed row where cursor is)
    /// This reflects actual content position in the primary screen
    pub fn cursor_line(&self) -> u16 {
        let term = self.term.lock();
        term.grid().cursor.point.line.0 as u16
    }

    /// Get the last line with non-empty content (0-indexed).
    /// This is useful for sizing: we want to show content up to the last
    /// non-empty line, not necessarily where the cursor is.
    /// Returns cursor_line if the cursor line has content.
    pub fn last_content_line(&self) -> u16 {
        let term = self.term.lock();
        let grid = term.grid();
        let cursor_line = grid.cursor.point.line.0 as u16;

        // Start from cursor line and work backwards to find last non-empty line
        for line_idx in (0..=cursor_line).rev() {
            let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
            let has_content = line.into_iter().any(|cell| {
                let c = cell.c;
                c != ' ' && c != '\0'
            });
            if has_content {
                return line_idx;
            }
        }

        // All lines empty, return 0
        0
    }

    /// Check if terminal has any meaningful (non-whitespace) content.
    ///
    /// This is different from content_rows() > 0 because a terminal can have
    /// cursor movement (e.g., just newlines) without any visible characters.
    /// Used for visibility decisions: we only want to show output terminals
    /// that have actual content to display.
    pub fn has_meaningful_content(&self) -> bool {
        let term = self.term.lock();
        let grid = term.grid();
        let cursor_line = grid.cursor.point.line.0 as u16;

        // Check all lines from 0 to cursor for any non-whitespace character
        for line_idx in 0..=cursor_line {
            let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
            let has_content = line.into_iter().any(|cell| {
                let c = cell.c;
                c != ' ' && c != '\0'
            });
            if has_content {
                return true;
            }
        }

        false
    }

    /// Get content row count
    pub fn content_rows(&self) -> u32 {
        self.sizing.content_rows()
    }

    /// Check if terminal is running
    pub fn is_running(&mut self) -> bool {
        self.pty.is_running()
    }

    /// Get PTY fd for polling
    pub fn pty_fd(&self) -> std::os::fd::RawFd {
        self.pty.as_raw_fd()
    }

    /// Get sizing state
    pub fn sizing_state(&self) -> &TerminalSizingState {
        &self.sizing
    }

    /// Get grid content as text lines (for debugging)
    ///
    /// Returns the content of each line in the grid (all 1000 lines).
    pub fn grid_content(&self) -> Vec<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let mut lines = Vec::new();

        for line_idx in 0..term.screen_lines() {
            let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
            let mut text = String::new();
            for cell in line.into_iter() {
                let c = cell.c;
                if c == '\0' {
                    text.push(' ');
                } else {
                    text.push(c);
                }
            }
            // Trim trailing spaces
            let trimmed = text.trim_end();
            lines.push(trimmed.to_string());
        }

        lines
    }

    /// Get visible content as text lines based on cursor position
    ///
    /// Returns the `num_rows` lines that would be visible if we render.
    /// Shows from line 0 if content fits, otherwise shows ending at last content line.
    pub fn visible_content(&self, num_rows: usize) -> Vec<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let cursor_line = term.grid().cursor.point.line.0 as usize;

        // Find last non-empty line for content-based positioning
        let last_content = {
            let mut last = 0;
            for line_idx in (0..=cursor_line).rev() {
                let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
                let has_content = line.into_iter().any(|cell| {
                    let c = cell.c;
                    c != ' ' && c != '\0'
                });
                if has_content {
                    last = line_idx;
                    break;
                }
            }
            last
        };

        // If content fits in viewport, show from line 0
        // If content exceeds viewport, show ending at last content line
        let start_line = if self.viewport_offset > 0 {
            // User scrolled back
            cursor_line
                .saturating_sub(num_rows - 1)
                .saturating_sub(self.viewport_offset)
        } else if last_content < num_rows {
            // Content fits - show from beginning
            0
        } else {
            // Content exceeds viewport - show ending at last content
            last_content.saturating_sub(num_rows - 1)
        };
        let end_line = (start_line + num_rows).min(term.screen_lines());

        let mut lines = Vec::new();
        for line_idx in start_line..end_line {
            let line = &grid[alacritty_terminal::index::Line(line_idx as i32)];
            let mut text = String::new();
            for cell in line.into_iter() {
                let c = cell.c;
                if c == '\0' {
                    text.push(' ');
                } else {
                    text.push(c);
                }
            }
            let trimmed = text.trim_end();
            lines.push(trimmed.to_string());
        }

        lines
    }

    /// Check pending events
    pub fn poll_events(&self) -> impl Iterator<Item = TerminalEvent> + '_ {
        self.events.try_iter()
    }

    /// Start a text selection at the given grid coordinates
    pub fn start_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        let point = Point::new(Line(row as i32), Column(col));
        term.selection = Some(Selection::new(SelectionType::Simple, point, Side::Left));
    }

    /// Update the selection end point
    pub fn update_selection(&self, col: usize, row: usize) {
        let mut term = self.term.lock();
        if let Some(ref mut selection) = term.selection {
            let point = Point::new(Line(row as i32), Column(col));
            selection.update(point, Side::Right);
        }
    }

    /// Clear the current selection
    pub fn clear_selection(&self) {
        let mut term = self.term.lock();
        term.selection = None;
    }

    /// Get the selected text, if any
    pub fn selection_text(&self) -> Option<String> {
        let term = self.term.lock();
        term.selection_to_string()
    }

    /// Check if there's an active selection
    pub fn has_selection(&self) -> bool {
        let term = self.term.lock();
        term.selection.is_some()
    }

    /// Scroll the terminal viewport into scrollback history
    ///
    /// Positive lines = scroll up (back in history)
    /// Negative lines = scroll down (toward live output)
    pub fn scroll_display(&mut self, lines: i32) {
        let term = self.term.lock();
        let cursor_line = term.grid().cursor.point.line.0 as usize;
        drop(term);

        // Calculate new offset
        let new_offset = if lines > 0 {
            self.viewport_offset.saturating_add(lines as usize)
        } else {
            self.viewport_offset.saturating_sub((-lines) as usize)
        };

        // Clamp to valid visual range.
        // The visual maximum is when first_visible_line would be 0.
        // With the render formula: first_visible_line = (cursor_line + 1) - (visible_rows - 1) - viewport_offset
        // Setting first_visible_line = 0: max_offset = cursor_line + 2 - visible_rows
        let visible_rows = self.last_visual_rows;
        let max_visual_offset = if visible_rows <= 1 {
            cursor_line // degenerate case
        } else {
            (cursor_line + 1).saturating_sub(visible_rows - 1)
        };
        self.viewport_offset = new_offset.min(max_visual_offset);
    }

    /// Returns true if terminal has scrollback history available
    /// (content above the current viewport)
    pub fn has_scrollback(&self) -> bool {
        let term = self.term.lock();
        let cursor_line = term.grid().cursor.point.line.0 as usize;
        // We have scrollback if cursor is past line 0
        cursor_line > 0
    }

    /// Get current scroll offset (0 = live output, >0 = scrolled into history)
    pub fn display_offset(&self) -> usize {
        self.viewport_offset
    }

    /// Reset viewport to show live output (scroll to bottom)
    pub fn scroll_to_bottom(&mut self) {
        self.viewport_offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_starts_and_clears() {
        let terminal = Terminal::new(80, 24).expect("terminal creation");

        // Initially no selection
        assert!(!terminal.has_selection());
        assert!(terminal.selection_text().is_none());

        // Start a selection
        terminal.start_selection(0, 0);
        assert!(terminal.has_selection());

        // Clear selection
        terminal.clear_selection();
        assert!(!terminal.has_selection());
    }

    #[test]
    fn selection_can_be_updated() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");

        // Inject some content
        terminal.inject_bytes(b"Hello World\r\n");

        // Start and update selection
        terminal.start_selection(0, 0);
        terminal.update_selection(4, 0); // Select "Hello"

        assert!(terminal.has_selection());

        // The selection should have some text (may vary based on alacritty internals)
        let text = terminal.selection_text();
        assert!(text.is_some(), "selection should have text");
    }

    #[test]
    fn selection_text_returns_selected_content() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");

        // Inject content - note: need to wait for VTE to process
        terminal.inject_bytes(b"ABCDEFGHIJ\r\n");

        // Select first 5 chars (A-E)
        terminal.start_selection(0, 0);
        terminal.update_selection(4, 0);

        let text = terminal.selection_text();
        assert!(text.is_some(), "should have selection text");

        // The text should contain at least some of what we wrote
        let text = text.unwrap();
        assert!(!text.is_empty(), "selection text should not be empty");
    }

    #[test]
    fn selection_survives_new_output() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");

        // Inject initial content
        terminal.inject_bytes(b"Line 1\r\n");

        // Make a selection
        terminal.start_selection(0, 0);
        terminal.update_selection(3, 0);
        assert!(terminal.has_selection());

        // Inject more content (shouldn't clear selection)
        terminal.inject_bytes(b"Line 2\r\n");

        // Selection should still exist
        assert!(terminal.has_selection());
    }

    #[test]
    fn scroll_display_changes_offset() {
        // Test that scroll_display actually changes the display offset
        let mut terminal = Terminal::new(80, 10).expect("terminal creation");

        // Inject enough output to have scrollback
        for i in 1..=100 {
            let line = format!("{}\r\n", i);
            terminal.inject_bytes(line.as_bytes());
        }

        // Initially at display_offset 0 (showing latest)
        assert_eq!(terminal.display_offset(), 0, "initial offset should be 0");

        // Scroll up into history (positive = scroll up)
        terminal.scroll_display(10);
        let offset_after_up = terminal.display_offset();
        eprintln!("After scroll_display(10): offset = {}", offset_after_up);
        assert!(offset_after_up > 0, "scroll up should increase offset, got {}", offset_after_up);

        // Scroll back down (negative = scroll down toward live)
        terminal.scroll_display(-5);
        let offset_after_down = terminal.display_offset();
        eprintln!("After scroll_display(-5): offset = {}", offset_after_down);
        assert!(offset_after_down < offset_after_up, "scroll down should decrease offset");
    }

    #[test]
    fn terminal_shows_latest_output_not_first() {
        // Bug: seq 500 shows lines 1-47 instead of latest output
        // The terminal should auto-scroll to show newest content
        let mut terminal = Terminal::new(80, 10).expect("terminal creation");

        // Simulate `seq 1 100` output - 100 lines
        for i in 1..=100 {
            let line = format!("{}\r\n", i);
            terminal.inject_bytes(line.as_bytes());
        }

        // Render the terminal at 10 rows height
        let (cell_w, cell_h) = terminal.cell_size();
        let width = 80 * cell_w;
        let height = 10 * cell_h;
        terminal.render(width, height, false);

        // The cursor should be at line 100 (after 100 lines of output)
        let cursor_line = terminal.cursor_line();
        eprintln!("Cursor at line: {}", cursor_line);
        assert!(
            cursor_line >= 90,
            "Cursor should be near end of output (expected >=90, got {})",
            cursor_line
        );

        // Get the visible lines - this should return what's actually rendered
        // With cursor at line 100 and 10 rows visible, we should see lines 91-100
        let visible = terminal.visible_content(10);
        eprintln!("Visible content (should be 91-100):");
        for (i, line) in visible.iter().enumerate() {
            eprintln!("  [{}]: {:?}", i, line);
        }

        // The first visible line should be "91", not "1"
        let first_visible = visible.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
        assert!(
            first_visible.starts_with("9") || first_visible.starts_with("100"),
            "First visible line should be 91-100, not the start. Got: {:?}",
            first_visible
        );
    }

    #[test]
    fn scroll_to_beginning_shows_first_line() {
        // Bug: User can't scroll to the very beginning - missing first 3 lines
        let mut terminal = Terminal::new(80, 10).expect("terminal creation");

        // Simulate `seq 1 100` output - 100 lines
        for i in 1..=100 {
            let line = format!("{}\r\n", i);
            terminal.inject_bytes(line.as_bytes());
        }

        let cursor_line = terminal.cursor_line();
        eprintln!("Cursor at line: {}", cursor_line);

        // Scroll all the way up (large positive value)
        terminal.scroll_display(1000);
        let max_offset = terminal.display_offset();
        eprintln!("Max scroll offset: {}", max_offset);

        // Render the terminal at 10 rows height
        let (cell_w, cell_h) = terminal.cell_size();
        let width = 80 * cell_w;
        let height = 10 * cell_h;
        terminal.render(width, height, false);

        // Get the visible lines at max scroll
        let visible = terminal.visible_content(10);
        eprintln!("Visible content at max scroll (should start with 1):");
        for (i, line) in visible.iter().enumerate() {
            eprintln!("  [{}]: {:?}", i, line);
        }

        // The first line should be "1"
        let first_visible = visible.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
        assert!(
            first_visible == "1",
            "First visible line at max scroll should be '1', got: {:?}",
            first_visible
        );
    }

    #[test]
    fn scroll_to_beginning_with_realistic_viewport() {
        // Test with realistic viewport size (like real compositor)
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");

        // Simulate `seq 1 100` output
        for i in 1..=100 {
            let line = format!("{}\n", i);
            terminal.inject_bytes(line.as_bytes());
        }

        let cursor_line = terminal.cursor_line();
        let (cell_w, cell_h) = terminal.cell_size();

        // Use a realistic viewport height (e.g., 800 pixels with ~17px cell height = ~47 rows)
        let width = 80 * cell_w;
        let height = 47 * cell_h; // 47 visible rows
        let visible_rows = height / cell_h;

        eprintln!("cursor_line: {}", cursor_line);
        eprintln!("visible_rows: {}", visible_rows);
        eprintln!("cell_height: {}", cell_h);

        // Scroll all the way up
        terminal.scroll_display(1000);
        let max_offset = terminal.display_offset();
        eprintln!("max_offset after scroll(1000): {}", max_offset);

        // Calculate what first_visible_line should be
        let expected_first = (cursor_line as u32)
            .saturating_sub(visible_rows - 1)
            .saturating_sub(max_offset as u32);
        eprintln!("expected first_visible_line: {}", expected_first);

        // Render
        terminal.render(width, height, false);

        // Get visible content
        let visible = terminal.visible_content(visible_rows as usize);
        eprintln!("Visible content at max scroll:");
        for (i, line) in visible.iter().take(10).enumerate() {
            eprintln!("  [{}]: {:?}", i, line);
        }

        // First line should be "1"
        let first_visible = visible.iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
        assert_eq!(
            first_visible, "1",
            "First visible line at max scroll should be '1', got: {:?}",
            first_visible
        );
    }

    #[test]
    fn scroll_to_beginning_with_shell_prompt() {
        // Simulates real compositor scenario: shell prompt, then seq 100
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");

        // Simulate shell prompt (like what would happen in a real shell)
        terminal.inject_bytes(b"user@host:~$ seq 100\r\n");

        // Simulate seq 100 output
        for i in 1..=100 {
            let line = format!("{}\r\n", i);
            terminal.inject_bytes(line.as_bytes());
        }

        // Simulate shell prompt after command
        terminal.inject_bytes(b"user@host:~$ ");

        let cursor_line = terminal.cursor_line();
        let (_cell_w, cell_h) = terminal.cell_size();

        let height = 47 * cell_h;
        let visible_rows = height / cell_h;

        eprintln!("cursor_line: {}", cursor_line);
        eprintln!("visible_rows: {}", visible_rows);

        // Scroll all the way up
        terminal.scroll_display(1000);
        let max_offset = terminal.display_offset();
        eprintln!("max_offset: {}", max_offset);

        // Get visible content
        let visible = terminal.visible_content(visible_rows as usize);
        eprintln!("Visible content at max scroll (first 15 lines):");
        for (i, line) in visible.iter().take(15).enumerate() {
            eprintln!("  [{}]: {:?}", i, line);
        }

        // First line should be the original prompt
        let first_visible = visible.first().cloned().unwrap_or_default();
        assert!(
            first_visible.starts_with("user@host"),
            "First visible line should be shell prompt, got: {:?}",
            first_visible
        );
    }
}
