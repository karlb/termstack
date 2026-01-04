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
use terminal::Theme;
use terminal::sizing::SizingAction;

use crate::coords::RenderY;

/// Unique identifier for a managed terminal
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalId(pub u32);

/// Terminal visibility state machine.
///
/// Replaces the old `hidden` + `has_had_output` boolean pair with an explicit
/// state machine. This makes the visibility rules clear and documents all
/// valid transitions.
///
/// # State Transitions
///
/// ```text
/// Shell: new() ──────────────────────────> AlwaysVisible
///                                               │
///                                        (gui foreground)
///                                               │
///                                               v
///                                    HiddenForForegroundGui
///                                               │
///                                          (gui exit)
///                                               │
///                                               v
///                                         AlwaysVisible
///
/// Command: new_with_command() ──────────> WaitingForOutput
///                                               │
///                     ┌─────────────────────────┼─────────────────────────┐
///                     │                         │                         │
///                 (output)               (alt-screen)                  (exit)
///                     │                         │                         │
///                     v                         v                         v
///                 HasOutput               HasOutput                 ExitedEmpty
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisibilityState {
    /// Shell terminal - always visible from creation
    AlwaysVisible,

    /// Command terminal waiting for first output before becoming visible
    WaitingForOutput,

    /// Command terminal that has produced output - visible forever
    HasOutput,

    /// Command terminal that exited without ever producing output
    ExitedEmpty,

    /// Launching terminal hidden while a foreground GUI app is running
    HiddenForForegroundGui,
}

impl VisibilityState {
    /// Returns whether the terminal should be rendered
    pub fn is_visible(&self) -> bool {
        matches!(self, Self::AlwaysVisible | Self::HasOutput)
    }

    /// Transition when first output arrives
    pub fn on_output(self) -> Self {
        match self {
            Self::WaitingForOutput => Self::HasOutput,
            other => other,
        }
    }

    /// Transition when entering alternate screen (TUI apps like fzf)
    pub fn on_alt_screen_enter(self) -> Self {
        match self {
            Self::WaitingForOutput => Self::HasOutput,
            other => other,
        }
    }

    /// Transition when process exits
    pub fn on_exit(self) -> Self {
        match self {
            Self::WaitingForOutput => Self::ExitedEmpty,
            other => other,
        }
    }

    /// Transition when a foreground GUI app exits - restore visibility
    pub fn on_gui_exit(self) -> Self {
        match self {
            Self::HiddenForForegroundGui => Self::AlwaysVisible,
            other => other,
        }
    }
}

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

    /// Title for the title bar
    pub title: String,

    /// Whether to show the title bar (false for initial shell terminals)
    pub show_title_bar: bool,

    /// Cached texture for rendering
    texture: Option<GlesTexture>,

    /// Whether the terminal needs re-rendering
    dirty: bool,

    /// Keep window open after process exits (for command terminals)
    pub keep_open: bool,

    /// Whether the process has exited (for hiding cursor)
    exited: bool,

    /// Visibility state machine - the source of truth for visibility
    pub visibility: VisibilityState,

    /// Parent terminal that spawned this one (if any)
    /// When this terminal exits, the parent is unhidden
    pub parent: Option<TerminalId>,

    /// Previous alternate screen state (for detecting transitions)
    prev_alt_screen: bool,

    /// Whether this terminal has been manually resized
    /// When true, auto-growth is disabled (user explicitly chose a size)
    pub manually_sized: bool,
}

impl ManagedTerminal {
    /// Create a new managed terminal
    pub fn new(id: TerminalId, cols: u16, rows: u16, cell_width: u32, cell_height: u32, theme: Theme) -> Result<Self, terminal::state::TerminalError> {
        let terminal = Terminal::new_with_theme(cols, rows, theme)?;

        // Use shell name as title
        let title = std::env::var("SHELL")
            .ok()
            .and_then(|s| s.rsplit('/').next().map(String::from))
            .unwrap_or_else(|| "Terminal".to_string());

        Ok(Self {
            terminal,
            id,
            width: cols as u32 * cell_width,
            height: rows as u32 * cell_height,
            title,
            show_title_bar: false, // Shell terminals don't show title bar
            texture: None,
            dirty: true,
            keep_open: false,
            exited: false,
            visibility: VisibilityState::AlwaysVisible,
            parent: None,
            prev_alt_screen: false,
            manually_sized: false,
        })
    }

