//! Terminal state and management
//!
//! Wraps alacritty_terminal with PTY and sizing state machine.

use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::Term;
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

    /// Rows
    rows: u16,
}

impl Terminal {
    /// Create a new terminal
    pub fn new(cols: u16, rows: u16) -> Result<Self, TerminalError> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

        // Create PTY
        let pty = Pty::spawn(&shell, cols, rows)?;

        // Create event channel
        let (sender, receiver) = std::sync::mpsc::channel();
        let event_proxy = TerminalEventProxy { sender };

        // Create terminal
        let config = TermConfig::default();
        let size = Size {
            cols: cols as usize,
            rows: rows as usize,
        };

        let term = Term::new(config, &size, event_proxy);
        let term = Arc::new(FairMutex::new(term));

        // Create VTE parser
        let parser = ansi::Processor::new();

        // Create renderer with font
        let font_config = crate::render::FontConfig::default_font();
        let renderer = TerminalRenderer::with_font(font_config);

        // Create sizing state
        let sizing = TerminalSizingState::new(rows);

        Ok(Self {
            term,
            parser,
            pty,
            sizing,
            renderer,
            events: receiver,
            cols,
            rows,
        })
    }

    /// Process PTY output
    pub fn process_pty(&mut self) -> Vec<SizingAction> {
        let mut actions = Vec::new();
        let mut buf = [0u8; 4096];
        let mut total_read = 0;

        loop {
            match self.pty.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    total_read += n;
                    let mut term = self.term.lock();

                    // Count newlines for sizing state machine
                    let newlines = buf[..n].iter().filter(|&&b| b == b'\n').count();

                    // Process bytes through VTE parser
                    for byte in &buf[..n] {
                        self.parser.advance(&mut *term, *byte);
                    }

                    drop(term);

                    // Update sizing state for each newline
                    for _ in 0..newlines {
                        let action = self.sizing.on_new_line();
                        if action != SizingAction::None {
                            actions.push(action);
                        }
                    }
                }
                Err(_) => break,
            }
        }

        if total_read > 0 {
            tracing::debug!(bytes = total_read, "read from PTY");
        }

        actions
    }

    /// Write input to terminal
    pub fn write(&mut self, data: &[u8]) -> Result<(), TerminalError> {
        self.pty.write(data).map_err(PtyError::from)?;
        Ok(())
    }

    /// Handle compositor configure (resize)
    pub fn configure(&mut self, rows: u16) -> SizingAction {
        let action = self.sizing.on_configure(rows);

        if let SizingAction::ApplyResize { rows } = action {
            let _ = self.pty.resize(self.cols, rows);
            self.rows = rows;

            // Update terminal size
            let mut term = self.term.lock();
            let size = Size {
                cols: self.cols as usize,
                rows: rows as usize,
            };
            term.resize(size);
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
    pub fn render(&mut self, width: u32, height: u32) {
        let term = self.term.lock();
        self.renderer.render(&term, width, height);
    }

    /// Get rendered pixel buffer
    pub fn buffer(&self) -> &[u32] {
        self.renderer.buffer()
    }

    /// Get cell size
    pub fn cell_size(&self) -> (u32, u32) {
        self.renderer.cell_size()
    }

    /// Get current dimensions
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
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

    /// Check pending events
    pub fn poll_events(&self) -> impl Iterator<Item = TerminalEvent> + '_ {
        self.events.try_iter()
    }
}
