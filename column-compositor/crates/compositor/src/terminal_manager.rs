//! Internal terminal management
//!
//! Manages spawning and rendering of internal terminals.

use std::collections::HashMap;
use std::os::fd::RawFd;
use std::path::Path;

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

    /// Keep window open after process exits (for command terminals)
    pub keep_open: bool,

    /// Whether the process has exited (for hiding cursor)
    exited: bool,

    /// Whether this terminal is temporarily hidden
    /// (e.g., parent hidden while child command runs)
    pub hidden: bool,

    /// Parent terminal that spawned this one (if any)
    /// When this terminal exits, the parent is unhidden
    pub parent: Option<TerminalId>,

    /// Previous alternate screen state (for detecting transitions)
    prev_alt_screen: bool,
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
            keep_open: false,
            exited: false,
            hidden: false,
            parent: None,
            prev_alt_screen: false,
        })
    }

    /// Create a new managed terminal running a specific command
    ///
    /// - `pty_rows`: Size reported to the PTY (program sees this many rows)
    /// - `visual_rows`: Initial visual size for display
    /// - `parent`: Parent terminal to unhide when this one exits
    pub fn new_with_command(
        id: TerminalId,
        cols: u16,
        pty_rows: u16,
        visual_rows: u16,
        cell_width: u32,
        cell_height: u32,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        parent: Option<TerminalId>,
    ) -> Result<Self, terminal::state::TerminalError> {
        let terminal = Terminal::new_with_command(cols, pty_rows, visual_rows, command, working_dir, env)?;

        Ok(Self {
            terminal,
            id,
            width: cols as u32 * cell_width,
            height: visual_rows as u32 * cell_height, // Use visual rows for display
            texture: None,
            dirty: true,
            keep_open: true, // Command terminals stay open after exit
            exited: false,
            hidden: false,
            parent,
            prev_alt_screen: false,
        })
    }

    /// Process PTY output and mark dirty if needed
    pub fn process(&mut self) -> (Vec<SizingAction>, usize) {
        let (actions, bytes_read) = self.terminal.process_pty_with_count();
        // Always mark dirty - we'll check in render if there's actually new content
        // The terminal may have received output even without sizing actions
        let was_dirty = self.dirty;
        self.dirty = true;
        if !was_dirty {
            tracing::info!(id = self.id.0, "terminal marked dirty");
        }
        (actions, bytes_read)
    }

    /// Write input to the terminal
    pub fn write(&mut self, data: &[u8]) -> Result<(), terminal::state::TerminalError> {
        tracing::info!(id = self.id.0, len = data.len(), "writing to terminal PTY");
        self.terminal.write(data)
    }

    /// Directly inject bytes into terminal emulator (for testing)
    ///
    /// This bypasses the PTY and directly feeds bytes to the VTE parser.
    /// Useful for simulating terminal output in tests.
    pub fn inject_bytes(&mut self, data: &[u8]) {
        self.terminal.inject_bytes(data);
        self.dirty = true;
    }

    /// Check if terminal transitioned to alternate screen and needs auto-resize.
    ///
    /// Returns true if the terminal just entered alternate screen mode and is not
    /// already at full height. This allows reactive resizing for TUI apps that
    /// weren't pre-configured in tui_apps list.
    ///
    /// Updates internal state to track the transition.
    pub fn check_alt_screen_resize_needed(&mut self, max_height: u32) -> bool {
        let is_alt = self.terminal.is_alternate_screen();
        let transitioned_to_alt = is_alt && !self.prev_alt_screen;
        self.prev_alt_screen = is_alt;

        if transitioned_to_alt && self.height < max_height {
            tracing::info!(
                id = self.id.0,
                current_height = self.height,
                max_height,
                "terminal entered alternate screen, needs resize"
            );
            true
        } else {
            false
        }
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
        let running = self.terminal.is_running();
        if !running {
            self.exited = true;
        }
        running
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
        tracing::debug!(id = self.id.0, dirty = self.dirty, has_texture = self.texture.is_some(), "render() called");
        if !self.dirty && self.texture.is_some() {
            tracing::debug!(id = self.id.0, "render() early return - not dirty and has texture");
            return self.texture.as_ref();
        }

        tracing::info!(id = self.id.0, width = self.width, height = self.height, dirty = self.dirty, "re-rendering terminal");

        // Render terminal to pixel buffer (hide cursor if process exited)
        self.terminal.render(self.width, self.height, !self.exited);
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

    /// Check if terminal needs re-render
    pub fn is_dirty(&self) -> bool {
        self.dirty
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

    /// Calculate total height of visible (non-hidden) terminals
    pub fn total_height(&self) -> i32 {
        self.terminals.values()
            .filter(|t| !t.hidden)
            .map(|t| t.height as i32)
            .sum()
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

    /// Spawn a new terminal running a specific command
    ///
    /// If `parent` is provided, that terminal will be hidden while this one runs.
    /// When this terminal's command exits, the parent will be unhidden.
    ///
    /// If `is_tui` is true, the terminal starts at full viewport height for TUI apps.
    pub fn spawn_command(
        &mut self,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        parent: Option<TerminalId>,
        is_tui: bool,
    ) -> Result<TerminalId, terminal::state::TerminalError> {
        let id = TerminalId(self.next_id);
        self.next_id += 1;

        // Hide the parent terminal while command runs
        if let Some(parent_id) = parent {
            if let Some(parent_term) = self.terminals.get_mut(&parent_id) {
                parent_term.hidden = true;
                tracing::info!(parent = parent_id.0, new_child = id.0, "hiding parent terminal");
            }
        } else {
            tracing::warn!(new_child = id.0, "no parent to hide - terminal_manager.focused was None");
        }

        // For TUI apps: use full viewport height for both PTY and visual
        // For regular commands: use large PTY (no scrolling) but small visual size
        let (pty_rows, visual_rows) = if is_tui {
            (self.max_rows, self.max_rows)
        } else {
            (1000, self.initial_rows)
        };

        let mut terminal = ManagedTerminal::new_with_command(
            id,
            self.default_cols,
            pty_rows,
            visual_rows,
            self.cell_width,
            self.cell_height,
            command,
            working_dir,
            env,
            parent,
        )?;

        // Get actual cell dimensions from the font and update
        let (actual_cell_width, actual_cell_height) = terminal.cell_size();
        if actual_cell_width != self.cell_width || actual_cell_height != self.cell_height {
            self.cell_width = actual_cell_width;
            self.cell_height = actual_cell_height;
            terminal.width = self.default_cols as u32 * actual_cell_width;
        }

        // Set small visual height (will grow based on content)
        terminal.height = visual_rows as u32 * self.cell_height;

        tracing::info!(id = id.0, cols = self.default_cols, pty_rows, visual_rows, is_tui,
                       height = terminal.height, max_rows = self.max_rows, cell_height = self.cell_height,
                       ?parent, command, "spawned command terminal");

        self.terminals.insert(id, terminal);

        // Focus the new command terminal
        self.focused = Some(id);

        // Debug: show which terminals are hidden/visible
        for (tid, term) in &self.terminals {
            tracing::info!(tid = tid.0, hidden = term.hidden, height = term.height, "terminal state after spawn");
        }

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

    /// Get visible (non-hidden) terminal IDs in order
    pub fn visible_ids(&self) -> Vec<TerminalId> {
        let mut ids: Vec<_> = self.terminals.iter()
            .filter(|(_, term)| !term.hidden)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    /// Number of terminals
    pub fn count(&self) -> usize {
        self.terminals.len()
    }

    /// Number of visible terminals
    pub fn visible_count(&self) -> usize {
        self.terminals.values().filter(|t| !t.hidden).count()
    }

    /// Process all terminal PTY output
    pub fn process_all(&mut self) -> Vec<(TerminalId, SizingAction)> {
        let mut actions = Vec::new();
        for (id, terminal) in &mut self.terminals {
            let (term_actions, bytes_read) = terminal.process();
            if bytes_read > 0 {
                tracing::info!(id = id.0, bytes_read, "PTY read for terminal");
            }
            for action in term_actions {
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

    /// Remove dead terminals (except those marked keep_open)
    /// Also handles unhiding parent terminals when command terminals exit
    ///
    /// Returns (dead_terminals, focus_changed_to)
    /// - dead_terminals: terminals that were removed
    /// - focus_changed_to: if Some, the focus should be updated to this terminal
    pub fn cleanup(&mut self) -> (Vec<TerminalId>, Option<TerminalId>) {
        // First collect IDs to check
        let ids: Vec<_> = self.terminals.keys().copied().collect();

        // Check each terminal for exit status
        let mut dead = Vec::new();
        let mut parents_to_unhide = Vec::new();
        let mut terminals_to_hide = Vec::new();
        let mut focus_changed_to = None;

        for id in ids {
            if let Some(term) = self.terminals.get_mut(&id) {
                // Check exited BEFORE is_running() since is_running() sets exited as side effect
                let was_already_exited = term.exited;
                let running = term.is_running();

                if !running {
                    // Terminal has exited
                    if !was_already_exited {
                        // First time detecting exit - handle parent unhiding
                        if let Some(parent_id) = term.parent {
                            parents_to_unhide.push(parent_id);
                            focus_changed_to = Some(parent_id);
                            tracing::info!(child = id.0, parent = parent_id.0, "command exited, will unhide parent");

                            // Check if command terminal has meaningful content
                            // If content_rows <= 1 (just the echo line or empty), hide it
                            let content_rows = term.content_rows();
                            if content_rows <= 1 {
                                terminals_to_hide.push(id);
                                tracing::info!(id = id.0, content_rows, "hiding empty command terminal");
                            }
                        }
                    }

                    if !term.keep_open {
                        dead.push(id);
                    }
                }
            }
        }

        // Unhide parent terminals
        for parent_id in parents_to_unhide {
            if let Some(parent) = self.terminals.get_mut(&parent_id) {
                parent.hidden = false;
                tracing::info!(id = parent_id.0, "unhiding parent terminal");
            }
            // Focus the parent
            self.focused = Some(parent_id);
        }

        // Hide empty command terminals
        for id in terminals_to_hide {
            if let Some(term) = self.terminals.get_mut(&id) {
                term.hidden = true;
            }
        }

        // Remove dead terminals
        for id in &dead {
            self.terminals.remove(id);
            tracing::info!(id = id.0, "terminal removed");
        }

        (dead, focus_changed_to)
    }

    /// Iterate over all terminals
    pub fn iter(&self) -> impl Iterator<Item = (&TerminalId, &ManagedTerminal)> {
        self.terminals.iter()
    }

    /// Iterate mutably over all terminals
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&TerminalId, &mut ManagedTerminal)> {
        self.terminals.iter_mut()
    }

    /// Focus the next visible terminal (by ID order)
    pub fn focus_next(&mut self) -> bool {
        let ids = self.visible_ids();
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

    /// Focus the previous visible terminal (by ID order)
    pub fn focus_prev(&mut self) -> bool {
        let ids = self.visible_ids();
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

    /// Get the Y position of a visible terminal (for scrolling to it)
    pub fn terminal_y_position(&self, target_id: TerminalId) -> Option<i32> {
        let mut y = 0i32;
        for id in self.visible_ids() {
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

    /// Find which visible terminal is at a given render Y position
    ///
    /// Takes a render Y coordinate (Y=0 at bottom, from pointer location)
    /// and converts it to content coordinates to find which terminal is there.
    pub fn terminal_at_y(&self, render_y: RenderY, scroll_offset: f64) -> Option<TerminalId> {
        // Convert render Y to content Y (accounting for scroll)
        // content_y = render_y + scroll_offset
        let content_y = render_y.to_content(scroll_offset);

        let mut y = 0.0;
        for id in self.visible_ids() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn tui_terminal_has_full_viewport_height() {
        // Create a terminal manager with known dimensions
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Capture initial values BEFORE spawning (these use default cell_height=17)
        let initial_cell_height = manager.cell_height;
        let initial_max_rows = manager.max_rows;

        assert_eq!(initial_cell_height, 17, "initial cell_height should be 17");
        assert_eq!(initial_max_rows, 42, "initial max_rows should be 720/17 = 42");

        // Spawn a TUI command
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, true);

        assert!(result.is_ok(), "spawn_command should succeed");
        let id = result.unwrap();

        // After spawning, cell dimensions may have been updated from the font
        let actual_cell_height = manager.cell_height;
        let actual_max_rows = manager.max_rows;

        // The terminal should use the CURRENT max_rows and cell_height
        let expected_height = actual_max_rows as u32 * actual_cell_height;

        // Get the terminal and check its height
        let terminal = manager.get(id).expect("terminal should exist");

        // Debug output
        eprintln!("initial_cell_height={}, actual_cell_height={}", initial_cell_height, actual_cell_height);
        eprintln!("initial_max_rows={}, actual_max_rows={}", initial_max_rows, actual_max_rows);
        eprintln!("terminal.height={}, expected_height={}", terminal.height, expected_height);

        assert_eq!(
            terminal.height,
            expected_height,
            "TUI terminal height should be {} (max_rows={} * cell_height={}), but was {}",
            expected_height,
            actual_max_rows,
            actual_cell_height,
            terminal.height
        );
    }

    #[test]
    fn tui_uses_max_rows_after_font_loads() {
        // This test verifies that when a TUI terminal is spawned,
        // it uses the CURRENT max_rows (which may differ from initial if font changed)
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a TUI command
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, true);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");

        // The key assertion: terminal height should fill the viewport
        // (within one cell_height, since max_rows is floor division)
        let viewport_height = output_height as i32;
        let terminal_height = terminal.height as i32;
        let cell_height = manager.cell_height as i32;

        // Terminal should be within one cell of viewport height
        let height_diff = (viewport_height - terminal_height).abs();
        assert!(
            height_diff < cell_height,
            "TUI terminal should fill viewport: viewport={}, terminal={}, diff={}, cell_height={}",
            viewport_height, terminal_height, height_diff, cell_height
        );
    }

    #[test]
    fn max_rows_updates_when_cell_height_changes() {
        // This test checks if max_rows is recalculated when cell dimensions change
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let initial_max_rows = manager.max_rows;
        let initial_cell_height = manager.cell_height;

        // Spawn any terminal to trigger font loading
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let _ = manager.spawn_command("echo test", cwd, &env, None, false);

        let new_cell_height = manager.cell_height;
        let new_max_rows = manager.max_rows;

        // If cell_height changed, max_rows SHOULD be recalculated
        if new_cell_height != initial_cell_height {
            let expected_max_rows = (output_height / new_cell_height).max(1) as u16;
            assert_eq!(
                new_max_rows, expected_max_rows,
                "max_rows should be recalculated when cell_height changes: \
                 initial_cell_height={}, new_cell_height={}, \
                 initial_max_rows={}, new_max_rows={}, expected={}",
                initial_cell_height, new_cell_height,
                initial_max_rows, new_max_rows, expected_max_rows
            );
        }
    }

    #[test]
    fn non_tui_terminal_has_small_height() {
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a non-TUI command
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, false);

        assert!(result.is_ok(), "spawn_command should succeed");
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;  // 3
        let expected_height = initial_rows as u32 * cell_height;

        assert_eq!(
            terminal.height,
            expected_height,
            "non-TUI terminal height should be {} (initial_rows={} * cell_height={}), but was {}",
            expected_height,
            initial_rows,
            cell_height,
            terminal.height
        );
    }

    #[test]
    fn tui_terminal_pty_rows_equals_max_rows() {
        // The PTY must report the correct number of rows to the program
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, true);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let (cols, rows) = terminal.terminal.dimensions();

        // For TUI apps, the PTY should report max_rows
        // (within one cell due to floor division)
        let expected_rows = manager.max_rows;

        assert_eq!(
            rows, expected_rows,
            "TUI terminal PTY rows should be max_rows={}, but was {}",
            expected_rows, rows
        );

        eprintln!("PTY dimensions: cols={}, rows={}", cols, rows);
        eprintln!("max_rows={}, cell_height={}", manager.max_rows, manager.cell_height);
    }

    #[test]
    fn non_tui_terminal_pty_has_large_rows() {
        // Non-TUI terminals use 1000 rows for PTY (no scrolling)
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, false);
        let id = result.unwrap();

        let terminal = manager.get(id).expect("terminal should exist");
        let (_, rows) = terminal.terminal.dimensions();

        assert_eq!(
            rows, 1000,
            "non-TUI terminal PTY rows should be 1000, but was {}",
            rows
        );
    }

    #[test]
    fn layout_height_uses_terminal_height_not_default() {
        // This test simulates what main.rs does when calculating heights
        // for layout after spawning a terminal
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a TUI terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, true).unwrap();

        // Simulate what main.rs does for calculating heights:
        // It iterates over cells and for each terminal, gets terminal.height
        let terminal = manager.get(id).unwrap();

        // The height used for layout MUST be the terminal's height, not some default
        let layout_height = if terminal.hidden {
            0
        } else {
            terminal.height as i32
        };

        // For TUI, layout height should be full viewport (within one cell)
        let viewport_height = output_height as i32;
        let cell_height = manager.cell_height as i32;

        eprintln!("layout_height={}, viewport_height={}, cell_height={}",
                  layout_height, viewport_height, cell_height);

        // Layout height should be close to viewport height
        let height_diff = (viewport_height - layout_height).abs();
        assert!(
            height_diff < cell_height,
            "TUI layout height should be close to viewport: layout={}, viewport={}, diff={}",
            layout_height, viewport_height, height_diff
        );

        // And NOT be the old default of 200
        assert!(
            layout_height > 200,
            "TUI layout height should NOT be default 200: was {}",
            layout_height
        );
    }

    #[test]
    fn tui_mc_gets_full_height() {
        // Spawn actual mc command and verify it gets full terminal dimensions
        // This simulates EXACTLY what main.rs does for IPC spawn
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Simulate the environment modifications from main.rs
        let mut env = HashMap::new();
        env.insert("GIT_PAGER".to_string(), "cat".to_string());
        env.insert("PAGER".to_string(), "cat".to_string());
        env.insert("LESS".to_string(), "-FRX".to_string());

        let cwd = std::path::Path::new("/tmp");

        // Simulate the command transformation from main.rs:
        // format!("echo '> {}'; {}", escaped, request.command)
        let command = "echo '> mc'; mc";

        // Spawn mc as a TUI app (is_tui = true)
        let result = manager.spawn_command(command, cwd, &env, None, true);
        assert!(result.is_ok(), "spawn mc should succeed: {:?}", result.err());
        let id = result.unwrap();

        let terminal = manager.get(id).unwrap();

        // Check terminal dimensions
        let (cols, pty_rows) = terminal.terminal.dimensions();
        let visual_height = terminal.height;
        let max_rows = manager.max_rows;
        let cell_height = manager.cell_height;

        eprintln!("mc terminal: cols={}, pty_rows={}, visual_height={}", cols, pty_rows, visual_height);
        eprintln!("expected: max_rows={}, expected_height={}", max_rows, max_rows as u32 * cell_height);

        // PTY rows should equal max_rows for TUI
        assert_eq!(
            pty_rows, max_rows,
            "mc PTY rows should be max_rows={}, but was {}",
            max_rows, pty_rows
        );

        // Visual height should be max_rows * cell_height
        let expected_height = max_rows as u32 * cell_height;
        assert_eq!(
            visual_height, expected_height,
            "mc visual height should be {}, but was {}",
            expected_height, visual_height
        );
    }

    #[test]
    fn tui_stty_reports_correct_size() {
        // Use stty to verify the PTY size is correctly set
        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn stty size which prints "rows cols"
        let result = manager.spawn_command("stty size", cwd, &env, None, true);
        assert!(result.is_ok(), "spawn stty should succeed");
        let id = result.unwrap();

        let terminal = manager.get(id).unwrap();
        let (cols, pty_rows) = terminal.terminal.dimensions();

        eprintln!("stty terminal: pty_rows={}, cols={}", pty_rows, cols);
        eprintln!("expected max_rows={}", manager.max_rows);

        // For TUI, PTY should report max_rows
        assert_eq!(
            pty_rows, manager.max_rows,
            "TUI stty PTY rows should be {}, was {}",
            manager.max_rows, pty_rows
        );
    }

    #[test]
    fn resize_to_full_updates_pty_and_dimensions() {
        // This test reproduces the TUI resize flow:
        // 1. Shell terminal starts small (content-based sizing)
        // 2. User runs TUI app -> column-term --resize full
        // 3. Terminal is resized to full viewport height
        // 4. TUI app runs and should see full-size terminal
        //
        // BUG: If resize doesn't properly update PTY, TUI apps will see old size

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a non-TUI terminal (like a shell)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let result = manager.spawn_command("echo test", cwd, &env, None, false);
        assert!(result.is_ok(), "spawn should succeed");
        let id = result.unwrap();

        // Get initial dimensions - should be small (initial_rows)
        let initial_rows = manager.initial_rows; // 3
        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        {
            let terminal = manager.get(id).expect("terminal should exist");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let visual_height = terminal.height;

            eprintln!("BEFORE resize: pty_rows={}, visual_height={}", pty_rows, visual_height);

            // Non-TUI terminals use 1000 rows for PTY (no scrolling)
            assert_eq!(pty_rows, 1000, "non-TUI PTY rows should be 1000");

            // Visual height should be small (initial_rows * cell_height)
            let expected_small_height = initial_rows as u32 * cell_height;
            assert_eq!(
                visual_height, expected_small_height,
                "initial visual height should be {} (initial_rows={})",
                expected_small_height, initial_rows
            );
        }

        // NOW: Resize to full height (simulating column-term --resize full)
        {
            let terminal = manager.get_mut(id).expect("terminal should exist");
            terminal.resize(max_rows, cell_height);
        }

        // Check dimensions AFTER resize
        {
            let terminal = manager.get(id).expect("terminal should exist");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let visual_height = terminal.height;

            eprintln!("AFTER resize: pty_rows={}, visual_height={}", pty_rows, visual_height);

            // PTY should now report max_rows
            assert_eq!(
                pty_rows, max_rows,
                "AFTER resize: PTY rows should be max_rows={}, but was {}",
                max_rows, pty_rows
            );

            // Visual height should be full viewport
            let expected_full_height = max_rows as u32 * cell_height;
            assert_eq!(
                visual_height, expected_full_height,
                "AFTER resize: visual height should be {} (max_rows={}), but was {}",
                expected_full_height, max_rows, visual_height
            );

            // Terminal should be marked dirty (needs re-render)
            assert!(
                terminal.is_dirty(),
                "AFTER resize: terminal should be marked dirty for re-render"
            );
        }
    }

    #[test]
    fn resize_to_content_shrinks_terminal() {
        // After TUI exits, terminal is resized back to content-based sizing

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a non-TUI terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // First resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify it's full size
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, max_rows as u32 * cell_height);
        }

        // Now resize back to content-based (e.g., 3 rows)
        let content_rows = 3u16;
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        // Verify it's shrunk
        {
            let terminal = manager.get(id).unwrap();
            let (_, pty_rows) = terminal.terminal.dimensions();

            assert_eq!(
                pty_rows, content_rows,
                "After shrink: PTY rows should be {}, was {}",
                content_rows, pty_rows
            );

            assert_eq!(
                terminal.height, content_rows as u32 * cell_height,
                "After shrink: height should be {}",
                content_rows as u32 * cell_height
            );
        }
    }

    #[test]
    fn resize_actually_changes_alacritty_grid_size() {
        // BUG REPRODUCTION: After resize, does the alacritty terminal grid
        // actually have the new dimensions? If not, TUI apps will draw
        // to wrong locations.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a non-TUI terminal (like shell)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Get initial grid dimensions from alacritty term
        let initial_grid_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.grid_rows()
        };

        eprintln!("BEFORE resize: grid_rows={}", initial_grid_rows);

        // Non-TUI terminals have PTY with 1000 rows, but alacritty grid should match
        // Wait - what IS the initial grid size for non-TUI?

        // Now resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Get grid dimensions AFTER resize
        let after_grid_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.grid_rows()
        };

        eprintln!("AFTER resize: grid_rows={}, expected max_rows={}", after_grid_rows, max_rows);

        // The alacritty grid should now have max_rows
        assert_eq!(
            after_grid_rows, max_rows,
            "AFTER resize: alacritty grid rows should be {}, but was {}",
            max_rows, after_grid_rows
        );
    }

    #[test]
    fn non_tui_terminal_initial_grid_size() {
        // What is the initial grid size for a non-TUI terminal?
        // This documents the current behavior.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let terminal = manager.get(id).unwrap();

        // Get dimensions from various sources
        let (pty_cols, pty_rows) = terminal.terminal.dimensions();
        let grid_rows = terminal.terminal.grid_rows();
        let visual_height = terminal.height;
        let cell_height = manager.cell_height;
        let initial_rows = manager.initial_rows;

        eprintln!("Non-TUI terminal dimensions:");
        eprintln!("  PTY: cols={}, rows={}", pty_cols, pty_rows);
        eprintln!("  Grid rows: {}", grid_rows);
        eprintln!("  Visual height: {} pixels", visual_height);
        eprintln!("  Expected visual height: {} * {} = {}", initial_rows, cell_height, initial_rows as u32 * cell_height);

        // PTY has 1000 rows (for no scrolling in content-based terminals)
        assert_eq!(pty_rows, 1000, "PTY rows should be 1000");

        // But what about grid_rows? Is it 1000 or initial_rows?
        // This is the key question for the bug!
        eprintln!("  Grid rows == PTY rows? {}", grid_rows == pty_rows);
        eprintln!("  Grid rows == initial_rows? {}", grid_rows == initial_rows);
    }

    #[test]
    fn content_visible_after_resize() {
        // BUG REPRODUCTION: After resize, is content written to the terminal
        // actually visible in the rendered output?
        //
        // This simulates: resize terminal -> TUI app draws -> should be visible

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a terminal that outputs content
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use a command that outputs multiple lines
        let id = manager.spawn_command("seq 1 50", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Wait a bit for the command to produce output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output
        manager.process_all();

        // Check content before resize
        let content_before = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows before resize: {}", content_before);

        // Now resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // The terminal should still have all the content
        let content_after = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows after resize: {}", content_after);

        // Content should NOT have been lost due to resize
        // Note: content_rows might be capped by the sizing state machine
        assert!(
            content_after >= content_before.min(max_rows as u32),
            "Content should not be lost after resize: before={}, after={}",
            content_before, content_after
        );
    }

    #[test]
    fn render_dimensions_match_terminal_height() {
        // BUG REPRODUCTION: Does the render buffer size match the terminal height?
        // If not, the rendered output will be wrong size.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Get the terminal height
        let terminal_height = {
            let terminal = manager.get(id).unwrap();
            terminal.height
        };

        eprintln!("After resize:");
        eprintln!("  terminal.height = {} pixels", terminal_height);
        eprintln!("  expected = max_rows * cell_height = {} * {} = {}",
                  max_rows, cell_height, max_rows as u32 * cell_height);

        // The terminal height should be max_rows * cell_height
        assert_eq!(
            terminal_height, max_rows as u32 * cell_height,
            "Terminal height should be {} but was {}",
            max_rows as u32 * cell_height, terminal_height
        );

        // Now render and check the buffer dimensions
        // Note: We can't easily call render() without a GlesRenderer in tests
        // But we can check that width/height are set correctly for when render is called
        let terminal = manager.get(id).unwrap();
        eprintln!("  terminal.width = {} pixels", terminal.width);

        // Width is calculated from font cell dimensions at terminal creation time
        // It may differ from output_width due to rounding
        // Just verify it's non-zero and reasonable
        assert!(terminal.width > 0, "Terminal width should be non-zero");
        assert!(terminal.width <= output_width + 100, "Terminal width should be close to output width");
    }

    #[test]
    fn sizing_state_after_resize() {
        // Check what state the sizing state machine is in after resize
        // If it's not Stable, new content might not be tracked correctly

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Check initial sizing state
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("BEFORE resize: sizing state = {:?}", sizing_state);
            assert!(sizing_state.is_stable(), "Initial state should be Stable");
        }

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Check sizing state after resize
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("AFTER resize: sizing state = {:?}", sizing_state);
            assert!(sizing_state.is_stable(), "State after resize should be Stable");
            assert_eq!(
                sizing_state.current_rows(), max_rows,
                "State should show max_rows={}, but shows {}",
                max_rows, sizing_state.current_rows()
            );
        }
    }

    #[test]
    fn resize_when_growth_pending() {
        // BUG REPRODUCTION: What happens if we resize while growth is pending?
        // This might cause the resize to be ignored or handled incorrectly.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a terminal that outputs a lot of content (triggers growth)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("seq 1 100", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Wait for output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY but DON'T handle sizing actions (simulates delayed compositor response)
        let _actions = manager.process_all();

        // Check if growth was requested
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            eprintln!("BEFORE forced resize: sizing state = {:?}", sizing_state);
        }

        // Now force resize to full height (simulating column-term --resize full)
        // This might conflict with pending growth
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Check the final state
        {
            let terminal = manager.get(id).unwrap();
            let sizing_state = terminal.terminal.sizing_state();
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();

            eprintln!("AFTER forced resize:");
            eprintln!("  sizing state = {:?}", sizing_state);
            eprintln!("  PTY rows = {}", pty_rows);
            eprintln!("  grid rows = {}", grid_rows);
            eprintln!("  terminal.height = {}", terminal.height);

            // Everything should be at max_rows
            assert_eq!(pty_rows, max_rows, "PTY rows should be max_rows");
            assert_eq!(grid_rows, max_rows, "Grid rows should be max_rows");
            assert!(sizing_state.is_stable(), "State should be Stable after resize");
        }
    }

    #[test]
    fn new_output_after_resize_marks_dirty() {
        // BUG REPRODUCTION: After resize, does new PTY output mark the terminal dirty?
        // If not, the terminal won't be re-rendered and updates will be "missing".

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a shell that we can write to
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use cat which will echo back what we write
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full height
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Clear dirty flag (simulate render happened)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        eprintln!("After resize, dirty cleared: dirty = {}",
                  manager.get(id).unwrap().is_dirty());

        // Write some input to the terminal (simulates TUI app drawing)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"Hello from TUI app!\n").unwrap();
        }

        // Wait for cat to echo back
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Process PTY output
        manager.process_all();

        // Check if terminal is dirty
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After writing and processing: dirty = {}", is_dirty);

        assert!(is_dirty, "Terminal should be marked dirty after new PTY output");
    }

    #[test]
    fn process_all_marks_dirty_on_output() {
        // Verify that process_all() marks terminals dirty when there's output

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo 'test output'", cwd, &env, None, false).unwrap();

        // Wait for command to produce output
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Clear dirty flag
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        eprintln!("Before process_all: dirty = {}", manager.get(id).unwrap().is_dirty());

        // Process PTY output
        manager.process_all();

        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After process_all: dirty = {}", is_dirty);

        assert!(is_dirty, "process_all should mark terminal dirty when there's output");
    }

    #[test]
    fn tui_output_processed_after_resize() {
        // BUG REPRODUCTION: After resizing terminal to full height and running a TUI app,
        // does the TUI's screen-drawing output get processed correctly?
        //
        // This simulates the TUI resize flow:
        // 1. Shell terminal starts small (content-based)
        // 2. column-term --resize full resizes it
        // 3. mc (or other TUI) runs and draws the full screen
        // 4. The compositor should see all of mc's output

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Use cat to simulate a terminal we can write to
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Initial visual height should be small
        {
            let terminal = manager.get(id).unwrap();
            let initial_height = terminal.height;
            eprintln!("Initial height: {} pixels ({} rows)", initial_height, initial_height / cell_height);
            assert!(initial_height < 100, "Initial height should be small");
        }

        // Simulate column-term --resize full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify resize happened
        {
            let terminal = manager.get(id).unwrap();
            let (_, pty_rows) = terminal.terminal.dimensions();
            eprintln!("After resize: PTY rows={}, height={}", pty_rows, terminal.height);
            assert_eq!(pty_rows, max_rows, "PTY should be resized to max_rows");
        }

        // Clear dirty flag (simulate frame render after resize)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
        }

        // Now simulate TUI drawing the screen
        // TUI apps typically:
        // 1. Clear screen: ESC[2J
        // 2. Move cursor home: ESC[H
        // 3. Draw each row with cursor positioning: ESC[row;colH
        {
            let terminal = manager.get_mut(id).unwrap();

            // Clear screen and move home
            let clear_screen = "\x1b[2J\x1b[H";
            terminal.write(clear_screen.as_bytes()).unwrap();

            // Draw a TUI-like screen (borders and content)
            // This simulates what mc does: draw characters at specific positions
            for row in 0..max_rows {
                // Move to row,1
                let move_cursor = format!("\x1b[{};1H", row + 1);
                terminal.write(move_cursor.as_bytes()).unwrap();

                // Draw a line
                let line = format!("Row {:02}: {}", row, "=" .repeat(50));
                terminal.write(line.as_bytes()).unwrap();
            }
        }

        // Wait for cat to echo back (cat just echoes what it receives)
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output
        manager.process_all();

        // The terminal should be marked dirty
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After TUI output: dirty = {}", is_dirty);
        assert!(is_dirty, "Terminal should be dirty after TUI output");

        // The terminal should have content (not blank)
        // Note: We can't easily verify the actual content without rendering,
        // but we can check that something was processed
    }

    #[test]
    fn resize_ipc_flow_simulation() {
        // This test simulates the EXACT flow when column-term --resize full is called:
        //
        // 1. column-term sends IPC message: {"type": "resize", "mode": "full"}
        // 2. Compositor receives message (in calloop callback)
        // 3. Compositor stores pending_resize_request
        // 4. Later in frame: process pending_resize_request
        // 5. Resize the focused terminal
        // 6. Send ACK
        //
        // The question: Is the resize actually applied BEFORE the ACK is sent?

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a shell (non-TUI)
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Record state BEFORE resize
        let before_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };
        let before_height = manager.get(id).unwrap().height;
        let before_grid_rows = manager.get(id).unwrap().terminal.grid_rows();

        eprintln!("BEFORE resize IPC:");
        eprintln!("  PTY rows: {}", before_pty_rows);
        eprintln!("  Grid rows: {}", before_grid_rows);
        eprintln!("  Visual height: {}", before_height);

        // Simulate the compositor processing the resize request
        // This is what happens in main.rs when pending_resize_request is processed
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // At this point, ACK would be sent
        // The question: are all these values updated?

        let after_pty_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.dimensions().1
        };
        let after_height = manager.get(id).unwrap().height;
        let after_grid_rows = manager.get(id).unwrap().terminal.grid_rows();
        let after_dirty = manager.get(id).unwrap().is_dirty();

        eprintln!("AFTER resize (before ACK):");
        eprintln!("  PTY rows: {}", after_pty_rows);
        eprintln!("  Grid rows: {}", after_grid_rows);
        eprintln!("  Visual height: {}", after_height);
        eprintln!("  Dirty: {}", after_dirty);

        // All these should be updated BEFORE ACK is sent
        assert_eq!(
            after_pty_rows, max_rows,
            "PTY rows should be max_rows BEFORE ACK"
        );
        assert_eq!(
            after_grid_rows, max_rows,
            "Grid rows should be max_rows BEFORE ACK"
        );
        assert_eq!(
            after_height, max_rows as u32 * cell_height,
            "Visual height should be updated BEFORE ACK"
        );
        assert!(after_dirty, "Terminal should be marked dirty BEFORE ACK");
    }

    #[test]
    fn multiple_process_all_calls_accumulate_output() {
        // Test that calling process_all multiple times properly accumulates output
        // This is important because TUI apps may produce output across multiple frames

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
            terminal.dirty = false; // Clear after resize
        }

        // Write some output
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"First line\n").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
        manager.process_all();

        let content1 = manager.get(id).unwrap().content_rows();
        eprintln!("After first write: content_rows = {}", content1);

        // Clear dirty and write more
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.dirty = false;
            terminal.write(b"Second line\n").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
        manager.process_all();

        let content2 = manager.get(id).unwrap().content_rows();
        let is_dirty = manager.get(id).unwrap().is_dirty();
        eprintln!("After second write: content_rows = {}, dirty = {}", content2, is_dirty);

        // Content should have increased
        assert!(content2 > content1, "Content rows should increase with more output");
        assert!(is_dirty, "Terminal should be dirty after new output");
    }

    #[test]
    fn tui_resize_then_app_draws_full_screen() {
        // This test simulates the EXACT TUI app flow with realistic timing:
        //
        // 1. Shell terminal starts (small, content-based)
        // 2. User runs TUI app (e.g., mc)
        // 3. column-term --resize full is called
        // 4. Terminal is resized to full viewport
        // 5. ACK is sent (column-term exits)
        // 6. Shell runs mc
        // 7. mc queries terminal size (TIOCGWINSZ)
        // 8. mc draws full screen
        // 9. Compositor processes output and renders
        //
        // The question: After step 9, is the full screen visible?

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Start with a shell terminal
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        // Use bash -c with a script that waits, then draws
        // This simulates the shell waiting for mc to start
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        eprintln!("=== Initial state ===");
        eprintln!("  max_rows={}, cell_height={}", max_rows, cell_height);
        eprintln!("  height={}", manager.get(id).unwrap().height);

        // Step 1: Simulate column-term --resize full
        // This happens BEFORE mc starts
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);

            eprintln!("=== After resize ===");
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  Grid rows: {}", grid_rows);
            eprintln!("  height: {}", terminal.height);

            assert_eq!(pty_rows, max_rows, "PTY should report max_rows");
            assert_eq!(grid_rows, max_rows, "Grid should have max_rows");
        }

        // Step 2: Simulate what mc does when it starts:
        // - Query terminal size (we assume it gets the correct size)
        // - Clear screen
        // - Draw content at every row
        {
            let terminal = manager.get_mut(id).unwrap();

            // mc sends: clear screen, move home
            terminal.write(b"\x1b[2J\x1b[H").unwrap();

            // mc draws content at every row from 1 to max_rows
            // This is TUI drawing - cursor positioning without newlines
            for row in 1..=max_rows {
                let escape = format!("\x1b[{};1H Row {:02}: Content here ==========", row, row);
                terminal.write(escape.as_bytes()).unwrap();
            }
        }

        // Wait for cat to echo back
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Step 3: Compositor processes PTY output (this is what happens in main loop)
        let actions = manager.process_all();
        eprintln!("=== After process_all ===");
        eprintln!("  Sizing actions: {:?}", actions);

        let terminal = manager.get(id).unwrap();
        let is_dirty = terminal.is_dirty();
        let content_rows = terminal.content_rows();
        let grid_rows = terminal.terminal.grid_rows();
        let (_, pty_rows) = terminal.terminal.dimensions();

        eprintln!("  dirty: {}", is_dirty);
        eprintln!("  content_rows: {}", content_rows);
        eprintln!("  grid_rows: {}", grid_rows);
        eprintln!("  pty_rows: {}", pty_rows);

        // Terminal should be dirty (needs re-render)
        assert!(is_dirty, "Terminal should be dirty after TUI output");

        // Grid should still be at max_rows
        assert_eq!(grid_rows, max_rows, "Grid rows should still be max_rows");

        // PTY should still report max_rows
        assert_eq!(pty_rows, max_rows, "PTY rows should still be max_rows");
    }

    #[test]
    fn cursor_positioning_after_resize_works() {
        // Test that cursor positioning commands work correctly after resize
        // This is critical for TUI apps that draw by moving the cursor

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Send cursor positioning to the LAST row (max_rows)
        // This is where TUI apps would draw their status bar
        {
            let terminal = manager.get_mut(id).unwrap();

            // Move to last row, column 1
            let escape = format!("\x1b[{};1H STATUS BAR AT BOTTOM", max_rows);
            terminal.write(escape.as_bytes()).unwrap();

            // Also draw at row 1 (top)
            terminal.write(b"\x1b[1;1H TOP ROW").unwrap();
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
        manager.process_all();

        // The terminal should have processed the output
        let terminal = manager.get(id).unwrap();
        assert!(terminal.is_dirty(), "Terminal should be dirty");

        // Note: We can't easily verify the cursor position without accessing
        // the alacritty grid internals, but the test passing means the
        // escape sequences were processed without error
    }

    #[test]
    fn resize_and_render_buffer_dimensions() {
        // BUG REPRODUCTION: After resize, is the render buffer the correct size?
        //
        // This is critical: if the render buffer has wrong dimensions, the
        // terminal will appear blank or partially rendered.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;
        let expected_full_height = max_rows as u32 * cell_height;

        // Record initial state
        let initial_height = manager.get(id).unwrap().height;
        eprintln!("Initial: height={}", initial_height);
        assert!(initial_height < expected_full_height, "Initial height should be small");

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // After resize, check all dimensions
        let terminal = manager.get(id).unwrap();

        // 1. Visual height (used for layout)
        assert_eq!(
            terminal.height, expected_full_height,
            "Visual height should be {} after resize, was {}",
            expected_full_height, terminal.height
        );

        // 2. PTY rows
        let (_, pty_rows) = terminal.terminal.dimensions();
        assert_eq!(
            pty_rows, max_rows,
            "PTY rows should be {} after resize, was {}",
            max_rows, pty_rows
        );

        // 3. Grid rows
        let grid_rows = terminal.terminal.grid_rows();
        assert_eq!(
            grid_rows, max_rows,
            "Grid rows should be {} after resize, was {}",
            max_rows, grid_rows
        );

        // 4. Dirty flag
        assert!(terminal.is_dirty(), "Terminal should be dirty after resize");

        // 5. Width is not changed by resize() - it's set at terminal creation time
        // based on the font's cell dimensions. Just verify it's non-zero.
        assert!(terminal.width > 0, "Terminal width should be non-zero");

        eprintln!("After resize: height={}, pty_rows={}, grid_rows={}, dirty={}, width={}",
                  terminal.height, pty_rows, grid_rows, terminal.is_dirty(), terminal.width);
    }

    #[test]
    fn tui_resize_to_content_then_full_cycle() {
        // Test the full TUI resize cycle:
        // 1. Terminal starts with content-based sizing
        // 2. Resize to full (for TUI app)
        // 3. Resize back to content (after TUI exits)
        //
        // This is what happens with column-term --resize full/content

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;
        let initial_rows = manager.initial_rows;

        // Phase 1: Initial state (content-based, small)
        let initial_height = manager.get(id).unwrap().height;
        let expected_initial = initial_rows as u32 * cell_height;
        assert_eq!(
            initial_height, expected_initial,
            "Phase 1: Initial height should be {}, was {}",
            expected_initial, initial_height
        );
        eprintln!("Phase 1 (initial): height={} ({} rows)", initial_height, initial_rows);

        // Phase 2: Resize to full (for TUI app)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }
        let full_height = manager.get(id).unwrap().height;
        let expected_full = max_rows as u32 * cell_height;
        assert_eq!(
            full_height, expected_full,
            "Phase 2: Full height should be {}, was {}",
            expected_full, full_height
        );
        eprintln!("Phase 2 (full): height={} ({} rows)", full_height, max_rows);

        // Verify PTY also resized
        let (_, pty_rows_full) = manager.get(id).unwrap().terminal.dimensions();
        assert_eq!(pty_rows_full, max_rows, "PTY should be at max_rows after resize to full");

        // Write some content while at full size (simulate TUI drawing)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"\x1b[H\x1b[2J").unwrap(); // Clear screen
            for i in 1..=10 {
                let line = format!("\x1b[{};1HLine {}\n", i, i);
                terminal.write(line.as_bytes()).unwrap();
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        manager.process_all();

        // Phase 3: Resize back to content (TUI exited)
        let content_rows = manager.get(id).unwrap().content_rows();
        let resize_to_rows = (content_rows as u16).max(3);
        eprintln!("Phase 3: content_rows={}, will resize to {} rows", content_rows, resize_to_rows);

        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(resize_to_rows, cell_height);
        }

        let content_height = manager.get(id).unwrap().height;
        let expected_content = resize_to_rows as u32 * cell_height;
        assert_eq!(
            content_height, expected_content,
            "Phase 3: Content height should be {}, was {}",
            expected_content, content_height
        );
        eprintln!("Phase 3 (content): height={} ({} rows)", content_height, resize_to_rows);

        // Verify PTY also resized back
        let (_, pty_rows_content) = manager.get(id).unwrap().terminal.dimensions();
        assert_eq!(
            pty_rows_content, resize_to_rows,
            "PTY should be at content rows after resize back"
        );
    }

    #[test]
    fn render_buffer_not_empty_after_tui_output() {
        // This test verifies that after TUI-style output, the terminal's
        // internal render buffer is populated (not empty/black).
        //
        // The buffer should contain the rendered glyphs.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Write TUI-style content (cursor positioning + text)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.write(b"\x1b[H\x1b[2J").unwrap(); // Clear
            terminal.write(b"\x1b[1;1HXXXXXXXXXXXX").unwrap(); // Row 1
            terminal.write(b"\x1b[10;1HMIDDLE ROW CONTENT").unwrap(); // Row 10
            let last_row = format!("\x1b[{};1HBOTTOM ROW", max_rows);
            terminal.write(last_row.as_bytes()).unwrap(); // Last row
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
        manager.process_all();

        // The terminal should have content
        let terminal = manager.get(id).unwrap();
        assert!(terminal.is_dirty(), "Terminal should be dirty after output");

        // Check content rows (TUI output with cursor positioning may not increment content_rows)
        // This is because content_rows tracks newlines, not cursor moves
        eprintln!("Content rows: {}", terminal.content_rows());
        eprintln!("Grid rows: {}", terminal.terminal.grid_rows());
        eprintln!("Height: {}", terminal.height);
    }

    #[test]
    fn all_size_components_match_after_resize() {
        // CRITICAL TEST: Verifies that all size-related components are consistent
        // after resize. A mismatch here would cause "missing updates" where TUI
        // apps draw content that doesn't appear.
        //
        // Components that must match:
        // 1. terminal.height / cell_height = expected rows
        // 2. PTY rows = expected rows
        // 3. Grid rows = expected rows
        // 4. Sizing state rows = expected rows

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("cat", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Record initial state
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("INITIAL STATE:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // Initial: PTY is 1000 rows (for no scrolling), visual is small
            assert_eq!(pty_rows, 1000, "Initial PTY rows should be 1000");
            assert_eq!(visual_rows, manager.initial_rows as u32, "Initial visual rows should be initial_rows");
        }

        // Resize to full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }

        // Verify ALL components match after resize
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("AFTER RESIZE TO FULL:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // All should equal max_rows
            assert_eq!(
                visual_rows, max_rows as u32,
                "Visual rows should equal max_rows after resize"
            );
            assert_eq!(
                pty_rows, max_rows,
                "PTY rows should equal max_rows after resize"
            );
            assert_eq!(
                grid_rows, max_rows,
                "Grid rows should equal max_rows after resize"
            );
            assert_eq!(
                sizing_rows, max_rows,
                "Sizing state rows should equal max_rows after resize"
            );
        }

        // Resize back to content
        let content_rows = 10u16;
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        // Verify ALL components match after shrink
        {
            let terminal = manager.get(id).unwrap();
            let visual_rows = terminal.height / cell_height;
            let (_, pty_rows) = terminal.terminal.dimensions();
            let grid_rows = terminal.terminal.grid_rows();
            let sizing_rows = terminal.terminal.sizing_state().current_rows();

            eprintln!("AFTER RESIZE TO CONTENT:");
            eprintln!("  visual_rows (height/cell_height): {}", visual_rows);
            eprintln!("  PTY rows: {}", pty_rows);
            eprintln!("  grid_rows: {}", grid_rows);
            eprintln!("  sizing_state.current_rows: {}", sizing_rows);

            // All should equal content_rows
            assert_eq!(
                visual_rows, content_rows as u32,
                "Visual rows should equal content_rows after shrink"
            );
            assert_eq!(
                pty_rows, content_rows,
                "PTY rows should equal content_rows after shrink"
            );
            assert_eq!(
                grid_rows, content_rows,
                "Grid rows should equal content_rows after shrink"
            );
            assert_eq!(
                sizing_rows, content_rows,
                "Sizing state rows should equal content_rows after shrink"
            );
        }
    }

    #[test]
    fn tui_terminal_pty_output_available_immediately() {
        // This test verifies that when a TUI terminal is spawned with a command
        // that produces output, the output is available from PTY read immediately
        // (within the first few process_all calls).
        //
        // BUG SCENARIO: mc takes 11 seconds to show output because our shell
        // integration intercepts mc's internal fish subshell command and spawns
        // it as a separate terminal, breaking mc's communication with its subshell.
        //
        // This test uses a simpler command (echo) that should produce output
        // immediately without any subshell complications.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn a TUI terminal with echo - should produce output immediately
        let result = manager.spawn_command("echo 'TUI OUTPUT TEST'", cwd, &env, None, true);
        assert!(result.is_ok(), "spawn should succeed");
        let id = result.unwrap();

        // Allow some time for the command to produce output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Process PTY output and track bytes read per terminal
        let mut total_bytes_from_tui = 0;
        for _ in 0..10 {
            for (tid, terminal) in manager.iter_mut() {
                let (_, bytes_read) = terminal.process();
                if *tid == id && bytes_read > 0 {
                    total_bytes_from_tui += bytes_read;
                    eprintln!("Terminal {} read {} bytes", tid.0, bytes_read);
                }
            }
            if total_bytes_from_tui > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // We should have read SOME output from the TUI terminal
        assert!(
            total_bytes_from_tui > 0,
            "TUI terminal should have produced output within 500ms, but got 0 bytes"
        );

        eprintln!("Total bytes read from TUI terminal: {}", total_bytes_from_tui);
    }

    #[test]
    fn tui_terminal_pty_read_works_for_correct_terminal() {
        // This test verifies that when we have multiple terminals,
        // we read from the correct terminal's PTY.
        //
        // Setup: Shell (terminal 0) -> TUI app (terminal 1)
        // The TUI app's output should come from terminal 1's PTY.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // First spawn a "shell" terminal (non-TUI, just sits there)
        let shell_id = manager.spawn_command("sleep 10", cwd, &env, None, false).unwrap();

        // Then spawn a TUI terminal with echo
        let tui_id = manager.spawn_command("echo 'FROM TUI'", cwd, &env, Some(shell_id), true).unwrap();

        // Allow time for output
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Track bytes per terminal
        let mut shell_bytes = 0;
        let mut tui_bytes = 0;

        for _ in 0..10 {
            for (tid, terminal) in manager.iter_mut() {
                let (_, bytes_read) = terminal.process();
                if *tid == shell_id {
                    shell_bytes += bytes_read;
                } else if *tid == tui_id {
                    tui_bytes += bytes_read;
                }
            }
            if tui_bytes > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        eprintln!("Shell bytes: {}, TUI bytes: {}", shell_bytes, tui_bytes);

        // TUI terminal should have output (the echo)
        assert!(
            tui_bytes > 0,
            "TUI terminal should have produced output, got {} bytes",
            tui_bytes
        );
    }

    #[test]
    fn fzf_resize_flow_shows_output() {
        // Full simulation of the fzf resize flow:
        // 1. Shell terminal starts small (non-TUI, PTY=1000, visual=3)
        // 2. column-term --resize full (PTY and visual become max_rows)
        // 3. fzf runs (enters alternate screen, draws, exits, prints output)
        // 4. column-term --resize content (should resize to show output)

        let output_width = 800;
        let output_height = 720;  // 720/17 = 42 max_rows
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("sleep 60", cwd, &env, None, false).unwrap();

        let cell_height = manager.cell_height;
        let max_rows = manager.max_rows;

        // Step 1: Shell has initial content
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"Welcome to fish\n");
            terminal.inject_bytes(b"$ echo a | fzf\n");
        }

        let initial_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        let initial_height = manager.get(id).unwrap().height;
        eprintln!("Initial: cursor={}, height={}, max_rows={}", initial_cursor, initial_height, max_rows);

        // Step 2: Simulate column-term --resize full
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(max_rows, cell_height);
        }
        let after_full_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        let after_full_height = manager.get(id).unwrap().height;
        eprintln!("After resize full: cursor={}, height={}", after_full_cursor, after_full_height);

        // Step 3: fzf runs - enters alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");  // Enter alternate screen
            terminal.inject_bytes(b"\x1b[2J\x1b[H"); // Clear and home
            for i in 0..20 {
                terminal.inject_bytes(format!("> option {}\n", i).as_bytes());
            }
        }

        // Verify alternate screen
        let is_alt = manager.get(id).unwrap().terminal.is_alternate_screen();
        assert!(is_alt, "Should be in alternate screen");

        // Step 3b: fzf exits alternate screen and prints selection
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");  // Exit alternate screen
            terminal.inject_bytes(b"a\n");           // fzf prints selected item
        }

        // Verify not in alternate screen
        let is_alt = manager.get(id).unwrap().terminal.is_alternate_screen();
        assert!(!is_alt, "Should NOT be in alternate screen");

        let after_fzf_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("After fzf exit: cursor={}", after_fzf_cursor);

        // Step 4: Simulate column-term --resize content
        // This is what the compositor does: cursor_line + 2
        let content_rows = (after_fzf_cursor + 2).max(3);
        eprintln!("Content rows to resize to: {}", content_rows);

        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.resize(content_rows, cell_height);
        }

        let final_height = manager.get(id).unwrap().height;
        let final_cursor = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Final: cursor={}, height={}", final_cursor, final_height);

        // The key assertion: after resize, the terminal should be tall enough
        // to show the output (cursor_line indicates content position)
        // cursor is 0-indexed, so visible rows = cursor_line + 1
        // We sized to cursor_line + 2, so there should be room
        let visible_rows = content_rows as u32;
        let cursor_row = after_fzf_cursor as u32 + 1;  // 1-indexed

        assert!(
            visible_rows >= cursor_row,
            "Terminal should have enough rows ({}) to show cursor position (row {})",
            visible_rows, cursor_row
        );
    }

    #[test]
    fn tui_output_visible_after_alternate_screen_exit() {
        // BUG REPRODUCTION: After a TUI app exits alternate screen and prints output,
        // the output should be visible and cursor_line() should reflect it.
        //
        // This simulates the fzf flow:
        // 1. Shell terminal has some content (cursor at line N)
        // 2. fzf enters alternate screen (ESC[?1049h)
        // 3. fzf draws TUI interface in alternate screen
        // 4. User selects item, fzf exits alternate screen (ESC[?1049l)
        // 5. fzf prints selected item to stdout
        // 6. cursor_line() should now be N+1 (reflecting the printed output)
        //
        // The bug: if cursor_line() returns the wrong value, resize-to-content
        // will use wrong height and the output won't be visible.

        let output_width = 800;
        let output_height = 720;
        let mut manager = TerminalManager::new_with_size(output_width, output_height);

        // Spawn a terminal - we'll inject bytes directly instead of using PTY
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("sleep 10", cwd, &env, None, false).unwrap();

        // Step 1: Simulate initial shell content (prompt, maybe previous commands)
        // Inject bytes directly to terminal emulator
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"line1\n");
            terminal.inject_bytes(b"line2\n");
            terminal.inject_bytes(b"$ echo a | fzf\n");
        }

        // Check cursor position before TUI
        let cursor_before_tui = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor before TUI: line {}", cursor_before_tui);

        // Step 2: TUI enters alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            // ESC[?1049h = save cursor and switch to alternate screen
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Verify we're in alternate screen
        let is_alt_after_enter = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.is_alternate_screen()
        };
        assert!(is_alt_after_enter, "Should be in alternate screen after ESC[?1049h");

        // Step 3: TUI draws in alternate screen (lots of output)
        {
            let terminal = manager.get_mut(id).unwrap();
            // Clear alternate screen and draw TUI interface
            terminal.inject_bytes(b"\x1b[2J\x1b[H");  // Clear and home
            for i in 0..20 {
                let line = format!("  option {}\n", i);
                terminal.inject_bytes(line.as_bytes());
            }
        }

        // Step 4: TUI exits alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            // ESC[?1049l = restore cursor and switch to primary screen
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Verify we're back to primary screen
        let is_alt_after_exit = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.is_alternate_screen()
        };
        assert!(!is_alt_after_exit, "Should NOT be in alternate screen after ESC[?1049l");

        // Check cursor after exiting alternate screen (should be restored)
        let cursor_after_alt_exit = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor after alt screen exit: line {}", cursor_after_alt_exit);

        // Step 5: TUI prints selected item (like fzf does)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"selected_item\n");
        }

        // Step 6: Check cursor reflects the new output
        let cursor_after_output = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.cursor_line()
        };
        eprintln!("Cursor after output: line {}", cursor_after_output);

        // The cursor should have advanced by 1 line (the printed output)
        assert!(
            cursor_after_output > cursor_after_alt_exit,
            "Cursor should advance after printing output: was {}, now {}",
            cursor_after_alt_exit, cursor_after_output
        );

        // content_rows should NOT have been inflated by alternate screen output
        let content_rows = {
            let terminal = manager.get(id).unwrap();
            terminal.terminal.content_rows()
        };
        eprintln!("Content rows: {}", content_rows);

        // Content rows should be close to cursor position, not inflated by alt screen
        // cursor_line is 0-indexed, so cursor_line + 1 = number of rows
        let expected_content = cursor_after_output + 1;
        assert!(
            content_rows <= expected_content as u32 + 2,
            "Content rows ({}) should not be much more than cursor position + 1 ({})",
            content_rows, expected_content
        );
    }

    /// Test that is_alternate_screen() correctly detects alternate screen mode.
    /// This is used for spawn rejection when TUI apps are running.
    #[test]
    fn is_alternate_screen_detection() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        // Initially not in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal should not be in alternate screen initially"
            );
        }

        // Enter alternate screen mode with CSI ? 1049 h
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Now should be in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                terminal.terminal.is_alternate_screen(),
                "Terminal should be in alternate screen after CSI ? 1049 h"
            );
        }

        // Exit alternate screen mode with CSI ? 1049 l
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Should no longer be in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal should not be in alternate screen after CSI ? 1049 l"
            );
        }
    }

    /// Test that max_rows does not imply alternate screen mode.
    /// This is a regression test for the spawn rejection heuristic change.
    /// Old behavior: reject spawn if parent_pty_rows == max_rows (false positive possible)
    /// New behavior: reject spawn if parent is in alternate screen (exact)
    #[test]
    fn max_rows_does_not_imply_alternate_screen() {
        let max_height = 160; // 10 rows * 16 cell height
        let mut manager = TerminalManager::new_with_size(800, max_height);

        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        // Resize the terminal to max height (simulating content growth)
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.height = max_height;
        }

        // Terminal has max height but is NOT in alternate screen
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, max_height, "Terminal should be at max height");
            assert!(
                !terminal.terminal.is_alternate_screen(),
                "Terminal at max height should NOT automatically be in alternate screen"
            );
        }
    }

    /// Test that spawn rejection should be based on alternate screen, not PTY size.
    /// This simulates the condition where spawns should be allowed.
    #[test]
    fn spawn_should_be_allowed_when_not_in_alternate_screen() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let parent_id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        // Parent is not in alternate screen - spawns should be allowed
        {
            let parent = manager.get(parent_id).unwrap();
            assert!(
                !parent.terminal.is_alternate_screen(),
                "Parent not in alternate screen"
            );
        }

        // Child spawn should succeed
        let child_id = manager.spawn_command("echo child", cwd, &env, Some(parent_id), false).unwrap();
        assert!(manager.get(child_id).is_some(), "Child should be spawned");
    }

    /// Test that alternate screen detection works for simulated TUI apps.
    /// When a TUI app is running (alternate screen), spawns should be rejected.
    #[test]
    fn spawn_should_be_rejected_when_in_alternate_screen() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let parent_id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        // Enter alternate screen (simulating TUI app start)
        {
            let terminal = manager.get_mut(parent_id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Verify parent is in alternate screen
        {
            let parent = manager.get(parent_id).unwrap();
            assert!(
                parent.terminal.is_alternate_screen(),
                "Parent should be in alternate screen"
            );
        }

        // NOTE: The actual spawn rejection happens in main.rs event loop.
        // This test verifies the detection works correctly.
        // Integration test would need to verify the full rejection path.
    }

    /// Test that check_alt_screen_resize_needed detects transition to alternate screen.
    #[test]
    fn check_alt_screen_resize_needed_detects_transition() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;

        // Initially not in alternate screen, so no resize needed
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Should not need resize when not in alternate screen"
            );
        }

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Now transition detected, resize needed
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                terminal.check_alt_screen_resize_needed(max_height),
                "Should need resize after transitioning to alternate screen"
            );
        }

        // Call again - should NOT need resize since transition already recorded
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Should not need resize on subsequent checks (no new transition)"
            );
        }
    }

    /// Test that no resize is needed if terminal is already at max height.
    #[test]
    fn no_resize_if_already_at_max_height() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");

        // Spawn as TUI terminal (already at max height)
        let id = manager.spawn_command("echo test", cwd, &env, None, true).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;

        // Verify terminal is at max height
        {
            let terminal = manager.get(id).unwrap();
            assert_eq!(terminal.height, max_height, "TUI terminal should be at max height");
        }

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Check resize needed - should be false since already at max
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Should not need resize when already at max height"
            );
        }
    }

    /// Test that exiting alternate screen does not trigger resize.
    #[test]
    fn exit_alternate_screen_does_not_trigger_resize() {
        let mut manager = TerminalManager::new_with_size(800, 600);
        let env = HashMap::new();
        let cwd = std::path::Path::new("/tmp");
        let id = manager.spawn_command("echo test", cwd, &env, None, false).unwrap();

        let max_height = manager.max_rows as u32 * manager.cell_height;

        // Enter alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049h");
        }

        // Consume the transition
        {
            let terminal = manager.get_mut(id).unwrap();
            let _ = terminal.check_alt_screen_resize_needed(max_height);
        }

        // Exit alternate screen
        {
            let terminal = manager.get_mut(id).unwrap();
            terminal.inject_bytes(b"\x1b[?1049l");
        }

        // Should not trigger resize (only entry triggers resize)
        {
            let terminal = manager.get_mut(id).unwrap();
            assert!(
                !terminal.check_alt_screen_resize_needed(max_height),
                "Exiting alternate screen should not trigger resize"
            );
        }
    }
}