    /// Create a new managed terminal running a specific command
    ///
    /// - `pty_rows`: Size reported to the PTY (program sees this many rows)
    /// - `visual_rows`: Initial visual size for display
    /// - `parent`: Parent terminal to unhide when this one exits
    #[allow(clippy::too_many_arguments)]
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
        theme: Theme,
    ) -> Result<Self, terminal::state::TerminalError> {
        let terminal = Terminal::new_with_command_and_theme(cols, pty_rows, visual_rows, command, working_dir, env, theme)?;

        Ok(Self {
            terminal,
            id,
            width: cols as u32 * cell_width,
            height: visual_rows as u32 * cell_height, // Use visual rows for display
            title: command.to_string(),
            show_title_bar: true, // Command terminals show title bar
            texture: None,
            dirty: true,
            keep_open: true, // Command terminals stay open after exit
            exited: false,
            visibility: VisibilityState::WaitingForOutput,
            parent,
            prev_alt_screen: false,
            manually_sized: false,
        })
    }

    /// Returns whether this terminal should be visible (rendered)
    pub fn is_visible(&self) -> bool {
        self.visibility.is_visible()
    }

    /// Returns whether this terminal has ever produced output.
    /// Once true, stays true forever.
    pub fn has_had_output(&self) -> bool {
        matches!(self.visibility, VisibilityState::AlwaysVisible | VisibilityState::HasOutput)
    }

    /// Process PTY output and mark dirty if needed
    pub fn process(&mut self) -> (Vec<SizingAction>, usize) {
        let (actions, bytes_read) = self.terminal.process_pty_with_count();
        // Only mark dirty when there's actual output to render
        if bytes_read > 0 && !self.dirty {
            self.dirty = true;
        }

        // If terminal has output for the first time, transition visibility state
        if self.visibility == VisibilityState::WaitingForOutput && self.terminal.has_meaningful_content() {
            self.visibility = self.visibility.on_output();
            tracing::info!(id = self.id.0, "terminal has meaningful output, now permanently visible");
        }

        (actions, bytes_read)
    }

    /// Write input to the terminal
    pub fn write(&mut self, data: &[u8]) -> Result<(), terminal::state::TerminalError> {
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
    /// already at full height. This allows reactive resizing for TUI apps.
    ///
    /// Updates internal state to track the transition. Also makes hidden terminals
    /// visible when they enter alternate screen (since TUI apps like fzf enter
    /// alternate screen before producing any content_rows).
    pub fn check_alt_screen_resize_needed(&mut self, max_height: u32) -> bool {
        let is_alt = self.terminal.is_alternate_screen();
        let transitioned_to_alt = is_alt && !self.prev_alt_screen;
        self.prev_alt_screen = is_alt;

        if transitioned_to_alt {
            // Make visible when entering alternate screen (TUI apps like fzf)
            if self.visibility == VisibilityState::WaitingForOutput {
                self.visibility = self.visibility.on_alt_screen_enter();
                tracing::info!(
                    id = self.id.0,
                    "terminal entered alternate screen, now visible"
                );
            }

            if self.height < max_height {
                tracing::info!(
                    id = self.id.0,
                    current_height = self.height,
                    max_height,
                    "terminal entered alternate screen, needs resize"
                );
                return true;
            }
        }
        false
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

    /// Resize to a specific pixel height (used for manual drag resize)
    /// Also sets `manually_sized` to disable auto-growth
    ///
    /// NOTE: Does NOT mark dirty - texture re-rendering is too slow (~30ms).
    /// The caller must call mark_dirty() when resize ends to trigger final render.
    pub fn resize_to_height(&mut self, height_px: u32, cell_height: u32) {
        // Update visual height for layout calculations (don't re-render texture yet)
        self.height = height_px;
        self.manually_sized = true;

        // Resize PTY if row count changed (so programs see correct size)
        let target_rows = (height_px / cell_height).max(1) as u16;
        let (_, current_rows) = self.terminal.dimensions();
        if target_rows != current_rows {
            let action = self.terminal.configure(target_rows);
            if let SizingAction::ApplyResize { .. } = action {
                self.terminal.complete_resize();
            }
        }
    }

    /// Resize columns (width change from compositor resize)
    pub fn resize_cols(&mut self, cols: u16, cell_width: u32) {
        self.terminal.resize_cols(cols);
        self.width = cols as u32 * cell_width;
        self.dirty = true;
    }

    /// Get the terminal's PTY fd for polling
    pub fn pty_fd(&self) -> RawFd {
        self.terminal.pty_fd()
    }

    /// Check if terminal is still running (no side effects)
    pub fn is_running(&mut self) -> bool {
        self.terminal.is_running()
    }

    /// Mark terminal as exited (hides cursor on next render)
    pub fn mark_exited(&mut self) {
        self.exited = true;
    }

    /// Check if terminal process has exited
    pub fn has_exited(&self) -> bool {
        self.exited
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

    /// Cell dimensions
    pub cell_width: u32,
    pub cell_height: u32,

    /// Default terminal width in cells
    pub default_cols: u16,

    /// Initial terminal height in rows (content-aware: starts small)
    pub initial_rows: u16,

    /// Maximum terminal height in rows (capped at viewport)
    pub max_rows: u16,

    /// Color theme for terminals
    theme: Theme,
}

impl TerminalManager {
    /// Create a new terminal manager with output size
    pub fn new_with_size(output_width: u32, output_height: u32, theme: Theme) -> Self {
        // Default cell dimensions (will be updated when font loads)
        let cell_width = 8u32;
        let cell_height = 17u32;  // Match fontdue's line height calculation

        // Calculate cols to fill width
        let default_cols = (output_width / cell_width).max(1) as u16;
        // Max rows based on viewport height
        let max_rows = (output_height / cell_height).max(1) as u16;
        // Start with minimal height (content-aware sizing will grow as needed)
        let initial_rows = 1;

        Self {
            terminals: HashMap::new(),
            next_id: 0,
            cell_width,
            cell_height,
            default_cols,
            initial_rows,
            max_rows,
            theme,
        }
    }

    /// Create a new terminal manager with default size
    pub fn new() -> Self {
        Self::new_with_size(800, 600, Theme::default())
    }

    /// Get the focused terminal mutably based on the compositor's focused window
    pub fn get_focused_mut(&mut self, focused_window: Option<&crate::state::FocusedWindow>) -> Option<&mut ManagedTerminal> {
        use crate::state::FocusedWindow;
        let id = match focused_window? {
            FocusedWindow::Terminal(id) => *id,
            FocusedWindow::External(_) => return None,
        };
        self.terminals.get_mut(&id)
    }

    /// Calculate total height of visible terminals
    pub fn total_height(&self) -> i32 {
        self.terminals.values()
            .filter(|t| t.is_visible())
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

    /// Update output size (called when compositor window is resized)
    pub fn update_output_size(&mut self, width: u32, height: u32) {
        self.default_cols = (width / self.cell_width).max(1) as u16;
        self.max_rows = (height / self.cell_height).max(1) as u16;
    }

    /// Resize all terminals to new column width
    pub fn resize_all_terminals(&mut self, output_width: u32) {
        let new_cols = (output_width / self.cell_width).max(1) as u16;
        let cell_width = self.cell_width;

        for terminal in self.terminals.values_mut() {
            terminal.resize_cols(new_cols, cell_width);
        }

        tracing::info!(
            new_cols,
            terminal_count = self.terminals.len(),
            "resized all terminals to new width"
        );
    }

    /// Grow a terminal to accommodate more content (capped at max_rows)
    pub fn grow_terminal(&mut self, id: TerminalId, target_rows: u16) {
        let max_rows = self.max_rows;
        let cell_height = self.cell_height;

        if let Some(terminal) = self.terminals.get_mut(&id) {
            let old_height = terminal.height;
            let new_rows = target_rows.min(max_rows);
            terminal.resize(new_rows, cell_height);
            tracing::info!(
                id = id.0,
                target_rows,
                new_rows,
                max_rows,
                old_height,
                new_height = terminal.height,
                "grew terminal"
            );
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
            self.theme,
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

        Ok(id)
    }

    /// Spawn a new terminal running a specific command
    ///
    /// If `parent` is provided, that terminal will be unhidden when this one exits.
    /// Terminals start hidden and become visible when they produce output.
    /// TUI apps are detected via alternate screen mode and auto-resized.
    pub fn spawn_command(
        &mut self,
        command: &str,
        working_dir: &Path,
        env: &HashMap<String, String>,
        parent: Option<TerminalId>,
    ) -> Result<TerminalId, terminal::state::TerminalError> {
        let id = TerminalId(self.next_id);
        self.next_id += 1;

        // Use large PTY (no scrolling) but small visual size
        // TUI apps will auto-resize when alternate screen is detected
        let (pty_rows, visual_rows) = (1000, self.initial_rows);

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
            self.theme,
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

        // Command terminals start hidden until they produce output
        // (has_had_output defaults to false in new_with_command)

        tracing::info!(id = id.0, cols = self.default_cols, pty_rows, visual_rows,
                       height = terminal.height, max_rows = self.max_rows, cell_height = self.cell_height,
                       ?parent, command, "spawned command terminal");

        self.terminals.insert(id, terminal);

        // Debug: show which terminals are hidden/visible
        for (tid, term) in &self.terminals {
            tracing::info!(tid = tid.0, visible = term.is_visible(), height = term.height, "terminal state after spawn");
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

    /// Get visible terminal IDs in order
    pub fn visible_ids(&self) -> Vec<TerminalId> {
        let mut ids: Vec<_> = self.terminals.iter()
            .filter(|(_, term)| term.is_visible())
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
        self.terminals.values().filter(|t| t.is_visible()).count()
    }

    /// Process all terminal PTY output
    pub fn process_all(&mut self) -> Vec<(TerminalId, SizingAction)> {
        let mut actions = Vec::new();
        for (id, terminal) in &mut self.terminals {
            let content_before = terminal.content_rows();
            let (term_actions, bytes_read) = terminal.process();
            let content_after = terminal.content_rows();

            if bytes_read > 0 {
                tracing::info!(
                    id = id.0,
                    bytes_read,
                    content_before,
                    content_after,
                    actions_count = term_actions.len(),
                    "PTY read for terminal"
                );
            }

            // Log each action for visibility
            for action in &term_actions {
                tracing::info!(id = id.0, ?action, "terminal sizing action");
                actions.push((*id, action.clone()));
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
        let mut parents_to_focus = Vec::new();
        let mut terminals_to_transition = Vec::new();
        let mut focus_changed_to = None;

        for id in ids {
            if let Some(term) = self.terminals.get_mut(&id) {
                let was_already_exited = term.exited;
                let running = term.is_running();

                if !running && !was_already_exited {
                    // First time detecting exit - mark as exited
                    term.mark_exited();

                    // Drain PTY buffer before checking content
                    // This ensures we capture any output that was written before exit
                    term.process();

                    // Handle parent focus
                    if let Some(parent_id) = term.parent {
                        parents_to_focus.push(parent_id);
                        focus_changed_to = Some(parent_id);
                        tracing::info!(child = id.0, parent = parent_id.0, "command exited, focusing parent");

                        // Transition visibility state on exit
                        // WaitingForOutput -> ExitedEmpty (hidden)
                        // HasOutput -> HasOutput (stays visible)
                        if term.visibility == VisibilityState::WaitingForOutput {
                            terminals_to_transition.push(id);
                            tracing::info!(id = id.0, "command terminal exited without output");
                        }
                    }
                }

                if !running && !term.keep_open {
                    dead.push(id);
                }
            }
        }

        // Transition visibility for exited terminals
        for id in terminals_to_transition {
            if let Some(term) = self.terminals.get_mut(&id) {
                term.visibility = term.visibility.on_exit();
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

}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}


#[cfg(test)]
mod tests;
