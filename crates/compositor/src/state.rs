//! Compositor state machine
//!
//! This module contains the main compositor state, implementing explicit state
//! tracking to prevent the bugs encountered in v1.

use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_xdg_shell;
use smithay::wayland::output::OutputHandler;
use smithay::desktop::{Space, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Physical, Point, Size, SERIAL_COUNTER};
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    with_states, CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler,
    XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};

use std::os::unix::net::UnixStream;

use crate::ipc::{ResizeMode, SpawnRequest};
use crate::layout::ColumnLayout;
use crate::terminal_manager::TerminalId;

/// Main compositor state
pub struct ColumnCompositor {
    /// Wayland display handle
    pub display_handle: DisplayHandle,

    /// Event loop handle
    pub loop_handle: LoopHandle<'static, Self>,

    /// Wayland protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,

    /// Desktop space for managing external windows
    pub space: Space<Window>,

    /// All cells in column order (terminals and external windows unified)
    pub cells: Vec<ColumnCell>,

    /// Current scroll offset (pixels from top)
    pub scroll_offset: f64,

    /// Index of focused cell (works for both terminals and windows)
    pub focused_index: Option<usize>,

    /// Cached layout calculation
    pub layout: ColumnLayout,

    /// Output dimensions
    pub output_size: Size<i32, Physical>,

    /// The seat
    pub seat: Seat<Self>,

    /// Running state
    pub running: bool,

    /// Flag to spawn a new terminal (set by input handler)
    pub spawn_terminal_requested: bool,

    /// Focus navigation request (1 = next, -1 = prev)
    pub focus_change_requested: i32,

    /// Scroll request (in pixels, positive = down)
    pub scroll_requested: f64,

    /// Cached cell heights for consistent positioning between input and render
    /// Updated at start of each frame before input processing
    pub cached_cell_heights: Vec<i32>,

    /// Pending spawn requests from IPC (column-term commands)
    pub pending_spawn_requests: Vec<SpawnRequest>,

    /// Pending resize request from IPC (column-term resize)
    /// Includes the stream for sending acknowledgement after resize completes
    pub pending_resize_request: Option<(ResizeMode, UnixStream)>,

    /// Index of newly added external window (for scroll-to-show)
    pub new_external_window_index: Option<usize>,

    /// Index and new height of resized external window (for scroll adjustment)
    pub external_window_resized: Option<(usize, i32)>,

    /// Pending output terminal to link with the next external window
    /// Set when spawning a GUI app command, consumed by add_window()
    pub pending_window_output_terminal: Option<TerminalId>,

    /// Pending command string for the next external window's title bar
    /// Set when spawning a GUI app command, consumed by add_window()
    pub pending_window_command: Option<String>,

    /// Output terminals from closed windows that need cleanup
    /// Processed in main loop - if terminal has no content, remove it; otherwise keep visible
    pub pending_output_terminal_cleanup: Vec<TerminalId>,
}

/// A window entry in our column
pub struct WindowEntry {
    /// The toplevel surface
    pub toplevel: ToplevelSurface,

    /// The window wrapper for space management
    pub window: Window,

    /// Explicit state machine
    pub state: WindowState,

    /// Output terminal for this window (captures stdout/stderr from GUI app)
    /// Hidden until output arrives, then promoted to standalone cell below window
    pub output_terminal: Option<TerminalId>,

    /// Command that spawned this window (for title bar display)
    pub command: String,
}

/// Explicit window state machine - prevents implicit state bugs from v1
#[derive(Debug, Clone)]
pub enum WindowState {
    /// Window is stable, accepting input
    Active { height: u32 },

    /// Resize requested, waiting for client acknowledgment
    PendingResize {
        current_height: u32,
        requested_height: u32,
        request_serial: u32,
    },

    /// Client acknowledged, waiting for commit with new size
    AwaitingCommit {
        current_height: u32,
        target_height: u32,
    },
}

impl WindowState {
    /// Get the current height for layout purposes
    pub fn current_height(&self) -> u32 {
        match self {
            Self::Active { height } => *height,
            Self::PendingResize { current_height, .. } => *current_height,
            Self::AwaitingCommit { current_height, .. } => *current_height,
        }
    }
}

/// A cell in the column layout - either a terminal or external window
pub enum ColumnCell {
    /// An internal terminal, referenced by ID (actual terminal data in TerminalManager)
    Terminal(TerminalId),
    /// An external Wayland window (owns the WindowEntry directly)
    External(WindowEntry),
}

