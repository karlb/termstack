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
use crate::render::TerminalRenderer;
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
}

impl Terminal {
    /// Create a new terminal running an interactive shell
    pub fn new(cols: u16, rows: u16) -> Result<Self, TerminalError> {
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

        // Create renderer with font
        let font_config = crate::render::FontConfig::default_font();
        let renderer = TerminalRenderer::with_font(font_config);

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

        // Create renderer with font
        let font_config = crate::render::FontConfig::default_font();
        let renderer = TerminalRenderer::with_font(font_config);

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

                    // Count line endings for sizing state machine
                    // Only count \n (newline) - \r is cursor control, not line advancement
                    let newlines = buf[..n].iter().filter(|&&b| b == b'\n').count();

                    // Log for debugging (INFO level for visibility)
                    if newlines > 0 {
                        tracing::info!(bytes = n, newlines, was_alt, "PTY output with newlines");
                    }

                    // Process bytes through VTE parser
                    for byte in &buf[..n] {
                        self.parser.advance(&mut *term, *byte);
                    }

                    // Check if in alternate screen AFTER processing
                    let is_alt = term.mode().contains(TermMode::ALT_SCREEN);

                    drop(term);

                    // Only count line endings when NOT in alternate screen mode
                    // TUI apps use alternate screen and their output shouldn't affect content rows
                    if !is_alt && !was_alt && newlines > 0 {
                        tracing::info!(newlines, content_rows = self.sizing.content_rows(), "counting line endings");
                        for _ in 0..newlines {
                            let action = self.sizing.on_new_line();
                            if action != SizingAction::None {
                                tracing::info!(?action, "sizing action from line ending");
                                actions.push(action);
                            }
                        }
                    } else if newlines > 0 && (is_alt || was_alt) {
                        tracing::info!(newlines, was_alt, is_alt, "skipping line endings in alternate screen");
                    }
                }
                Err(_) => break,
            }
        }

        if total_read > 0 {
            tracing::info!(bytes = total_read, "read from PTY");
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

    /// Write input to terminal
    pub fn write(&mut self, data: &[u8]) -> Result<(), TerminalError> {
        self.pty.write(data)?;
        Ok(())
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
    /// Only resizes the PTY (what programs see), NOT the grid.
    /// The grid stays large (1000 rows) to hold all content without scrolling.
    /// Visual clipping is handled by the render function.
    pub fn configure(&mut self, rows: u16) -> SizingAction {
        let action = self.sizing.on_configure(rows);

        if let SizingAction::ApplyResize { rows } = action {
            // Only resize PTY - programs see the new size for cursor positioning
            // Do NOT resize the grid - it stays large to hold all content
            let _ = self.pty.resize(self.cols, rows);
            // Track the actual PTY size (what programs see)
            self.pty_rows = rows;
        }

        action
    }

    /// Complete resize (called after terminal processes the resize)
    pub fn complete_resize(&mut self) -> SizingAction {
        self.sizing.on_resize_complete()
    }

    /// Request growth (transition from stable to growth requested)
    pub fn request_growth(&mut self, target_rows: u16) {
        self.sizing.request_growth(target_rows);
    }

    /// Render to pixel buffer
    pub fn render(&mut self, width: u32, height: u32, show_cursor: bool) {
        let term = self.term.lock();
        self.renderer.render(&term, width, height, show_cursor);
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
    /// Returns the content of each visible line in the grid.
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
}
