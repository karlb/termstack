//! Compositor state machine
//!
//! This module contains the main compositor state, implementing explicit state
//! tracking to prevent the bugs encountered in v1.

use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_text_input_manager;
use smithay::delegate_xdg_decoration;
use smithay::delegate_xdg_shell;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::output::OutputHandler;
use smithay::desktop::{PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, PopupUngrabStrategy, Space, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Physical, Point, Rectangle, Size, SERIAL_COUNTER};
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
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState;
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::text_input::{TextInputManagerState, TextInputSeat};

use std::os::unix::net::UnixStream;

use crate::ipc::{ResizeMode, SpawnRequest};
use crate::layout::ColumnLayout;
use crate::terminal_manager::TerminalId;

/// Active resize drag state
pub struct ResizeDrag {
    /// Index of the cell being resized
    pub cell_index: usize,
    /// Initial pointer Y in screen coordinates (Y=0 at top)
    pub start_screen_y: i32,
    /// Cell height when drag started
    pub start_height: i32,
}

/// Size of the resize handle zone at cell borders (pixels)
pub const RESIZE_HANDLE_SIZE: i32 = 8;

/// Minimum cell height (pixels)
pub const MIN_CELL_HEIGHT: i32 = 50;

/// Main compositor state
pub struct ColumnCompositor {
    /// Wayland display handle
    pub display_handle: DisplayHandle,

    /// Event loop handle
    pub loop_handle: LoopHandle<'static, Self>,

    /// Wayland protocol state
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub text_input_state: TextInputManagerState,

    /// Desktop space for managing external windows
    pub space: Space<Window>,

    /// Popup manager for tracking XDG popups
    pub popup_manager: PopupManager,

    /// All cells in column order (terminals and external windows unified)
    pub layout_nodes: Vec<LayoutNode>,

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

    /// Host clipboard access (None if unavailable)
    pub clipboard: Option<arboard::Clipboard>,

    /// Pending paste request (set by keybinding, handled in input loop)
    pub pending_paste: bool,

    /// Pending copy request (set by keybinding, handled in input loop)
    pub pending_copy: bool,

    /// Active selection state: (terminal_id, terminal_y_offset, terminal_height)
    /// Set when mouse button is pressed on a terminal, cleared on release
    pub selecting: Option<(TerminalId, i32, i32)>,

    /// Active resize drag state
    /// Set when mouse button is pressed on a resize handle, cleared on release
    pub resizing: Option<ResizeDrag>,

    /// Key repeat state for terminals: (bytes to send, next repeat instant)
    /// Set on key press, cleared on key release
    pub key_repeat: Option<(Vec<u8>, std::time::Instant)>,

    /// Key repeat delay in milliseconds (before repeat starts)
    pub repeat_delay_ms: u64,

    /// Key repeat interval in milliseconds (between repeat events)
    pub repeat_interval_ms: u64,

    /// Last known pointer position in render coordinates (Y=0 at bottom)
    /// Used for Shift+Scroll to scroll terminal under pointer
    pub pointer_position: Point<f64, smithay::utils::Logical>,

    /// Whether the cursor should show a resize icon
    /// Set by input handling when pointer is over a resize handle
    pub cursor_on_resize_handle: bool,

    /// Pending X11 resize event (new width, height)
    /// Set by X11 backend callback, processed in main loop
    pub x11_resize_pending: Option<(u16, u16)>,
}

/// A node in the column layout containing the cell and its cached height.
///
/// # Height Consistency (Critical!)
///
/// The `height` field is cached from the previous frame's *actual rendered height*,
/// not from querying the cell's preferred or bbox height. This is essential because:
///
/// 1. **Click detection must use render heights**: When a user clicks, we need to know
///    which cell they hit. This calculation uses `height` from LayoutNode.
///
/// 2. **Heights must match render positions**: The render loop calculates Y positions
///    using element geometry. If click detection used different heights (e.g., from
///    `bbox()` which can differ), clicks would hit the wrong cells.
///
/// 3. **Frame-to-frame lag is acceptable**: Heights are updated at the start of each
///    frame via `update_layout_heights()`. This means click detection uses heights from
///    frame N-1, but since we also rendered with those heights in frame N-1, coordinates
///    remain consistent.
///
/// **Do not** read heights from `Terminal.bbox()` or `WindowState.current_height()` for
/// positioning calculations - always use `LayoutNode.height` which matches what was
/// actually rendered.
pub struct LayoutNode {
    pub cell: ColumnCell,
    /// Cached height from last render frame. Used for layout, click detection, and scroll.
    /// Updated by `update_layout_heights()` at the start of each frame.
    pub height: i32,
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

