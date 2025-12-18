//! Internal terminal management
//!
//! Manages spawning and rendering of internal terminals.

use std::collections::HashMap;
use std::os::fd::RawFd;

use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::ImportMem;
use smithay::utils::Size;

use terminal::Terminal;
use terminal::sizing::SizingAction;

/// Unique identifier for a managed terminal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(pub u32);

/// A managed internal terminal
pub struct ManagedTerminal {
    /// The terminal instance
    pub terminal: Terminal,

    /// Terminal ID
    pub id: TerminalId,

    /// Pixel width
    pub width: u32,

    /// Pixel height
    pub height: u32,

    /// Cached texture for rendering
    texture: Option<GlesTexture>,

    /// Whether the terminal needs re-rendering
    dirty: bool,
}

impl ManagedTerminal {
    /// Create a new managed terminal
    pub fn new(id: TerminalId, cols: u16, rows: u16, cell_width: u32, cell_height: u32) -> Result<Self, terminal::state::TerminalError> {
        let terminal = Terminal::new(cols, rows)?;

        Ok(Self {
            terminal,
            id,
            width: cols as u32 * cell_width,
            height: rows as u32 * cell_height,
            texture: None,
            dirty: true,
        })
    }

    /// Process PTY output and mark dirty if needed
    pub fn process(&mut self) -> Vec<SizingAction> {
        let actions = self.terminal.process_pty();
        if !actions.is_empty() {
            self.dirty = true;
        }
        actions
    }

    /// Write input to the terminal
    pub fn write(&mut self, data: &[u8]) -> Result<(), terminal::state::TerminalError> {
        self.terminal.write(data)
    }

    /// Handle resize
    pub fn resize(&mut self, rows: u16, cell_height: u32) {
        let action = self.terminal.configure(rows);
        self.height = rows as u32 * cell_height;
        self.dirty = true;

        if let SizingAction::ApplyResize { .. } = action {
            self.terminal.complete_resize();
        }
    }

    /// Get the terminal's PTY fd for polling
    pub fn pty_fd(&self) -> RawFd {
        self.terminal.pty_fd()
    }

    /// Check if terminal is still running
    pub fn is_running(&mut self) -> bool {
        self.terminal.is_running()
    }

    /// Get content row count
    pub fn content_rows(&self) -> u32 {
        self.terminal.content_rows()
    }

    /// Render terminal to texture
    pub fn render(&mut self, renderer: &mut GlesRenderer) -> Option<&GlesTexture> {
        if !self.dirty && self.texture.is_some() {
            return self.texture.as_ref();
        }

        // Render terminal to pixel buffer
        self.terminal.render(self.width, self.height);
        let buffer = self.terminal.buffer();

        if buffer.is_empty() {
            return None;
        }

        // Convert u32 ARGB to bytes
        let bytes: Vec<u8> = buffer.iter()
            .flat_map(|pixel| {
                let a = ((pixel >> 24) & 0xFF) as u8;
                let r = ((pixel >> 16) & 0xFF) as u8;
                let g = ((pixel >> 8) & 0xFF) as u8;
                let b = (pixel & 0xFF) as u8;
                [r, g, b, a]  // RGBA for OpenGL
            })
            .collect();

        // Import texture from raw pixels
        let size = Size::from((self.width as i32, self.height as i32));

        match renderer.import_memory(
            &bytes,
            smithay::backend::allocator::Fourcc::Abgr8888,
            size,
            false,
        ) {
            Ok(texture) => {
                self.texture = Some(texture);
                self.dirty = false;
                self.texture.as_ref()
            }
            Err(e) => {
                tracing::warn!("Failed to create texture: {:?}", e);
                None
            }
        }
    }

    /// Mark terminal as needing re-render
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Get cached texture (for rendering after pre-render pass)
    pub fn get_texture(&self) -> Option<&GlesTexture> {
        self.texture.as_ref()
    }
}

/// Manages all internal terminals
pub struct TerminalManager {
    /// All managed terminals
    terminals: HashMap<TerminalId, ManagedTerminal>,

    /// Next terminal ID
    next_id: u32,

    /// Cell dimensions
    pub cell_width: u32,
    pub cell_height: u32,

    /// Default terminal size in cells
    pub default_cols: u16,
    pub default_rows: u16,
}

impl TerminalManager {
    /// Create a new terminal manager
    pub fn new() -> Self {
        // Default cell dimensions (will be updated when font loads)
        Self {
            terminals: HashMap::new(),
            next_id: 0,
            cell_width: 8,
            cell_height: 16,
            default_cols: 80,
            default_rows: 24,
        }
    }

    /// Spawn a new terminal
    pub fn spawn(&mut self) -> Result<TerminalId, terminal::state::TerminalError> {
        let id = TerminalId(self.next_id);
        self.next_id += 1;

        let terminal = ManagedTerminal::new(
            id,
            self.default_cols,
            self.default_rows,
            self.cell_width,
            self.cell_height,
        )?;

        tracing::info!(id = id.0, "spawned new terminal");

        self.terminals.insert(id, terminal);
        Ok(id)
    }

    /// Get a terminal by ID
    pub fn get(&self, id: TerminalId) -> Option<&ManagedTerminal> {
        self.terminals.get(&id)
    }

    /// Get a mutable terminal by ID
    pub fn get_mut(&mut self, id: TerminalId) -> Option<&mut ManagedTerminal> {
        self.terminals.get_mut(&id)
    }

    /// Remove a terminal
    pub fn remove(&mut self, id: TerminalId) -> Option<ManagedTerminal> {
        self.terminals.remove(&id)
    }

    /// Get all terminal IDs in order
    pub fn ids(&self) -> Vec<TerminalId> {
        let mut ids: Vec<_> = self.terminals.keys().copied().collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    /// Number of terminals
    pub fn count(&self) -> usize {
        self.terminals.len()
    }

    /// Process all terminal PTY output
    pub fn process_all(&mut self) -> Vec<(TerminalId, SizingAction)> {
        let mut actions = Vec::new();
        for (id, terminal) in &mut self.terminals {
            for action in terminal.process() {
                actions.push((*id, action));
            }
        }
        actions
    }

    /// Get PTY fds for polling
    pub fn pty_fds(&self) -> Vec<(TerminalId, RawFd)> {
        self.terminals.iter()
            .map(|(id, term)| (*id, term.pty_fd()))
            .collect()
    }

    /// Remove dead terminals
    pub fn cleanup(&mut self) -> Vec<TerminalId> {
        // First collect IDs to check
        let ids: Vec<_> = self.terminals.keys().copied().collect();

        // Check each terminal and collect dead ones
        let mut dead = Vec::new();
        for id in ids {
            if let Some(term) = self.terminals.get_mut(&id) {
                if !term.is_running() {
                    dead.push(id);
                }
            }
        }

        // Remove dead terminals
        for id in &dead {
            self.terminals.remove(id);
            tracing::info!(id = id.0, "terminal exited");
        }

        dead
    }

    /// Iterate over all terminals
    pub fn iter(&self) -> impl Iterator<Item = (&TerminalId, &ManagedTerminal)> {
        self.terminals.iter()
    }

    /// Iterate mutably over all terminals
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&TerminalId, &mut ManagedTerminal)> {
        self.terminals.iter_mut()
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}
