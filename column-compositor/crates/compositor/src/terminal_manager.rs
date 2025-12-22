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

use crate::coords::RenderY;

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
        // Always mark dirty - we'll check in render if there's actually new content
        // The terminal may have received output even without sizing actions
        self.dirty = true;
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

    /// Get cell size from the terminal's font
    pub fn cell_size(&self) -> (u32, u32) {
        self.terminal.cell_size()
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

        // Convert u32 ARGB to BGRA bytes for Argb8888 format
        let bytes: Vec<u8> = buffer.iter()
            .flat_map(|pixel| {
                let a = ((pixel >> 24) & 0xFF) as u8;
                let r = ((pixel >> 16) & 0xFF) as u8;
                let g = ((pixel >> 8) & 0xFF) as u8;
                let b = (pixel & 0xFF) as u8;
                [b, g, r, a]
            })
            .collect();

        // Import texture from raw pixels
        let size = Size::from((self.width as i32, self.height as i32));

        match renderer.import_memory(
            &bytes,
            smithay::backend::allocator::Fourcc::Argb8888,
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

    /// Currently focused terminal
    pub focused: Option<TerminalId>,

    /// Cell dimensions
    pub cell_width: u32,
    pub cell_height: u32,

    /// Default terminal width in cells
    pub default_cols: u16,

    /// Initial terminal height in rows (content-aware: starts small)
    pub initial_rows: u16,

    /// Maximum terminal height in rows (capped at viewport)
    pub max_rows: u16,
}

impl TerminalManager {
    /// Create a new terminal manager with output size
    pub fn new_with_size(output_width: u32, output_height: u32) -> Self {
        // Default cell dimensions (will be updated when font loads)
        let cell_width = 8u32;
        let cell_height = 17u32;  // Match fontdue's line height calculation

        // Calculate cols to fill width
        let default_cols = (output_width / cell_width).max(1) as u16;
        // Max rows based on viewport height
        let max_rows = (output_height / cell_height).max(1) as u16;
        // Start with small height (enough for prompt + a line or two)
        let initial_rows = 3;

        Self {
            terminals: HashMap::new(),
            next_id: 0,
            focused: None,
            cell_width,
            cell_height,
            default_cols,
            initial_rows,
            max_rows,
        }
    }

    /// Create a new terminal manager with default size
    pub fn new() -> Self {
        Self::new_with_size(800, 600)
    }

    /// Get the focused terminal mutably
    pub fn get_focused_mut(&mut self) -> Option<&mut ManagedTerminal> {
        let id = self.focused?;
        self.terminals.get_mut(&id)
    }

    /// Calculate total height of all terminals
    pub fn total_height(&self) -> i32 {
        self.terminals.values().map(|t| t.height as i32).sum()
    }

    /// Update cell dimensions (called after font loads)
    pub fn set_cell_size(&mut self, width: u32, height: u32, output_width: u32, output_height: u32) {
        self.cell_width = width;
        self.cell_height = height;
        self.default_cols = (output_width / width).max(1) as u16;
        self.max_rows = (output_height / height).max(1) as u16;
    }

    /// Grow a terminal to accommodate more content (capped at max_rows)
    pub fn grow_terminal(&mut self, id: TerminalId, target_rows: u16) {
        let max_rows = self.max_rows;
        let cell_height = self.cell_height;

        if let Some(terminal) = self.terminals.get_mut(&id) {
            let new_rows = target_rows.min(max_rows);
            terminal.resize(new_rows, cell_height);
            tracing::debug!(id = id.0, target_rows, new_rows, max_rows, "grew terminal");
        }
    }

    /// Spawn a new terminal
    pub fn spawn(&mut self) -> Result<TerminalId, terminal::state::TerminalError> {
        let id = TerminalId(self.next_id);
        self.next_id += 1;

        let mut terminal = ManagedTerminal::new(
            id,
            self.default_cols,
            self.initial_rows,  // Start small, will grow with content
            self.cell_width,
            self.cell_height,
        )?;

        // Get actual cell dimensions from the font and update
        let (actual_cell_width, actual_cell_height) = terminal.cell_size();
        if actual_cell_width != self.cell_width || actual_cell_height != self.cell_height {
            self.cell_width = actual_cell_width;
            self.cell_height = actual_cell_height;
            // Recalculate max_rows with correct cell size
            // (initial_rows stays the same)
            terminal.width = self.default_cols as u32 * actual_cell_width;
            terminal.height = self.initial_rows as u32 * actual_cell_height;
        }

        tracing::info!(id = id.0, cols = self.default_cols, rows = self.initial_rows,
                       max_rows = self.max_rows,
                       cell_w = self.cell_width, cell_h = self.cell_height,
                       "spawned new terminal");

        self.terminals.insert(id, terminal);

        // Focus the new terminal
        self.focused = Some(id);

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

    /// Focus the next terminal (by ID order)
    pub fn focus_next(&mut self) -> bool {
        let ids = self.ids();
        if ids.is_empty() {
            return false;
        }

        let current_idx = self.focused
            .and_then(|f| ids.iter().position(|id| *id == f))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % ids.len();
        self.focused = Some(ids[next_idx]);
        tracing::info!(focused = ?self.focused, "focused next terminal");
        true
    }

    /// Focus the previous terminal (by ID order)
    pub fn focus_prev(&mut self) -> bool {
        let ids = self.ids();
        if ids.is_empty() {
            return false;
        }

        let current_idx = self.focused
            .and_then(|f| ids.iter().position(|id| *id == f))
            .unwrap_or(0);

        let prev_idx = if current_idx == 0 { ids.len() - 1 } else { current_idx - 1 };
        self.focused = Some(ids[prev_idx]);
        tracing::info!(focused = ?self.focused, "focused prev terminal");
        true
    }

    /// Get the Y position of a terminal (for scrolling to it)
    pub fn terminal_y_position(&self, target_id: TerminalId) -> Option<i32> {
        let mut y = 0i32;
        for id in self.ids() {
            if id == target_id {
                return Some(y);
            }
            if let Some(term) = self.terminals.get(&id) {
                y += term.height as i32;
            }
        }
        None
    }

    /// Get the Y position and height of the focused terminal
    pub fn focused_position(&self) -> Option<(i32, i32)> {
        let focused_id = self.focused?;
        let y = self.terminal_y_position(focused_id)?;
        let height = self.terminals.get(&focused_id)?.height as i32;
        Some((y, height))
    }

    /// Find which terminal is at a given render Y position
    ///
    /// Takes a render Y coordinate (Y=0 at bottom, from pointer location)
    /// and converts it to content coordinates to find which terminal is there.
    pub fn terminal_at_y(&self, render_y: RenderY, scroll_offset: f64) -> Option<TerminalId> {
        // Convert render Y to content Y (accounting for scroll)
        // content_y = render_y + scroll_offset
        let content_y = render_y.to_content(scroll_offset);

        let mut y = 0.0;
        for id in self.ids() {
            if let Some(term) = self.terminals.get(&id) {
                let height = term.height as f64;
                if content_y.value() >= y && content_y.value() < y + height {
                    return Some(id);
                }
                y += height;
            }
        }
        None
    }

    /// Focus a specific terminal
    pub fn focus(&mut self, id: TerminalId) {
        if self.terminals.contains_key(&id) {
            self.focused = Some(id);
            tracing::info!(?id, "focused terminal");
        }
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}