    /// Whether window uses client-side decorations (skip our title bar if true)
    pub uses_csd: bool,
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
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let text_input_state = TextInputManagerState::new::<Self>(&display_handle);

        let mut seat = seat_state.new_wl_seat(&display_handle, "seat0");

        // Add keyboard and pointer capabilities
        seat.add_keyboard(Default::default(), 200, 25).expect("Failed to add keyboard");
        seat.add_pointer();

        let compositor = Self {
            display_handle,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            shm_state,
            seat_state,
            data_device_state,
            text_input_state,
            space: Space::default(),
            popup_manager: PopupManager::default(),
            layout_nodes: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
            layout: ColumnLayout::empty(),
            output_size,
            seat,
            running: true,
            spawn_terminal_requested: false,
            focus_change_requested: 0,
            scroll_requested: 0.0,
            pending_spawn_requests: Vec::new(),
            pending_resize_request: None,
            new_external_window_index: None,
            external_window_resized: None,
            pending_window_output_terminal: None,
            pending_window_command: None,
            pending_output_terminal_cleanup: Vec::new(),
            clipboard: arboard::Clipboard::new().ok(),
            pending_paste: false,
            pending_copy: false,
            selecting: None,
            resizing: None,
            key_repeat: None,
            repeat_delay_ms: 400,    // Standard delay before repeat starts
            repeat_interval_ms: 30,  // ~33 keys per second
            pointer_position: Point::from((0.0, 0.0)),
            cursor_on_resize_handle: false,
            x11_resize_pending: None,
        };