impl ColumnCompositor {
    /// Create a new compositor state
    /// Returns (compositor, display) - display must be kept alive for dispatching
    pub fn new(
        display: Display<Self>,
        loop_handle: LoopHandle<'static, Self>,
        output_size: Size<i32, Physical>,
    ) -> (Self, Display<Self>) {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");

        // Add keyboard and pointer capabilities
        seat.add_keyboard(Default::default(), 200, 25).expect("Failed to add keyboard");
        seat.add_pointer();

        let compositor = Self {
            display_handle,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            space: Space::default(),
            cells: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
            layout: ColumnLayout::empty(),
            output_size,
            seat,
            running: true,
            spawn_terminal_requested: false,
            focus_change_requested: 0,
            scroll_requested: 0.0,
            cached_cell_heights: Vec::new(),
            pending_spawn_requests: Vec::new(),
            pending_resize_request: None,
            new_external_window_index: None,
            external_window_resized: None,
            pending_window_output_terminal: None,
            pending_window_command: None,
            pending_output_terminal_cleanup: Vec::new(),
        };

        (compositor, display)
    }

    /// Recalculate layout after any change
    pub fn recalculate_layout(&mut self) {
        // Use cached heights for layout calculation
        let heights = self.cached_cell_heights.iter().map(|&h| h as u32);
        self.layout = ColumnLayout::calculate_from_heights(
            heights,
            self.output_size.h as u32,
            self.scroll_offset,
        );

        // Update external window positions in Space for click detection
        self.update_space_positions();
    }

    /// Update Space element positions for external windows
    /// This ensures Smithay's click detection matches actual rendered positions
    pub fn update_space_positions(&mut self) {
        // Calculate render_y for each cell (with Y-flip for OpenGL)
        let screen_height = self.output_size.h;
        let mut content_y: i32 = -(self.scroll_offset as i32);

        for (i, cell) in self.cells.iter().enumerate() {
            let height = self.get_cell_height(i).unwrap_or(200);

            // Only external windows need to be mapped in Space
            if let ColumnCell::External(entry) = cell {
                // Apply Y-flip: render_y = screen_height - content_y - height
                let render_y = screen_height - content_y - height;
                let loc = Point::from((0, render_y));
                self.space.map_element(entry.window.clone(), loc, false);

                tracing::trace!(
                    index = i,
                    content_y,
                    render_y,
                    height,
                    scroll = self.scroll_offset,
                    "update_space_positions: external window"
                );
            }

            content_y += height;
        }
    }

    /// Add a new external window at the focused position
    pub fn add_window(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new_wayland_window(toplevel.clone());

        // Consume pending output terminal (if any)
        let output_terminal = self.pending_window_output_terminal.take();

        // Consume pending command for title bar (if any)
        let command = self.pending_window_command.take().unwrap_or_default();

        // Default initial height (will be resized based on content)
        let initial_height = 200u32;

        let entry = WindowEntry {
            toplevel,
            window: window.clone(),
            state: WindowState::Active {
                height: initial_height,
            },
            output_terminal,
            command: command.clone(),
        };

        // If we have an output terminal, remove it from the cells list
        // (it will be promoted back when it has output)
        if let Some(term_id) = output_terminal {
            self.cells.retain(|cell| {
                !matches!(cell, ColumnCell::Terminal(id) if *id == term_id)
            });
            tracing::info!(
                terminal_id = term_id.0,
                "output terminal removed from cells (hidden until output)"
            );
        }

        // Insert AT focused index (appears above/before it on screen since lower index = higher Y)
        let insert_index = self.focused_index.unwrap_or(self.cells.len());
        self.cells.insert(insert_index, ColumnCell::External(entry));

        // Keep focus on the previously focused cell (which moved down by 1)
        // If nothing was focused, focus the new cell
        self.focused_index = Some(self.focused_index.map(|idx| idx + 1).unwrap_or(insert_index));

        // Signal main loop to scroll to show this new window
        self.new_external_window_index = Some(insert_index);

        self.recalculate_layout();

        tracing::info!(
            cell_count = self.cells.len(),
            focused = ?self.focused_index,
            insert_index,
            has_output_terminal = output_terminal.is_some(),
            command = %command,
            "external window added"
        );
    }

    /// Add a new terminal above the focused position
    pub fn add_terminal(&mut self, id: TerminalId) {
        // Insert at focused index to appear ABOVE the focused cell
        // (lower index = higher on screen after Y-flip)
        let insert_index = self.focused_index.unwrap_or(self.cells.len());

        self.cells.insert(insert_index, ColumnCell::Terminal(id));

        // Also insert placeholder into cached_cell_heights to keep indices aligned
        // Using 0 ensures the height calculation will use terminal.height instead of a stale value
        if insert_index <= self.cached_cell_heights.len() {
            self.cached_cell_heights.insert(insert_index, 0);
        }

        // Keep focus on the previously focused cell (which moved down by 1)
        // If nothing was focused, focus the new cell
        self.focused_index = Some(self.focused_index.map(|idx| idx + 1).unwrap_or(insert_index));

        self.recalculate_layout();

        tracing::info!(
            terminal_id = id.0,
            insert_index,
            cell_count = self.cells.len(),
            "terminal added"
        );
    }

    /// Remove an external window by its surface
    /// If the window had an output terminal, it's added to pending_output_terminal_cleanup
    pub fn remove_window(&mut self, surface: &WlSurface) {
        if let Some(index) = self.cells.iter().position(|cell| {
            matches!(cell, ColumnCell::External(entry) if entry.toplevel.wl_surface() == surface)
        }) {
            let output_terminal = if let ColumnCell::External(entry) = self.cells.remove(index) {
                self.space.unmap_elem(&entry.window);
                entry.output_terminal
            } else {
                None
            };

            // Queue output terminal for cleanup in main loop
            if let Some(term_id) = output_terminal {
                tracing::info!(
                    terminal_id = term_id.0,
                    "window closed, queuing output terminal for cleanup"
                );
                self.pending_output_terminal_cleanup.push(term_id);
            }

            self.update_focus_after_removal(index);

            self.recalculate_layout();

            tracing::info!(
                cell_count = self.cells.len(),
                focused = ?self.focused_index,
                has_output_terminal = output_terminal.is_some(),
                "external window removed"
            );
        }
    }

    /// Remove a terminal by its ID
    pub fn remove_terminal(&mut self, id: TerminalId) {
        if let Some(index) = self.cells.iter().position(|cell| {
            matches!(cell, ColumnCell::Terminal(tid) if *tid == id)
        }) {
            self.cells.remove(index);
            self.update_focus_after_removal(index);
            self.recalculate_layout();

            tracing::info!(
                cell_count = self.cells.len(),
                focused = ?self.focused_index,
                terminal_id = ?id,
                "terminal removed"
            );
        }
    }

    /// Update focus after removing a cell at the given index
    fn update_focus_after_removal(&mut self, removed_index: usize) {
        if self.cells.is_empty() {
            self.focused_index = None;
        } else if let Some(focused) = self.focused_index {
            if focused >= self.cells.len() {
                self.focused_index = Some(self.cells.len() - 1);
            } else if focused > removed_index {
                self.focused_index = Some(focused - 1);
            }
        }
    }

    /// Request an external window resize (by cell index)
    pub fn request_resize(&mut self, index: usize, new_height: u32) {
        let Some(ColumnCell::External(entry)) = self.cells.get_mut(index) else {
            return;
        };

        let current = entry.state.current_height();
        if current == new_height {
            return;
        }

        // Get output width
        let width = self.output_size.w as u32;

        // Request the resize
        entry.toplevel.with_pending_state(|state| {
            state.size = Some(Size::from((width as i32, new_height as i32)));
        });

        entry.toplevel.send_configure();
        let serial = SERIAL_COUNTER.next_serial().into();

        entry.state = WindowState::PendingResize {
            current_height: current,
            requested_height: new_height,
            request_serial: serial,
        };

        tracing::debug!(
            index,
            current_height = current,
            requested_height = new_height,
            "resize requested"
        );
    }

    /// Handle window commit - check for resize completion
    pub fn handle_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.cells.iter().position(|cell| {
            matches!(cell, ColumnCell::External(entry) if entry.toplevel.wl_surface() == surface)
        }) else {
            return;
        };

        let Some(ColumnCell::External(entry)) = self.cells.get_mut(index) else {
            return;
        };