        (compositor, display)
    }

    /// Recalculate layout after any change
    pub fn recalculate_layout(&mut self) {
        // Use cached heights for layout calculation
        let heights = self.layout_nodes.iter().map(|node| node.height as u32);
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

        for (i, node) in self.layout_nodes.iter().enumerate() {
            let height = node.height;

            // Only external windows need to be mapped in Space
            if let ColumnCell::External(entry) = &node.cell {
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
            uses_csd: false, // Will be set by XdgDecorationHandler if client requests CSD
        };

        // Keep the output terminal in the layout - its title bar shows the command
        // that launched this window, which is useful context for the user.
        // (Previously we removed it and only promoted back if it had output,
        // but now that we have title bars, the terminal is valuable even without output.)
        if let Some(term_id) = output_terminal {
            tracing::info!(
                terminal_id = term_id.0,
                "output terminal kept in cells (title bar shows command)"
            );
        }

        // Insert AT focused index (appears above/before it on screen since lower index = higher Y)
        let insert_index = self.focused_index.unwrap_or(self.layout_nodes.len());
        self.layout_nodes.insert(insert_index, LayoutNode {
            cell: ColumnCell::External(entry),
            height: initial_height as i32,
        });

        // Keep focus on the previously focused cell (which moved down by 1)
        // If nothing was focused, focus the new cell
        self.focused_index = Some(self.focused_index.map(|idx| idx + 1).unwrap_or(insert_index));

        // Signal main loop to scroll to show this new window
        self.new_external_window_index = Some(insert_index);

        self.recalculate_layout();

        // Activate the new window (required for GTK animations to work)
        self.activate_toplevel(insert_index);

        tracing::info!(
            cell_count = self.layout_nodes.len(),
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
        let insert_index = self.focused_index.unwrap_or(self.layout_nodes.len());

        // Insert with placeholder height 0, will be updated in next frame
        self.layout_nodes.insert(insert_index, LayoutNode {
            cell: ColumnCell::Terminal(id),
            height: 0,
        });

        // Keep focus on the previously focused cell (which moved down by 1)
        // If nothing was focused, focus the new cell
        self.focused_index = Some(self.focused_index.map(|idx| idx + 1).unwrap_or(insert_index));

        self.recalculate_layout();

        tracing::info!(
            terminal_id = id.0,
            insert_index,
            cell_count = self.layout_nodes.len(),
            "terminal added"
        );
    }

    /// Remove an external window by its surface
    /// If the window had an output terminal, it's added to pending_output_terminal_cleanup
    pub fn remove_window(&mut self, surface: &WlSurface) {
        if let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(&node.cell, ColumnCell::External(entry) if entry.toplevel.wl_surface() == surface)
        }) {
            let output_terminal = if let ColumnCell::External(entry) = &self.layout_nodes.remove(index).cell {
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
                cell_count = self.layout_nodes.len(),
                focused = ?self.focused_index,
                has_output_terminal = output_terminal.is_some(),
                "external window removed"
            );
        }
    }

    /// Remove a terminal by its ID
    pub fn remove_terminal(&mut self, id: TerminalId) {
        if let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(node.cell, ColumnCell::Terminal(tid) if tid == id)
        }) {
            self.layout_nodes.remove(index);
            self.update_focus_after_removal(index);
            self.recalculate_layout();

            tracing::info!(
                cell_count = self.layout_nodes.len(),
                focused = ?self.focused_index,
                terminal_id = ?id,
                "terminal removed"
            );
        }
    }

    /// Update focus after removing a cell at the given index
    fn update_focus_after_removal(&mut self, removed_index: usize) {
        if self.layout_nodes.is_empty() {
            self.focused_index = None;
        } else if let Some(focused) = self.focused_index {
            if focused >= self.layout_nodes.len() {
                self.focused_index = Some(self.layout_nodes.len() - 1);
            } else if focused > removed_index {
                self.focused_index = Some(focused - 1);
            }
        }
    }

    /// Request an external window resize (by cell index)
    pub fn request_resize(&mut self, index: usize, new_height: u32) {
        let Some(node) = self.layout_nodes.get_mut(index) else {
            return;
        };
        let ColumnCell::External(entry) = &mut node.cell else {
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

    /// Resize all external windows to new width (called when compositor is resized)
    pub fn resize_all_external_windows(&mut self, new_width: i32) {
        for node in &mut self.layout_nodes {
            if let ColumnCell::External(entry) = &mut node.cell {
                let current_height = entry.state.current_height();
                entry.toplevel.with_pending_state(|state| {
                    state.size = Some(Size::from((new_width, current_height as i32)));
                });
                entry.toplevel.send_configure();
            }
        }

        tracing::info!(
            new_width,
            "resized all external windows to new width"
        );
    }

    /// Handle window commit - check for resize completion
    pub fn handle_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(&node.cell, ColumnCell::External(entry) if entry.toplevel.wl_surface() == surface)
        }) else {
            return;
        };

        let Some(node) = self.layout_nodes.get_mut(index) else {
            return;
        };
        let ColumnCell::External(entry) = &mut node.cell else {
            return;
        };

        // Detect CSD apps by app_id (GTK4 apps don't use xdg-decoration protocol)
        if !entry.uses_csd {
            let app_id = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .and_then(|data| data.lock().ok())
                    .and_then(|attrs| attrs.app_id.clone())
            });

            if let Some(id) = app_id {
                let is_csd_app = id.starts_with("org.gnome.")
                    || id.starts_with("org.gtk.")
                    || id == "org.pwmt.zathura"
                    || id == "zathura";

                if is_csd_app {
                    entry.uses_csd = true;
                    tracing::info!(app_id = %id, "detected CSD app by app_id");
                }
            }
        }

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

    /// Calculate maximum scroll offset based on content height
    fn max_scroll(&self) -> f64 {
        let total_height: i32 = self.layout_nodes.iter().map(|n| n.height).sum();
        (total_height as f64 - self.output_size.h as f64).max(0.0)
    }

    /// Scroll by a delta (clamped to valid range)
    pub fn scroll(&mut self, delta: f64) {
        let max_scroll = self.max_scroll();
        self.scroll_offset = (self.scroll_offset + delta).clamp(0.0, max_scroll);
        self.recalculate_layout();
    }

    /// Scroll to the top (scroll_offset = 0)
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0.0;
        self.recalculate_layout();
    }

    /// Scroll to the bottom (scroll_offset = max)
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll();
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
            if current + 1 < self.layout_nodes.len() {
                self.focused_index = Some(current + 1);
                self.ensure_focused_visible();
            }
        }
    }

    /// Set the activated state on a toplevel window at the given index.
    /// Also clears the activated state from all other toplevels.
    /// This is required for GTK apps to run animations and handle input properly.
    pub fn activate_toplevel(&mut self, index: usize) {
        for (i, node) in self.layout_nodes.iter().enumerate() {
            if let ColumnCell::External(entry) = &node.cell {
                let should_activate = i == index;
                entry.toplevel.with_pending_state(|state| {
                    if should_activate {
                        state.states.set(ToplevelState::Activated);
                    } else {
                        state.states.unset(ToplevelState::Activated);
                    }
                });
                entry.toplevel.send_pending_configure();
            }
        }
    }

    /// Deactivate all toplevel windows (e.g., when focusing a terminal)
    pub fn deactivate_all_toplevels(&mut self) {
        for node in &self.layout_nodes {
            if let ColumnCell::External(entry) = &node.cell {
                entry.toplevel.with_pending_state(|state| {
                    state.states.unset(ToplevelState::Activated);
                });
                entry.toplevel.send_pending_configure();
            }
        }
    }

    /// Update cached cell heights from actual render heights.
    ///
    /// Called at the start of each frame with heights from the previous frame's
    /// rendering. This ensures click detection coordinates match what was actually
    /// rendered. See [`LayoutNode`] for why this consistency matters.
    ///
    /// The `heights` vector must have the same length as `layout_nodes`.
    pub fn update_layout_heights(&mut self, heights: Vec<i32>) {
        if heights.len() != self.layout_nodes.len() {
            tracing::warn!(
                expected = self.layout_nodes.len(),
                actual = heights.len(),
                "update_layout_heights length mismatch"
            );
            return;
        }

        for (node, height) in self.layout_nodes.iter_mut().zip(heights.into_iter()) {
            node.height = height;
        }
    }

    /// Get the height of a cell at the given index
    pub fn get_cell_height(&self, index: usize) -> Option<i32> {
        self.layout_nodes.get(index).map(|n| n.height)
    }

    /// Scroll to ensure a cell's bottom edge is visible on screen.
    /// Returns the new scroll offset if it changed, None otherwise.
    pub fn scroll_to_show_cell_bottom(&mut self, cell_index: usize) -> Option<f64> {
        let y: i32 = self.layout_nodes.iter().take(cell_index).map(|n| n.height).sum();
        let height = self.get_cell_height(cell_index).unwrap_or(200);
        let bottom_y = y + height;
        let visible_height = self.output_size.h;
        let total_height: i32 = self.layout_nodes.iter().map(|n| n.height).sum();
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
            .and_then(|i| self.layout_nodes.get(i))
            .map(|node| matches!(node.cell, ColumnCell::Terminal(_)))
            .unwrap_or(false)
    }

    /// Check if the focused cell is an external window
    pub fn is_external_focused(&self) -> bool {
        self.focused_index
            .and_then(|i| self.layout_nodes.get(i))
            .map(|node| matches!(node.cell, ColumnCell::External(_)))
            .unwrap_or(false)
    }

    /// Get the focused terminal ID, if any
    pub fn focused_terminal(&self) -> Option<TerminalId> {
        self.focused_index
            .and_then(|i| self.layout_nodes.get(i))
            .and_then(|node| match &node.cell {
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

        for (i, node) in self.layout_nodes.iter().enumerate() {
            let cell_height = node.height as f64;

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
            matches!(self.layout_nodes.get(i), Some(node) if matches!(node.cell, ColumnCell::External(_)))
        })
    }

    /// Check if a point is on a terminal cell
    pub fn is_on_terminal(&self, point: Point<f64, smithay::utils::Logical>) -> bool {
        self.cell_at(point)
            .map(|i| matches!(self.layout_nodes.get(i), Some(node) if matches!(node.cell, ColumnCell::Terminal(_))))
            .unwrap_or(false)
    }

    /// Get the render position (render_y, height) for a cell at the given index
    /// Returns (render_y, height) where render_y is in render coordinates (Y=0 at bottom)
    pub fn get_cell_render_position(&self, index: usize) -> (f64, i32) {
        let screen_height = self.output_size.h as f64;
        let mut content_y = -self.scroll_offset;

        for (i, node) in self.layout_nodes.iter().enumerate() {
            if i == index {
                // render_y = screen_height - content_y - height
                let render_y = screen_height - content_y - node.height as f64;
                return (render_y, node.height);
            }
            content_y += node.height as f64;
        }

        // Fallback if index out of bounds
        (0.0, 0)
    }

    /// Get the screen bounds (top_y, bottom_y) for a cell at the given index
    /// Returns (top_y, bottom_y) in screen coordinates (Y=0 at top)
    pub fn get_cell_screen_bounds(&self, index: usize) -> Option<(i32, i32)> {
        let mut content_y = -(self.scroll_offset as i32);

        for (i, node) in self.layout_nodes.iter().enumerate() {
            if i == index {
                // In screen coords: top_y = content_y, bottom_y = content_y + height
                let top_y = content_y;
                let bottom_y = content_y + node.height;
                return Some((top_y, bottom_y));
            }
            content_y += node.height;
        }
        None
    }

    /// Find if a screen Y coordinate is on a resize handle between cells
    /// Returns the index of the cell whose bottom border is being grabbed
    /// (i.e., the cell that will be resized)
    ///
    /// The resize handle is at the bottom edge of each cell (except the last).
    /// In screen coordinates (Y=0 at top): handle zone is [cell_bottom - HANDLE_SIZE/2, cell_bottom + HANDLE_SIZE/2]
    pub fn find_resize_handle_at(&self, screen_y: i32) -> Option<usize> {
        // Don't allow resizing the last cell (no border below it)
        if self.layout_nodes.len() < 2 {
            return None;
        }

        let mut content_y = -(self.scroll_offset as i32);
        let half_handle = RESIZE_HANDLE_SIZE / 2;

        for (i, node) in self.layout_nodes.iter().enumerate() {
            let bottom_y = content_y + node.height;

            // Check if screen_y is in the handle zone around this cell's bottom edge
            // But not for the last cell (nothing below to resize into)
            if i < self.layout_nodes.len() - 1
                && screen_y >= bottom_y - half_handle
                && screen_y <= bottom_y + half_handle
            {
                return Some(i);
            }

            content_y = bottom_y;
        }
        None
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
        // Update popup manager state (moves unmapped popups to mapped when committed)
        self.popup_manager.commit(surface);

        // Following Anvil's pattern: send initial configure for popups during commit
        // This is the correct time to send it, not in new_popup
        if let Some(popup) = self.popup_manager.find_popup(surface) {
            if let PopupKind::Xdg(ref xdg_popup) = popup {
                if !xdg_popup.is_initial_configure_sent() {
                    // Send the initial configure event
                    // NOTE: This should never fail as the initial configure is always allowed
                    if let Err(e) = xdg_popup.send_configure() {
                        tracing::warn!(?e, "failed to send initial popup configure");
                    } else {
                        tracing::info!(
                            surface_id = ?surface.id(),
                            "popup initial configure sent"
                        );
                    }
                }
            }
            return; // Popup handled, don't process as toplevel
        }

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

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        // Following Anvil's pattern: set geometry with constraints, then track
        // Initial configure is sent during commit, not here

        let parent_surface = surface.get_parent_surface();

        // Find the parent window's position in content coordinates
        // This is needed to properly constrain the popup to the screen
        let parent_content_y = parent_surface.as_ref().and_then(|parent| {
            self.layout_nodes.iter().enumerate().find_map(|(idx, node)| {
                if let ColumnCell::External(entry) = &node.cell {
                    if entry.toplevel.wl_surface() == parent {
                        // Calculate content Y position (sum of heights above this window)
                        let content_y: i32 = self.layout_nodes[..idx]
                            .iter()
                            .map(|n| n.height)
                            .sum();
                        return Some(content_y);
                    }
                }
                None
            })
        }).unwrap_or(0);

        // Calculate the PARENT SURFACE position on screen (accounting for scroll)
        // The positioner works in parent-surface-local coordinates
        // Content Y to Screen Y: screen_y = content_y - scroll_offset
        let parent_screen_y = (parent_content_y as f64 - self.scroll_offset).max(0.0) as i32;

        // Parent X is at the focus indicator offset
        let parent_screen_x = crate::render::FOCUS_INDICATOR_WIDTH;

        // Create target rectangle in PARENT-SURFACE-LOCAL coordinates
        // This tells the positioner where the screen edges are relative to the parent's (0,0)
        // Screen top (Y=0) is at parent-local Y = -parent_screen_y
        // Screen left (X=0) is at parent-local X = -parent_screen_x
        let target = Rectangle::new(
            Point::from((-parent_screen_x, -parent_screen_y)),
            Size::from((self.output_size.w, self.output_size.h)),
        );

        let geo = positioner.get_unconstrained_geometry(target);

        tracing::debug!(
            ?geo,
            ?target,
            "new_popup: XDG popup created"
        );

        surface.with_pending_state(|state| {
            state.geometry = geo;
            state.positioner = positioner;
        });

        // Track the popup so it can be rendered and receive input
        if let Err(e) = self.popup_manager.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(?e, "failed to track popup");
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.remove_window(surface.wl_surface());
    }

    fn popup_destroyed(&mut self, surface: PopupSurface) {
        // Log popup destruction for debugging
        let popup_id = format!("{:?}", surface.wl_surface().id());
        tracing::info!(?popup_id, "popup_destroyed: XDG popup destroyed");
        // PopupManager cleanup() called every frame will handle internal state
    }

    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: smithay::utils::Serial) {
        // Popup grabs are used for click-outside-to-dismiss behavior.
        // GTK4 requests grabs for autocomplete popups.
        //
        // IMPORTANT: We do NOT dismiss popups when grab fails!
        // Calling send_popup_done() would tell GTK the popup is dismissed,
        // but the popup surface remains visible, causing state mismatch and freezes.

        let popup_geo = surface.with_pending_state(|state| state.geometry);
        tracing::info!(
            ?popup_geo,
            parent_exists = surface.get_parent_surface().is_some(),
            "grab(): XDG popup grab requested"
        );

        // Find the root toplevel surface for this popup
        if surface.get_parent_surface().is_none() {
            tracing::warn!("grab(): popup has no parent - ignoring grab request");
            // Do NOT call send_popup_done() - just ignore the grab
            return;
        }

        // Find the keyboard focus surface (the toplevel window)
        let keyboard = self.seat.get_keyboard();
        let keyboard_focus = keyboard.as_ref().and_then(|kbd| {
            kbd.current_focus().clone()
        });
        let keyboard_grabbed = keyboard.as_ref().map(|kbd| kbd.is_grabbed()).unwrap_or(false);

        tracing::info!(
            keyboard_focus_id = ?keyboard_focus.as_ref().map(|f| f.id()),
            keyboard_grabbed,
            "grab(): current keyboard state"
        );

        let Some(focus) = keyboard_focus else {
            tracing::warn!("grab(): no keyboard focus - ignoring grab request");
            // Do NOT call send_popup_done() - just ignore the grab
            return;
        };

        // Try to set up the popup grab
        let popup_kind = PopupKind::Xdg(surface.clone());
        match self.popup_manager.grab_popup::<Self>(
            focus.clone(),
            popup_kind,
            &self.seat,
            serial,
        ) {
            Ok(mut grab) => {
                tracing::info!(
                    focus_id = ?focus.id(),
                    "grab(): popup grab ACCEPTED"
                );

                // Following the Anvil example pattern: set both keyboard and pointer grabs
                // This is the standard way to handle popup grabs in Smithay

                // Set up keyboard grab
                // First check if there's an existing grab that conflicts
                if let Some(keyboard) = self.seat.get_keyboard() {
                    if keyboard.is_grabbed()
                        && !(keyboard.has_grab(serial)
                            || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
                    {
                        tracing::warn!("grab(): conflicting keyboard grab, ungrabbing popup");
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }

                    // Set keyboard focus to the current popup in the grab stack
                    // PopupKeyboardGrab will route events appropriately
                    if let Some(current) = grab.current_grab() {
                        keyboard.set_focus(self, Some(current), serial);
                    }
                    keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
                    tracing::info!("grab(): keyboard grab set successfully");
                }

                // Set up pointer grab
                if let Some(pointer) = self.seat.get_pointer() {
                    if pointer.is_grabbed()
                        && !(pointer.has_grab(serial)
                            || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
                    {
                        tracing::warn!("grab(): conflicting pointer grab, ungrabbing popup");
                        grab.ungrab(PopupUngrabStrategy::All);
                        return;
                    }
                    pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, smithay::input::pointer::Focus::Keep);
                    tracing::info!("grab(): pointer grab set successfully");
                }

                // Log final state
                if let Some(kbd) = self.seat.get_keyboard() {
                    let focus_after = kbd.current_focus();
                    tracing::info!(
                        focus_after_id = ?focus_after.map(|f| f.id()),
                        keyboard_grabbed_after = kbd.is_grabbed(),
                        "grab(): final keyboard state after grab setup"
                    );
                }
            }
            Err(e) => {
                // Grab failed - this commonly happens if popup was already committed.
                // Do NOT call send_popup_done() - that would cause state mismatch.
                // Popup remains visible, just without click-outside-dismiss.
                tracing::warn!(?e, "grab(): popup grab denied, popup remains visible without grab");
            }
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        // GTK may request popup repositioning (e.g., when popup would go off-screen)
        // We must respond with send_repositioned() or GTK may hang

        // Find parent window position (same logic as new_popup)
        let parent_surface = surface.get_parent_surface();
        let parent_content_y = parent_surface.as_ref().and_then(|parent| {
            self.layout_nodes.iter().enumerate().find_map(|(idx, node)| {
                if let ColumnCell::External(entry) = &node.cell {
                    if entry.toplevel.wl_surface() == parent {
                        let content_y: i32 = self.layout_nodes[..idx]
                            .iter()
                            .map(|n| n.height)
                            .sum();
                        return Some(content_y);
                    }
                }
                None
            })
        }).unwrap_or(0);

        // Calculate parent surface position on screen (same as new_popup)
        let parent_screen_y = (parent_content_y as f64 - self.scroll_offset).max(0.0) as i32;
        let parent_screen_x = crate::render::FOCUS_INDICATOR_WIDTH;

        // Target in parent-surface-local coordinates
        let target = Rectangle::new(
            Point::from((-parent_screen_x, -parent_screen_y)),
            Size::from((self.output_size.w, self.output_size.h)),
        );

        let new_geo = positioner.get_unconstrained_geometry(target);

        tracing::info!(
            ?token,
            ?new_geo,
            ?target,
            "reposition_request: updating popup position"
        );

        // Update popup geometry and positioner
        surface.with_pending_state(|state| {
            state.geometry = new_geo;
            state.positioner = positioner;
        });

        // Send repositioned event to client
        surface.send_repositioned(token);

        // Send configure to apply the new geometry
        surface.send_configure().ok();
    }
}

impl SeatHandler for ColumnCompositor {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&Self::KeyboardFocus>) {
        tracing::debug!(
            focused = focused.map(|f| format!("{:?}", f.id())).unwrap_or_else(|| "None".to_string()),
            "focus_changed: keyboard focus changed"
        );

        // Update text input focus to match keyboard focus
        let text_input = seat.text_input();
        text_input.leave();
        text_input.set_focus(focused.cloned());
        text_input.enter();
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

impl XdgDecorationHandler for ColumnCompositor {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Advertise server-side decoration as preferred
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        // Get app_id to check if this app respects SSD
        let app_id = with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .and_then(|attrs| attrs.app_id.clone())
        });

        // Apps known to ignore SSD preference and always draw their own decorations
        let csd_stubborn = app_id.as_ref().map(|id| {
            id.starts_with("org.gnome.") ||  // GTK/GNOME apps with header bars
            id.starts_with("org.gtk.") ||
            id == "org.pwmt.zathura" ||      // zathura
            id == "zathura"
        }).unwrap_or(false);

        let surface = toplevel.wl_surface();
        for node in &mut self.layout_nodes {
            if let ColumnCell::External(entry) = &mut node.cell {
                if entry.toplevel.wl_surface() == surface {
                    entry.uses_csd = csd_stubborn;
                    tracing::info!(
                        requested = ?mode,
                        app_id = ?app_id,
                        uses_csd = entry.uses_csd,
                        command = %entry.command,
                        "decoration mode negotiated"
                    );
                    break;
                }
            }
        }

        // Tell stubborn apps to use CSD, others to use SSD
        let response_mode = if csd_stubborn {
            DecorationMode::ClientSide
        } else {
            DecorationMode::ServerSide
        };
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(response_mode);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        // Client unset mode preference - revert to server-side
        let surface = toplevel.wl_surface();
        for node in &mut self.layout_nodes {
            if let ColumnCell::External(entry) = &mut node.cell {
                if entry.toplevel.wl_surface() == surface {
                    entry.uses_csd = false;
                    tracing::info!(
                        command = %entry.command,
                        "decoration mode unset, reverting to SSD"
                    );
                    break;
                }
            }
        }

        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }
}

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
delegate_xdg_decoration!(ColumnCompositor);
delegate_seat!(ColumnCompositor);
delegate_data_device!(ColumnCompositor);
delegate_output!(ColumnCompositor);
delegate_text_input_manager!(ColumnCompositor);

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
        layout_heights: Vec<i32>,
    }

    impl MockPositioning {
        /// Replicate the cell_at() logic for testing
        /// This is the exact same formula as in ColumnCompositor::cell_at()
        fn cell_at(&self, y: f64) -> Option<usize> {
            let screen_height = self.screen_height as f64;
            let mut content_y = -self.scroll_offset;

            for (i, &height) in self.layout_heights.iter().enumerate() {
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

            self.layout_heights
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

            self.layout_heights
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
            layout_heights: vec![200],
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
            layout_heights: vec![200, 300],
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
            layout_heights: vec![150, 200, 100],
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
            layout_heights: vec![200, 300],
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
            layout_heights: vec![100, 200, 150],
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
            layout_heights: heights.clone(),
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
            layout_heights: vec![100, 150, 200],
        };

        // Check every pixel in the cell range
        let total_height: i32 = pos.layout_heights.iter().sum();
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
        use crate::state::{ColumnCell, LayoutNode};

        // Simulate cell insertion behavior
        let mut layout_nodes: Vec<LayoutNode> = Vec::new();
        let mut focused_index: Option<usize> = None;

        // Helper to add terminal with the same logic as add_terminal
        let add_terminal = |id: u32, nodes: &mut Vec<LayoutNode>, focused: &mut Option<usize>| {
            let insert_index = focused.unwrap_or(nodes.len());
            nodes.insert(insert_index, LayoutNode {
                cell: ColumnCell::Terminal(TerminalId(id)),
                height: 0
            });
            *focused = Some(focused.map(|idx| idx + 1).unwrap_or(insert_index));
        };

        // Add first terminal - should be focused
        add_terminal(0, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 1);
        assert_eq!(focused_index, Some(0));
        assert!(matches!(layout_nodes[0].cell, ColumnCell::Terminal(TerminalId(0))));

        // Add second terminal - should appear above T0, focus stays on T0
        add_terminal(1, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 2);
        assert_eq!(focused_index, Some(1), "focus should move to index 1 (still T0)");
        assert!(matches!(layout_nodes[0].cell, ColumnCell::Terminal(TerminalId(1))), "T1 should be at index 0 (top)");
        assert!(matches!(layout_nodes[1].cell, ColumnCell::Terminal(TerminalId(0))), "T0 should be at index 1");

        // Add third terminal - should appear above T0 (at index 1), focus stays on T0
        add_terminal(2, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 3);
        assert_eq!(focused_index, Some(2), "focus should move to index 2 (still T0)");
        assert!(matches!(layout_nodes[0].cell, ColumnCell::Terminal(TerminalId(1))), "T1 should be at index 0");
        assert!(matches!(layout_nodes[1].cell, ColumnCell::Terminal(TerminalId(2))), "T2 should be at index 1");
        assert!(matches!(layout_nodes[2].cell, ColumnCell::Terminal(TerminalId(0))), "T0 should be at index 2 (bottom)");
    }
}