        // Get the committed size
        let committed_size = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .and_then(|data| data.current.size)
        });

        if let Some(size) = committed_size {
            let new_height = size.h as u32;

            match &entry.state {
                WindowState::PendingResize { requested_height, .. }
                    if new_height == *requested_height =>
                {
                    entry.state = WindowState::Active { height: new_height };
                    tracing::debug!(index, height = new_height, "resize completed");
                    self.external_window_resized = Some((index, new_height as i32));
                    self.recalculate_layout();
                }
                WindowState::AwaitingCommit { target_height, .. }
                    if new_height == *target_height =>
                {
                    entry.state = WindowState::Active { height: new_height };
                    tracing::debug!(index, height = new_height, "resize completed");
                    self.external_window_resized = Some((index, new_height as i32));
                    self.recalculate_layout();
                }
                WindowState::Active { height } if new_height != *height => {
                    entry.state = WindowState::Active { height: new_height };
                    tracing::debug!(index, height = new_height, "external window size changed");
                    self.external_window_resized = Some((index, new_height as i32));
                    self.recalculate_layout();
                }
                _ => {}
            }
        }
    }

    /// Scroll by a delta
    pub fn scroll(&mut self, delta: f64) {
        // Total content height from all cells
        let total_height: i32 = self.cached_cell_heights.iter().sum();
        let max_scroll = (total_height as f64 - self.output_size.h as f64).max(0.0);
        self.scroll_offset = (self.scroll_offset + delta).clamp(0.0, max_scroll);
        self.recalculate_layout();
    }

    /// Focus previous cell
    pub fn focus_prev(&mut self) {
        if let Some(current) = self.focused_index {
            if current > 0 {
                self.focused_index = Some(current - 1);
                self.ensure_focused_visible();
            }
        }
    }

    /// Focus next cell
    pub fn focus_next(&mut self) {
        if let Some(current) = self.focused_index {
            if current + 1 < self.cells.len() {
                self.focused_index = Some(current + 1);
                self.ensure_focused_visible();
            }
        }
    }

    /// Update cached cell heights from actual render heights
    /// Called at the start of each frame with heights from rendering
    pub fn update_cached_cell_heights(&mut self, heights: Vec<i32>) {
        self.cached_cell_heights = heights;
    }

    /// Get the height of a cell at the given index
    pub fn get_cell_height(&self, index: usize) -> Option<i32> {
        self.cached_cell_heights.get(index).copied()
    }

    /// Scroll to ensure a cell's bottom edge is visible on screen.
    /// Returns the new scroll offset if it changed, None otherwise.
    pub fn scroll_to_show_cell_bottom(&mut self, cell_index: usize) -> Option<f64> {
        let y: i32 = self.cached_cell_heights.iter().take(cell_index).sum();
        let height = self.get_cell_height(cell_index).unwrap_or(200);
        let bottom_y = y + height;
        let visible_height = self.output_size.h;
        let total_height: i32 = self.cached_cell_heights.iter().sum();
        let max_scroll = (total_height - visible_height).max(0) as f64;
        let min_scroll_for_bottom = (bottom_y - visible_height).max(0) as f64;
        let new_scroll = min_scroll_for_bottom.min(max_scroll);

        if (new_scroll - self.scroll_offset).abs() > 0.5 {
            self.scroll_offset = new_scroll;
            Some(new_scroll)
        } else {
            None
        }
    }

    /// Ensure the focused cell is visible
    fn ensure_focused_visible(&mut self) {
        if let Some(index) = self.focused_index {
            self.scroll_to_show_cell_bottom(index);
        }
    }

    /// Check if the focused cell is a terminal
    pub fn is_terminal_focused(&self) -> bool {
        self.focused_index
            .and_then(|i| self.cells.get(i))
            .map(|cell| matches!(cell, ColumnCell::Terminal(_)))
            .unwrap_or(false)
    }

    /// Check if the focused cell is an external window
    pub fn is_external_focused(&self) -> bool {
        self.focused_index
            .and_then(|i| self.cells.get(i))
            .map(|cell| matches!(cell, ColumnCell::External(_)))
            .unwrap_or(false)
    }

    /// Get the focused terminal ID, if any
    pub fn focused_terminal(&self) -> Option<TerminalId> {
        self.focused_index
            .and_then(|i| self.cells.get(i))
            .and_then(|cell| match cell {
                ColumnCell::Terminal(id) => Some(*id),
                ColumnCell::External(_) => None,
            })
    }

    /// Get the cell under a point
    ///
    /// The point must be in render coordinates (Y=0 at bottom).
    /// Returns the cell index if found.
    ///
    /// This uses our own coordinate calculation (not Smithay's Space.element_under)
    /// to ensure consistent behavior with Y-flip coordinates.
    pub fn cell_at(&self, point: Point<f64, smithay::utils::Logical>) -> Option<usize> {
        let screen_height = self.output_size.h as f64;
        let mut content_y = -self.scroll_offset;

        for (i, &height) in self.cached_cell_heights.iter().enumerate() {
            let cell_height = height as f64;

            // Calculate render Y for this cell (same formula as main.rs rendering)
            // render_y = screen_height - content_y - height
            let render_y = screen_height - content_y - cell_height;
            let render_end = render_y + cell_height;

            if point.y >= render_y && point.y < render_end {
                tracing::debug!(
                    index = i,
                    point = ?(point.x, point.y),
                    render_y,
                    render_end,
                    content_y,
                    "cell_at: hit"
                );
                return Some(i);
            }
            content_y += cell_height;
        }
        None
    }

    /// Get the external window cell under a point (returns None for terminals)
    /// This is for compatibility with existing code that only cares about external windows
    pub fn window_at(&self, point: Point<f64, smithay::utils::Logical>) -> Option<usize> {
        self.cell_at(point).filter(|&i| {
            matches!(self.cells.get(i), Some(ColumnCell::External(_)))
        })
    }

    /// Check if a point is on a terminal cell
    pub fn is_on_terminal(&self, point: Point<f64, smithay::utils::Logical>) -> bool {
        self.cell_at(point)
            .map(|i| matches!(self.cells.get(i), Some(ColumnCell::Terminal(_))))
            .unwrap_or(false)
    }
}

// Wayland protocol implementations

impl CompositorHandler for ColumnCompositor {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a smithay::reexports::wayland_server::Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        // Process buffer for desktop rendering abstractions
        on_commit_buffer_handler::<Self>(surface);
        // Handle toplevel commits
        self.handle_commit(surface);
    }
}

impl BufferHandler for ColumnCompositor {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for ColumnCompositor {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl XdgShellHandler for ColumnCompositor {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::info!("XDG toplevel created");

        // Configure the surface with initial size
        let size = Size::from((self.output_size.w, 200));
        surface.with_pending_state(|state| {
            state.size = Some(size);
        });
        surface.send_configure();

        self.add_window(surface);
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {
        // Popups not yet supported
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: smithay::utils::Serial) {
        // Popup grabs not yet supported
    }

    fn reposition_request(&mut self, _surface: PopupSurface, _positioner: PositionerState, _token: u32) {
        // Repositioning not yet supported
    }
}

impl SeatHandler for ColumnCompositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&Self::KeyboardFocus>) {
        // Focus change handling
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: smithay::input::pointer::CursorImageStatus) {
        // Cursor handling
    }
}

impl SelectionHandler for ColumnCompositor {
    type SelectionUserData = ();
}

impl DataDeviceHandler for ColumnCompositor {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for ColumnCompositor {}
impl ServerDndGrabHandler for ColumnCompositor {}
impl OutputHandler for ColumnCompositor {}

/// Client state for tracking Wayland client resources
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

// Delegate macros
delegate_compositor!(ColumnCompositor);
delegate_shm!(ColumnCompositor);
delegate_xdg_shell!(ColumnCompositor);
delegate_seat!(ColumnCompositor);
delegate_data_device!(ColumnCompositor);
delegate_output!(ColumnCompositor);

#[cfg(test)]
mod tests {

    /// Test data for positioning - simulates the state needed for cell_at() calculations
    /// Uses the same Y-flip coordinate system as the actual implementation.
    ///
    /// Coordinate system:
    /// - Render coords: Y=0 at BOTTOM (OpenGL convention)
    /// - Cell 0 appears at TOP of screen (high render Y)
    /// - Cell N appears at BOTTOM of screen (low render Y)
    struct MockPositioning {
        screen_height: i32,
        scroll_offset: f64,
        cached_cell_heights: Vec<i32>,
    }

    impl MockPositioning {
        /// Replicate the cell_at() logic for testing
        /// This is the exact same formula as in ColumnCompositor::cell_at()
        fn cell_at(&self, y: f64) -> Option<usize> {
            let screen_height = self.screen_height as f64;
            let mut content_y = -self.scroll_offset;

            for (i, &height) in self.cached_cell_heights.iter().enumerate() {
                let cell_height = height as f64;

                // Y-flip formula: render_y = screen_height - content_y - height
                let render_y = screen_height - content_y - cell_height;
                let render_end = render_y + cell_height;

                if y >= render_y && y < render_end {
                    return Some(i);
                }
                content_y += cell_height;
            }
            None
        }

        /// Get render positions (render_y, height) for each cell
        /// Uses Y-flip: render_y = screen_height - content_y - height
        fn render_positions(&self) -> Vec<(i32, i32)> {
            let screen_height = self.screen_height;
            let mut content_y = -(self.scroll_offset as i32);

            self.cached_cell_heights
                .iter()
                .map(|&height| {
                    let render_y = screen_height - content_y - height;
                    content_y += height;
                    (render_y, height)
                })
                .collect()
        }

        /// Get the render Y range for each cell (render_start, render_end)
        fn cell_ranges(&self) -> Vec<(f64, f64)> {
            let screen_height = self.screen_height as f64;
            let mut content_y = -self.scroll_offset;

            self.cached_cell_heights
                .iter()
                .map(|&height| {
                    let cell_height = height as f64;
                    let render_y = screen_height - content_y - cell_height;
                    let render_end = render_y + cell_height;
                    content_y += cell_height;
                    (render_y, render_end)
                })
                .collect()
        }
    }

    #[test]
    fn test_cell_at_single_cell() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            cached_cell_heights: vec![200],
        };

        // With Y-flip: cell 0 (height 200) at content_y=0
        // render_y = 600 - 0 - 200 = 400
        // render_end = 400 + 200 = 600
        // So cell 0 is at render Y 400-600 (TOP of screen in render coords)

        assert_eq!(pos.cell_at(300.0), None, "render Y=300 below cell 0");
        assert_eq!(pos.cell_at(400.0), Some(0), "render Y=400 at cell 0 start");
        assert_eq!(pos.cell_at(500.0), Some(0), "render Y=500 inside cell 0");
        assert_eq!(pos.cell_at(599.0), Some(0), "render Y=599 inside cell 0");
        assert_eq!(pos.cell_at(600.0), None, "render Y=600 is past screen top");
    }

    #[test]
    fn test_cell_at_two_cells_no_overlap() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            cached_cell_heights: vec![200, 300],
        };

        // Cell 0: content_y=0, height=200
        //   render_y = 600 - 0 - 200 = 400, render_end = 600
        // Cell 1: content_y=200, height=300
        //   render_y = 600 - 200 - 300 = 100, render_end = 400

        let ranges = pos.cell_ranges();
        assert_eq!(ranges[0], (400.0, 600.0), "cell 0 at top (high render Y)");
        assert_eq!(ranges[1], (100.0, 400.0), "cell 1 below (lower render Y)");

        // Cell 0's bottom (400) equals cell 1's top (400) - no overlap
        assert_eq!(ranges[0].0, ranges[1].1, "cells should be adjacent");

        // Test click detection
        assert_eq!(pos.cell_at(500.0), Some(0), "render Y=500 hits cell 0");
        assert_eq!(pos.cell_at(400.0), Some(0), "render Y=400 at boundary hits cell 0");
        assert_eq!(pos.cell_at(399.0), Some(1), "render Y=399 hits cell 1");
        assert_eq!(pos.cell_at(200.0), Some(1), "render Y=200 hits cell 1");
        assert_eq!(pos.cell_at(100.0), Some(1), "render Y=100 at cell 1 start");
        assert_eq!(pos.cell_at(99.0), None, "render Y=99 below all cells");
    }

    #[test]
    fn test_render_positions_match_click_detection() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            cached_cell_heights: vec![150, 200, 100],
        };

        let render_pos = pos.render_positions();
        let click_ranges = pos.cell_ranges();

        // Verify render Y matches click detection range
        for (i, ((render_y, render_h), (click_start, click_end))) in
            render_pos.iter().zip(click_ranges.iter()).enumerate()
        {
            assert_eq!(
                *render_y as f64, *click_start,
                "cell {} render Y ({}) should match click start ({})",
                i, render_y, click_start
            );
            assert_eq!(
                *render_h as f64, click_end - click_start,
                "cell {} render height should match click range",
                i
            );
        }
    }

    #[test]
    fn test_cell_positions_with_scroll() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 50.0,
            cached_cell_heights: vec![200, 300],
        };

        // With scroll=50:
        // Cell 0: content_y=-50, height=200
        //   render_y = 600 - (-50) - 200 = 450, render_end = 650 (partially off screen)
        // Cell 1: content_y=150, height=300
        //   render_y = 600 - 150 - 300 = 150, render_end = 450

        let ranges = pos.cell_ranges();
        assert_eq!(ranges[0], (450.0, 650.0), "cell 0 scrolled up (higher render Y)");
        assert_eq!(ranges[1], (150.0, 450.0), "cell 1 scrolled up");

        // Cell 0's bottom (450) equals cell 1's top (450)
        assert_eq!(ranges[0].0, ranges[1].1, "cells should remain adjacent after scroll");
    }

    #[test]
    fn test_cell_order_matches_visual_top_to_bottom() {
        // Cell 0 should be at TOP of screen (highest render Y)
        // Cell N should be at BOTTOM of screen (lowest render Y)
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            cached_cell_heights: vec![100, 200, 150],
        };

        // Total height = 100 + 200 + 150 = 450
        // Cell 0: content_y=0,   height=100 → render_y=500, render_end=600
        // Cell 1: content_y=100, height=200 → render_y=300, render_end=500
        // Cell 2: content_y=300, height=150 → render_y=150, render_end=300

        let ranges = pos.cell_ranges();
        assert_eq!(ranges[0], (500.0, 600.0), "cell 0");
        assert_eq!(ranges[1], (300.0, 500.0), "cell 1");
        assert_eq!(ranges[2], (150.0, 300.0), "cell 2");

        // Cell 0 at top, cell 1 below, cell 2 at bottom
        assert!(ranges[0].0 > ranges[1].0, "cell 0 should be higher than cell 1");
        assert!(ranges[1].0 > ranges[2].0, "cell 1 should be higher than cell 2");

        // Clicking at HIGH render Y (top of screen) should hit cell 0
        assert_eq!(pos.cell_at(550.0), Some(0), "high render Y (top of screen) hits cell 0");

        // Clicking at LOW render Y (bottom of screen) should hit cell 2
        assert_eq!(pos.cell_at(200.0), Some(2), "low render Y (bottom of screen) hits cell 2");

        // Below cell 2 should hit nothing
        assert_eq!(pos.cell_at(149.0), None, "below all cells hits nothing");
    }

    #[test]
    fn test_cells_stack_vertically_not_overlap() {
        let heights = vec![200, 300, 150];
        let pos = MockPositioning {
            screen_height: 800,
            scroll_offset: 0.0,
            cached_cell_heights: heights.clone(),
        };

        let ranges = pos.cell_ranges();

        // Each cell's render_y (bottom) should equal the next cell's render_end (top)
        for i in 0..ranges.len() - 1 {
            assert_eq!(
                ranges[i].0, ranges[i + 1].1,
                "cell {} bottom ({}) should equal cell {} top ({})",
                i, ranges[i].0, i + 1, ranges[i + 1].1
            );
        }

        // First cell's top should be at screen_height - 0 = 800... no wait,
        // with content_y=0, height=200: render_end = 800 - 0 = 800
        assert_eq!(ranges[0].1, 800.0, "first cell top should be at screen height");

        // Last cell's bottom should be at screen_height - total_content
        let total: i32 = heights.iter().sum();
        assert_eq!(ranges.last().unwrap().0, (800 - total) as f64, "last cell bottom");
    }

    #[test]
    fn test_point_in_only_one_cell() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            cached_cell_heights: vec![100, 150, 200],
        };

        // Check every pixel in the cell range
        let total_height: i32 = pos.cached_cell_heights.iter().sum();
        let bottom_y = pos.screen_height - total_height;

        for y in bottom_y..pos.screen_height {
            let result = pos.cell_at(y as f64);
            assert!(result.is_some(), "render Y={} should hit a cell", y);
            let idx = result.unwrap();
            assert!(idx < 3, "cell index should be valid");
        }

        // Below content should hit nothing
        assert_eq!(pos.cell_at((bottom_y - 1) as f64), None);
        // At/above screen top should hit nothing
        assert_eq!(pos.cell_at(600.0), None);
    }

    #[test]
    fn test_new_terminals_insert_above_focused() {
        use crate::terminal_manager::TerminalId;
        use crate::state::ColumnCell;

        // Simulate cell insertion behavior
        let mut cells: Vec<ColumnCell> = Vec::new();
        let mut focused_index: Option<usize> = None;

        // Helper to add terminal with the same logic as add_terminal
        let add_terminal = |id: u32, cells: &mut Vec<ColumnCell>, focused: &mut Option<usize>| {
            let insert_index = focused.unwrap_or(cells.len());
            cells.insert(insert_index, ColumnCell::Terminal(TerminalId(id)));
            *focused = Some(focused.map(|idx| idx + 1).unwrap_or(insert_index));
        };

        // Add first terminal - should be focused
        add_terminal(0, &mut cells, &mut focused_index);
        assert_eq!(cells.len(), 1);
        assert_eq!(focused_index, Some(0));
        assert!(matches!(cells[0], ColumnCell::Terminal(TerminalId(0))));

        // Add second terminal - should appear above T0, focus stays on T0
        add_terminal(1, &mut cells, &mut focused_index);
        assert_eq!(cells.len(), 2);
        assert_eq!(focused_index, Some(1), "focus should move to index 1 (still T0)");
        assert!(matches!(cells[0], ColumnCell::Terminal(TerminalId(1))), "T1 should be at index 0 (top)");
        assert!(matches!(cells[1], ColumnCell::Terminal(TerminalId(0))), "T0 should be at index 1");

        // Add third terminal - should appear above T0 (at index 1), focus stays on T0
        add_terminal(2, &mut cells, &mut focused_index);
        assert_eq!(cells.len(), 3);
        assert_eq!(focused_index, Some(2), "focus should move to index 2 (still T0)");
        assert!(matches!(cells[0], ColumnCell::Terminal(TerminalId(1))), "T1 should be at index 0");
        assert!(matches!(cells[1], ColumnCell::Terminal(TerminalId(2))), "T2 should be at index 1");
        assert!(matches!(cells[2], ColumnCell::Terminal(TerminalId(0))), "T0 should be at index 2 (bottom)");
    }

    #[test]
    fn test_add_terminal_syncs_cached_heights() {
        // Tests that add_terminal inserts a placeholder into cached_cell_heights
        // to keep indices aligned with cells
        //
        // This simulates the exact logic used in add_terminal:
        // 1. Insert new cell at insert_index
        // 2. Insert 0 into cached_cell_heights at same index

        use crate::terminal_manager::TerminalId;
        use super::ColumnCell;

        // Simulate initial state: one terminal with cached height 51
        let mut cells: Vec<ColumnCell> = vec![ColumnCell::Terminal(TerminalId(0))];
        let mut cached_cell_heights: Vec<i32> = vec![51];
        let mut focused_index: Option<usize> = Some(0);

        // Simulate add_terminal logic
        let insert_index = focused_index.unwrap_or(cells.len());
        cells.insert(insert_index, ColumnCell::Terminal(TerminalId(1)));

        // This is the fix: insert 0 into cached heights to keep indices aligned
        if insert_index <= cached_cell_heights.len() {
            cached_cell_heights.insert(insert_index, 0);
        }

        focused_index = Some(focused_index.map(|idx| idx + 1).unwrap_or(insert_index));

        // After add_terminal:
        // - cells should be [Terminal(1), Terminal(0)]
        // - cached_cell_heights should be [0, 51]
        // The 0 is a placeholder for the new terminal, old height (51) shifted to index 1

        assert_eq!(cells.len(), 2);
        assert!(matches!(cells[0], ColumnCell::Terminal(TerminalId(1))), "new terminal at index 0");
        assert!(matches!(cells[1], ColumnCell::Terminal(TerminalId(0))), "old terminal at index 1");

        assert_eq!(cached_cell_heights.len(), 2,
            "cached_cell_heights should grow with cells");
        assert_eq!(cached_cell_heights[0], 0,
            "new terminal should have placeholder height 0");
        assert_eq!(cached_cell_heights[1], 51,
            "old terminal's height should shift to index 1");

        assert_eq!(focused_index, Some(1), "focus should stay on old terminal");

        // Now simulate the height recalculation that happens after add_terminal in main.rs
        // The key is: for the new terminal, we should NOT use cached_cell_heights[0]
        // because it's 0 (placeholder), not a real cached height

        // This mimics the logic in main.rs:
        let new_terminal_height = 714; // Full TUI height
        let old_terminal_hidden = true; // Parent is hidden

        let new_heights: Vec<i32> = cells.iter().enumerate().map(|(i, cell)| {
            match cell {
                ColumnCell::Terminal(tid) => {
                    // Check if hidden first
                    let is_hidden = if tid.0 == 0 { old_terminal_hidden } else { false };
                    if is_hidden {
                        return 0;
                    }
                    // Use cached if available AND > 0
                    if let Some(&cached) = cached_cell_heights.get(i) {
                        if cached > 0 {
                            return cached;
                        }
                    }
                    // For new terminals (cached=0), use actual terminal.height
                    if tid.0 == 1 { new_terminal_height } else { 51 }
                }
                _ => 200,
            }
        }).collect();

        assert_eq!(new_heights[0], 714, "new TUI terminal should use full height, not stale cached value");
        assert_eq!(new_heights[1], 0, "old terminal is hidden, should be 0");
    }
}
