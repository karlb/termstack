//! Compositor state machine
//!
//! This module contains the main compositor state, implementing explicit state
//! tracking to prevent the bugs encountered in v1.
//!
//! # Responsibilities
//!
//! - Window lifecycle management (add/remove windows)
//! - Focus tracking and focus change handling
//! - Scroll state management
//! - External window integration (Wayland clients)
//! - Popup management and grab state
//! - Wayland protocol handler implementations
//!
//! # NOT Responsible For
//!
//! - Layout calculation (see `layout.rs` - pure functions)
//! - Coordinate system conversions (see `coords.rs` - type-safe wrappers)
//! - Rendering operations (see `render.rs` - rendering helpers)
//! - Terminal content management (see `terminal_manager/` - terminal lifecycle)
//! - Input event handling (see `input.rs` - keyboard/pointer events)

mod core;
mod external;
mod focus;
mod resize;
#[cfg(test)]
mod initial_size_test;

use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_output;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_text_input_manager;
use smithay::delegate_viewporter;
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
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler,
    XdgShellState,
};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::text_input::{TextInputManagerState, TextInputSeat};

use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::Instant;

use std::collections::HashMap;
use crate::ipc::{BuiltinRequest, ResizeMode, SpawnRequest};
use crate::layout::ColumnLayout;
use crate::terminal_manager::TerminalId;

/// Identifies a focused cell by its content identity, not position.
///
/// Unlike indices, cell identity remains stable when cells are added/removed,
/// preventing focus from accidentally sliding to a different cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusedWindow {
    /// A terminal cell, identified by its TerminalId
    Terminal(TerminalId),
    /// An external window, identified by its surface ObjectId
    External(smithay::reexports::wayland_server::backend::ObjectId),
}

/// Active resize drag state
pub struct ResizeDrag {
    /// Index of the cell being resized
    pub window_index: usize,
    /// Initial pointer Y in screen coordinates (Y=0 at top)
    pub start_screen_y: i32,
    /// Cell height when drag started
    pub start_height: i32,
    /// Last time we sent a configure request (for throttling)
    pub last_configure_time: std::time::Instant,
    /// Current drag target height (may differ from committed height)
    pub target_height: i32,
    /// Last height we sent in configure (for deduplication)
    pub last_sent_height: Option<u32>,
}

/// Maximum time to wait for a window commit before sending next configure (milliseconds)
/// If window doesn't commit within this timeout, we send another configure anyway
/// This balances responsiveness (avoid >1s lag) with preventing window thrashing
pub const CONFIGURE_COMMIT_TIMEOUT_MS: u64 = 100;

/// Minimum cell height (pixels)
pub const MIN_WINDOW_HEIGHT: i32 = 50;

/// xwayland-satellite process monitor with crash tracking
pub struct XWaylandSatelliteMonitor {
    /// The xwayland-satellite process handle
    pub child: std::process::Child,
    /// Timestamp of last crash (for detecting rapid crash loops)
    pub last_crash_time: Option<Instant>,
    /// Number of rapid crashes (within 10s window)
    pub crash_count: u32,
}

/// Main compositor state
pub struct TermStack {
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
    pub viewporter_state: smithay::wayland::viewporter::ViewporterState,

    /// Desktop space for managing external windows
    pub space: Space<Window>,

    /// Popup manager for tracking XDG popups
    pub popup_manager: PopupManager,

    /// All cells in column order (terminals and external windows unified)
    ///
    /// INVARIANT: layout_nodes[0] renders at highest Y (top of screen after Y-flip).
    /// After any mutation (insert/remove), must call recalculate_layout() to update positions.
    pub layout_nodes: Vec<LayoutNode>,

    /// Current scroll offset (pixels from top)
    pub scroll_offset: f64,

    /// Identity of focused cell (stable across cell additions/removals)
    pub focused_window: Option<FocusedWindow>,

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

    /// Accumulated scroll delta from input events (applied once per frame to avoid repeated layout recalc)
    pub pending_scroll_delta: f64,

    /// Pending terminal spawn requests from IPC (termstack commands with foreground=None)
    pub pending_spawn_requests: Vec<SpawnRequest>,

    /// Pending GUI spawn requests from IPC (termstack commands with foreground=Some(_))
    pub pending_gui_spawn_requests: Vec<SpawnRequest>,

    /// Pending builtin command notifications from IPC (termstack --builtin)
    pub pending_builtin_requests: Vec<BuiltinRequest>,

    /// Pending resize request from IPC (termstack resize)
    /// Includes the stream for sending acknowledgement after resize completes
    pub pending_resize_request: Option<(ResizeMode, UnixStream)>,

    /// Index of newly added external window (for scroll-to-show)
    pub new_external_window_index: Option<usize>,

    /// Whether the new external window needs keyboard focus (for foreground GUI)
    pub new_window_needs_keyboard_focus: bool,

    /// Index and new height of resized external window (for scroll adjustment)
    pub external_window_resized: Option<(usize, i32)>,

    /// Pending output terminal to link with the next external window
    /// Set when spawning a GUI app command, consumed by add_window()
    pub pending_window_output_terminal: Option<TerminalId>,

    /// Pending command string for the next external window's title bar
    /// Set when spawning a GUI app command, consumed by add_window()
    pub pending_window_command: Option<String>,

    /// Whether the next window should be treated as a foreground GUI
    /// Set when processing a gui_spawn request, consumed by add_window()
    pub pending_gui_foreground: bool,

    /// Maps output_terminal_id -> (launching_terminal_id, window_was_linked)
    /// For restoring launcher when GUI exits. The bool tracks whether a window
    /// was ever linked to this output terminal.
    pub foreground_gui_sessions: HashMap<TerminalId, (TerminalId, bool)>,

    /// Output terminals from closed windows that need cleanup
    /// Processed in main loop - if terminal has no content, remove it; otherwise keep visible
    pub pending_output_terminal_cleanup: Vec<TerminalId>,

    /// Host clipboard access (None if unavailable)
    /// Note: Clipboard operations can block for several seconds waiting for the
    /// clipboard owner to respond, so paste operations are done asynchronously
    /// via clipboard_receiver.
    pub clipboard: Option<arboard::Clipboard>,

    /// Receiver for async clipboard read results.
    /// When pending_paste is triggered, a background thread reads the clipboard
    /// and sends the result here to avoid blocking the compositor.
    pub clipboard_receiver: Option<mpsc::Receiver<String>>,

    /// Pending paste request (set by keybinding, triggers async clipboard read)
    pub pending_paste: bool,

    /// Pending copy request (set by keybinding, handled in input loop)
    pub pending_copy: bool,

    /// Receiver for async PRIMARY selection read results (middle-click paste).
    pub primary_selection_receiver: Option<mpsc::Receiver<String>>,

    /// Active selection state: (terminal_id, window_render_y, window_height, last_col, last_row, last_update_time)
    /// Set when mouse button is pressed on a terminal, cleared on release
    /// Tracks last grid coordinates and update time to throttle motion events
    pub selecting: Option<(TerminalId, i32, i32, usize, usize, std::time::Instant)>,

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

    /// Number of pointer buttons currently pressed
    /// Used to detect and clear stale drag state when focus is lost
    pub pointer_buttons_pressed: u32,

    /// Pending compositor window resize event (new width, height)
    /// Set when the compositor's own window is resized (X11Event::Resized), processed in main loop
    pub compositor_window_resize_pending: Option<(u16, u16)>,

    /// App IDs that use client-side decorations (from config)
    pub csd_apps: Vec<String>,

    // XWayland support (via xwayland-satellite)
    /// xwayland-satellite process monitor (acts as X11 WM, presents X11 windows as Wayland)
    /// Includes crash tracking for auto-restart with backoff
    pub xwayland_satellite: Option<XWaylandSatelliteMonitor>,

    /// X11 display number (e.g., 0 for :0)
    pub x11_display_number: Option<u32>,

    /// Flag to spawn initial terminal (set when XWayland is ready)
    pub spawn_initial_terminal: bool,
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
    pub cell: StackWindow,
    /// Cached height from last render frame. Used for layout, click detection, and scroll.
    /// Updated by `update_layout_heights()` at the start of each frame.
    pub height: i32,
}

/// All external windows (including X11 apps via xwayland-satellite)
pub type SurfaceKind = ToplevelSurface;

/// A window entry in our column
pub struct WindowEntry {
    /// The surface (Wayland or X11)
    pub surface: SurfaceKind,

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

    /// Whether this window was launched in foreground mode
    /// (launching terminal is hidden and should be restored when this window closes)
    pub is_foreground_gui: bool,
}

/// Timeout for pending resize operations (milliseconds)
pub const RESIZE_TIMEOUT_MS: u128 = 500;

/// Explicit window state machine - prevents implicit state bugs from v1
///
/// # Design Note: Why Not Unified with TerminalSizingState?
///
/// This state machine tracks the Wayland resize protocol (configure/commit handshake).
/// Terminals have a separate `TerminalSizingState` that tracks content correctness.
///
/// These serve fundamentally different purposes:
/// - WindowState: External protocol coordination (Wayland client handshake)
/// - TerminalSizingState: Internal correctness (prevent content double-counting)
///
/// We considered unifying them but concluded:
/// - Different purposes: Protocol tracking vs correctness guarantee
/// - Different data: Windows need serial/timeout; terminals need content_rows/scrollback
/// - Forced unification would obscure their distinct purposes
/// - Terminal state machine prevents v1 bugs; making it generic risks losing that
///
/// The asymmetry is honest - they solve different problems with different requirements.
#[derive(Debug, Clone)]
pub enum WindowState {
    /// Window is stable, accepting input
    Active { height: u32 },

    /// Resize requested, waiting for client acknowledgment
    PendingResize {
        current_height: u32,
        requested_height: u32,
        request_serial: u32,
        /// When the resize was requested, for timeout detection
        requested_at: Instant,
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
///
/// # Design Note: Intentional Asymmetry
///
/// This enum uses different storage patterns for terminals vs external windows:
/// - Terminals: ID-based (data in TerminalManager)
/// - External windows: Inline ownership (Box<WindowEntry>)
///
/// This asymmetry is intentional and reflects their different needs:
/// - Terminals have complex state (PTY, parser, renderer) requiring centralized
///   management with heavy cross-component mutation (input, output, resize)
/// - External windows are simpler Wayland surfaces with minimal shared state
///
/// We considered unifying to WindowId + WindowManager for symmetry, but concluded:
/// - Inline ownership for windows is actually cleaner (no lookups needed)
/// - Asymmetry honestly reflects complexity differences
/// - Unification would add indirection without clear benefit
/// - The ID pattern for terminals is necessary; for windows it would be ceremony
pub enum StackWindow {
    /// An internal terminal, referenced by ID (actual terminal data in TerminalManager)
    Terminal(TerminalId),
    /// An external Wayland window (owns the WindowEntry directly)
    /// Boxed to reduce enum size since WindowEntry is ~200 bytes while TerminalId is 4 bytes
    External(Box<WindowEntry>),
}

impl StackWindow {
    /// Get the terminal ID if this is a Terminal cell
    pub fn terminal_id(&self) -> Option<TerminalId> {
        match self {
            Self::Terminal(id) => Some(*id),
            Self::External(_) => None,
        }
    }

    /// Check if this is a Terminal cell
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminal(_))
    }

    /// Check if this is an External cell
    pub fn is_external(&self) -> bool {
        matches!(self, Self::External(_))
    }

    /// Get a reference to the WindowEntry if this is an External cell
    pub fn external_entry(&self) -> Option<&WindowEntry> {
        match self {
            Self::External(entry) => Some(entry),
            Self::Terminal(_) => None,
        }
    }

    /// Get a mutable reference to the WindowEntry if this is an External cell
    pub fn external_entry_mut(&mut self) -> Option<&mut WindowEntry> {
        match self {
            Self::External(entry) => Some(entry),
            Self::Terminal(_) => None,
        }
    }
}

impl TermStack {
    /// Create a new compositor state
    /// Returns (compositor, display) - display must be kept alive for dispatching
    pub fn new(
        display: Display<Self>,
        loop_handle: LoopHandle<'static, Self>,
        output_size: Size<i32, Physical>,
        csd_apps: Vec<String>,
    ) -> (Self, Display<Self>) {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let text_input_state = TextInputManagerState::new::<Self>(&display_handle);
        let viewporter_state = smithay::wayland::viewporter::ViewporterState::new::<Self>(&display_handle);

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
            viewporter_state,
            space: Space::default(),
            popup_manager: PopupManager::default(),
            layout_nodes: Vec::new(),
            scroll_offset: 0.0,
            focused_window: None,
            layout: ColumnLayout::empty(),
            output_size,
            seat,
            running: true,
            spawn_terminal_requested: false,
            focus_change_requested: 0,
            pending_scroll_delta: 0.0,
            pending_spawn_requests: Vec::new(),
            pending_resize_request: None,
            new_external_window_index: None,
            new_window_needs_keyboard_focus: false,
            external_window_resized: None,
            pending_window_output_terminal: None,
            pending_window_command: None,
            pending_gui_spawn_requests: Vec::new(),
            pending_builtin_requests: Vec::new(),
            pending_gui_foreground: false,
            foreground_gui_sessions: HashMap::new(),
            pending_output_terminal_cleanup: Vec::new(),
            clipboard: arboard::Clipboard::new().ok(),
            clipboard_receiver: None,
            pending_paste: false,
            pending_copy: false,
            primary_selection_receiver: None,
            selecting: None,
            resizing: None,
            key_repeat: None,
            repeat_delay_ms: 400,    // Standard delay before repeat starts
            repeat_interval_ms: 30,  // ~33 keys per second
            pointer_position: Point::from((0.0, 0.0)),
            cursor_on_resize_handle: false,
            pointer_buttons_pressed: 0,
            compositor_window_resize_pending: None,
            csd_apps,
            xwayland_satellite: None,
            x11_display_number: None,
            spawn_initial_terminal: false,
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
            if let StackWindow::External(entry) = &node.cell {
                // Apply Y-flip
                let render_y = crate::coords::content_to_render_y(
                    content_y as f64,
                    height as f64,
                    screen_height as f64
                ) as i32;
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

    /// Calculate maximum scroll offset based on content height
    pub fn max_scroll(&self) -> f64 {
        // Use layout_nodes height which includes title bars for terminals
        let total_height: i32 = self.layout_nodes.iter().map(|node| node.height).sum();
        (total_height as f64 - self.output_size.h as f64).max(0.0)
    }

    /// Apply any accumulated scroll delta (call once per frame after input processing)
    /// Does NOT recalculate layout - caller should do that after
    pub fn apply_pending_scroll(&mut self) {
        if self.pending_scroll_delta != 0.0 {
            // Pending delta is already clamped at accumulation time (in handle_pointer_axis)
            // so this should always stay in valid range, but clamp defensively
            let max_scroll = self.max_scroll();
            self.scroll_offset = (self.scroll_offset + self.pending_scroll_delta).clamp(0.0, max_scroll);
            self.pending_scroll_delta = 0.0;
        }
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
        self.pending_scroll_delta = 0.0; // Clear any pending scroll
        self.recalculate_layout();
    }

    /// Scroll to the bottom (scroll_offset = max)
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.max_scroll();
        self.pending_scroll_delta = 0.0; // Clear any pending scroll
        self.recalculate_layout();
    }

    /// Clear stale drag state if no pointer buttons are pressed.
    ///
    /// This handles the case where a pointer release event is lost (e.g., window
    /// loses focus mid-drag). Without this cleanup, selecting/resizing state
    /// would remain stuck forever.
    pub fn clear_stale_drag_state(&mut self, any_button_pressed: bool) {
        if !any_button_pressed {
            if self.selecting.is_some() {
                tracing::debug!("clearing stale selection state");
                self.selecting = None;
            }
            if self.resizing.is_some() {
                tracing::debug!("clearing stale resize state");
                self.resizing = None;
                self.cursor_on_resize_handle = false;
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
    pub fn get_window_height(&self, index: usize) -> Option<i32> {
        self.layout_nodes.get(index).map(|n| n.height)
    }

    /// Scroll to ensure a cell's bottom edge is visible on screen.
    /// Returns the new scroll offset if it changed, None otherwise.
    pub fn scroll_to_show_window_bottom(&mut self, window_index: usize) -> Option<f64> {
        let y: i32 = self.layout_nodes[..window_index].iter().map(|node| node.height).sum();
        let height = self.layout_nodes.get(window_index).map(|n| n.height).unwrap_or(0);
        let bottom_y = y + height;
        let visible_height = self.output_size.h;
        let total_height: i32 = self.layout_nodes.iter().map(|node| node.height).sum();
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

    /// Get the cell under a point
    ///
    /// The point must be in render coordinates (Y=0 at bottom).
    /// Returns the cell index if found.
    ///
    /// This uses our own coordinate calculation (not Smithay's Space.element_under)
    /// to ensure consistent behavior with Y-flip coordinates.
    pub fn window_at(&self, render_y: crate::coords::RenderY) -> Option<usize> {
        let render_y_value = render_y.value();
        let screen_height = self.output_size.h as f64;
        let mut content_y = -self.scroll_offset;

        for i in 0..self.layout_nodes.len() {
            let window_height = self.layout_nodes[i].height as f64;

            // Calculate render Y for this cell (same formula as main.rs rendering)
            let cell_render_y = crate::coords::content_to_render_y(content_y, window_height, screen_height);
            let render_end = cell_render_y + window_height;

            if render_y_value >= cell_render_y && render_y_value < render_end {
                tracing::debug!(
                    index = i,
                    render_y = render_y_value,
                    cell_render_y,
                    render_end,
                    window_height,
                    content_y,
                    "window_at: hit"
                );
                return Some(i);
            }
            content_y += window_height;
        }
        None
    }

    /// Check if a point is on a terminal cell
    pub fn is_on_terminal(&self, point: Point<f64, smithay::utils::Logical>) -> bool {
        self.window_at(crate::coords::RenderY::new(point.y))
            .map(|i| matches!(self.layout_nodes.get(i), Some(node) if matches!(node.cell, StackWindow::Terminal(_))))
            .unwrap_or(false)
    }

    /// Get the render position (render_y, height) for a cell at the given index
    /// Returns (render_y, height) where render_y is in render coordinates (Y=0 at bottom)
    pub fn get_window_render_position(&self, index: usize) -> (crate::coords::RenderY, i32) {
        let screen_height = self.output_size.h as f64;
        let mut content_y = -self.scroll_offset;

        for i in 0..self.layout_nodes.len() {
            if i == index {
                let height = self.layout_nodes[i].height;
                let render_y = crate::coords::content_to_render_y(content_y, height as f64, screen_height);
                return (crate::coords::RenderY::new(render_y), height);
            }
            content_y += self.layout_nodes[i].height as f64;
        }

        // Fallback if index out of bounds
        (crate::coords::RenderY::new(0.0), 0)
    }

    /// Get the screen bounds (top_y, bottom_y) for a cell at the given index
    /// Returns (top_y, bottom_y) in screen coordinates (Y=0 at top)
    pub fn get_window_screen_bounds(&self, index: usize) -> Option<(i32, i32)> {
        let mut content_y = -(self.scroll_offset as i32);

        for i in 0..self.layout_nodes.len() {
            if i == index {
                // In screen coords: top_y = content_y, bottom_y = content_y + height
                let top_y = content_y;
                let height = self.layout_nodes[i].height;
                let bottom_y = content_y + height;
                return Some((top_y, bottom_y));
            }
            content_y += self.layout_nodes[i].height;
        }
        None
    }

    /// Process pending PRIMARY selection paste (from middle-click)
    ///
    /// This should be called from the main event loop to handle async clipboard reads.
    /// Middle-click triggers the async read, but the result must be checked here
    /// since pointer events don't go through keyboard handling where regular paste is processed.
    pub fn process_primary_selection_paste(&mut self, terminals: &mut crate::terminal_manager::TerminalManager) {
        if let Some(ref receiver) = self.primary_selection_receiver {
            match receiver.try_recv() {
                Ok(text) => {
                    self.primary_selection_receiver = None;
                    if let Some(terminal) = terminals.get_focused_mut(self.focused_window.as_ref()) {
                        if terminal.has_exited() {
                            tracing::debug!("ignoring primary paste to exited terminal");
                        } else if let Err(e) = terminal.write(text.as_bytes()) {
                            tracing::error!(?e, "failed to paste from PRIMARY selection");
                        } else {
                            tracing::debug!(len = text.len(), "pasted from PRIMARY selection");
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // Still waiting
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.primary_selection_receiver = None;
                    tracing::debug!("PRIMARY selection read thread disconnected");
                }
            }
        }
    }
}

// Wayland protocol implementations

impl CompositorHandler for TermStack {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a smithay::reexports::wayland_server::Client) -> &'a CompositorClientState {
        // XWayland client may not have ClientState attached - use a static default
        if let Some(state) = client.get_data::<ClientState>() {
            &state.compositor_state
        } else {
            // This is the XWayland internal client
            static XWAYLAND_COMPOSITOR_STATE: std::sync::OnceLock<CompositorClientState> = std::sync::OnceLock::new();
            XWAYLAND_COMPOSITOR_STATE.get_or_init(CompositorClientState::default)
        }
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

impl BufferHandler for TermStack {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl ShmHandler for TermStack {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

/// Compute the initial configure bounds for external windows.
///
/// We set bounds (max available space) but NOT size, letting apps
/// pick their preferred size within the bounds. This matches what
/// floating compositors like Anvil do.
///
/// On first commit, we enforce our width while keeping the app's height.
#[inline]
pub fn initial_configure_bounds(output_size: Size<i32, Physical>) -> Size<i32, Logical> {
    // Bounds = max available space (converted to logical)
    Size::from((output_size.w, output_size.h))
}

impl XdgShellHandler for TermStack {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        tracing::debug!(
            pending_output_terminal = ?self.pending_window_output_terminal,
            pending_gui_foreground = self.pending_gui_foreground,
            "XDG toplevel created"
        );

        // Configure window with width constraint but let app choose height.
        // Per xdg-shell spec: size=(width, 0) means width is constrained, height is client's choice.
        // Tiled states indicate the app is in a column layout with fixed width.
        let bounds = initial_configure_bounds(self.output_size);
        let constrained_width = self.output_size.w;
        surface.with_pending_state(|state| {
            state.bounds = Some(bounds);
            // Width constrained, height=0 means client chooses
            state.size = Some(Size::from((constrained_width, 0)));
            // Tiled states tell the app it's width-constrained (like a tiled window)
            state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledLeft);
            state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledRight);
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
                if let StackWindow::External(entry) = &node.cell {
                    if entry.surface.wl_surface() == parent {
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
                if let StackWindow::External(entry) = &node.cell {
                    if entry.surface.wl_surface() == parent {
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

impl SeatHandler for TermStack {
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

impl SelectionHandler for TermStack {
    type SelectionUserData = ();
}

impl DataDeviceHandler for TermStack {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for TermStack {}
impl ServerDndGrabHandler for TermStack {}
impl OutputHandler for TermStack {}

impl XdgDecorationHandler for TermStack {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        // Advertise server-side decoration as preferred
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(DecorationMode::ServerSide);
        });
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        let uses_csd = matches!(mode, DecorationMode::ClientSide);

        let surface = toplevel.wl_surface();
        for node in &mut self.layout_nodes {
            if let StackWindow::External(entry) = &mut node.cell {
                if entry.surface.wl_surface() == surface {
                    entry.uses_csd = uses_csd;
                    tracing::info!(
                        requested = ?mode,
                        uses_csd = entry.uses_csd,
                        command = %entry.command,
                        "decoration mode negotiated"
                    );
                    break;
                }
            }
        }

        // Honor client's request
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(mode);
        });
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        // Client unset mode preference - revert to server-side
        let surface = toplevel.wl_surface();
        for node in &mut self.layout_nodes {
            if let StackWindow::External(entry) = &mut node.cell {
                if entry.surface.wl_surface() == surface {
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
// Note: XWaylandShellHandler not needed - xwayland-satellite handles X11 WM duties
delegate_compositor!(TermStack);
delegate_shm!(TermStack);
delegate_xdg_shell!(TermStack);
delegate_xdg_decoration!(TermStack);
delegate_seat!(TermStack);
delegate_data_device!(TermStack);
delegate_output!(TermStack);
delegate_text_input_manager!(TermStack);
delegate_viewporter!(TermStack);

#[cfg(test)]
mod tests {
    use super::*;

    /// Test data for positioning - simulates the state needed for window_at() calculations
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
        /// Replicate the window_at() logic for testing
        /// This is the exact same formula as in TermStack::window_at()
        fn window_at(&self, y: f64) -> Option<usize> {
            let screen_height = self.screen_height as f64;
            let mut content_y = -self.scroll_offset;

            for (i, &height) in self.layout_heights.iter().enumerate() {
                let window_height = height as f64;

                // Y-flip formula
                let render_y = crate::coords::content_to_render_y(content_y, window_height, screen_height);
                let render_end = render_y + window_height;

                if y >= render_y && y < render_end {
                    return Some(i);
                }
                content_y += window_height;
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
                    let render_y = crate::coords::content_to_render_y(
                        content_y as f64,
                        height as f64,
                        screen_height as f64
                    ) as i32;
                    content_y += height;
                    (render_y, height)
                })
                .collect()
        }

        /// Get the render Y range for each cell (render_start, render_end)
        fn window_ranges(&self) -> Vec<(f64, f64)> {
            let screen_height = self.screen_height as f64;
            let mut content_y = -self.scroll_offset;

            self.layout_heights
                .iter()
                .map(|&height| {
                    let window_height = height as f64;
                    let render_y = crate::coords::content_to_render_y(content_y, window_height, screen_height);
                    let render_end = render_y + window_height;
                    content_y += window_height;
                    (render_y, render_end)
                })
                .collect()
        }
    }

    #[test]
    fn test_window_at_single_cell() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            layout_heights: vec![200],
        };

        // With Y-flip: cell 0 (height 200) at content_y=0
        // render_y = 600 - 0 - 200 = 400
        // render_end = 400 + 200 = 600
        // So cell 0 is at render Y 400-600 (TOP of screen in render coords)

        assert_eq!(pos.window_at(300.0), None, "render Y=300 below cell 0");
        assert_eq!(pos.window_at(400.0), Some(0), "render Y=400 at cell 0 start");
        assert_eq!(pos.window_at(500.0), Some(0), "render Y=500 inside cell 0");
        assert_eq!(pos.window_at(599.0), Some(0), "render Y=599 inside cell 0");
        assert_eq!(pos.window_at(600.0), None, "render Y=600 is past screen top");
    }

    #[test]
    fn test_window_at_two_cells_no_overlap() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            layout_heights: vec![200, 300],
        };

        // Cell 0: content_y=0, height=200
        //   render_y = 600 - 0 - 200 = 400, render_end = 600
        // Cell 1: content_y=200, height=300
        //   render_y = 600 - 200 - 300 = 100, render_end = 400

        let ranges = pos.window_ranges();
        assert_eq!(ranges[0], (400.0, 600.0), "cell 0 at top (high render Y)");
        assert_eq!(ranges[1], (100.0, 400.0), "cell 1 below (lower render Y)");

        // Cell 0's bottom (400) equals cell 1's top (400) - no overlap
        assert_eq!(ranges[0].0, ranges[1].1, "cells should be adjacent");

        // Test click detection
        assert_eq!(pos.window_at(500.0), Some(0), "render Y=500 hits cell 0");
        assert_eq!(pos.window_at(400.0), Some(0), "render Y=400 at boundary hits cell 0");
        assert_eq!(pos.window_at(399.0), Some(1), "render Y=399 hits cell 1");
        assert_eq!(pos.window_at(200.0), Some(1), "render Y=200 hits cell 1");
        assert_eq!(pos.window_at(100.0), Some(1), "render Y=100 at cell 1 start");
        assert_eq!(pos.window_at(99.0), None, "render Y=99 below all cells");
    }

    #[test]
    fn test_render_positions_match_click_detection() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            layout_heights: vec![150, 200, 100],
        };

        let render_pos = pos.render_positions();
        let click_ranges = pos.window_ranges();

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
    fn test_window_positions_with_scroll() {
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

        let ranges = pos.window_ranges();
        assert_eq!(ranges[0], (450.0, 650.0), "cell 0 scrolled up (higher render Y)");
        assert_eq!(ranges[1], (150.0, 450.0), "cell 1 scrolled up");

        // Cell 0's bottom (450) equals cell 1's top (450)
        assert_eq!(ranges[0].0, ranges[1].1, "cells should remain adjacent after scroll");
    }

    #[test]
    fn test_window_order_matches_visual_top_to_bottom() {
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

        let ranges = pos.window_ranges();
        assert_eq!(ranges[0], (500.0, 600.0), "cell 0");
        assert_eq!(ranges[1], (300.0, 500.0), "cell 1");
        assert_eq!(ranges[2], (150.0, 300.0), "cell 2");

        // Cell 0 at top, cell 1 below, cell 2 at bottom
        assert!(ranges[0].0 > ranges[1].0, "cell 0 should be higher than cell 1");
        assert!(ranges[1].0 > ranges[2].0, "cell 1 should be higher than cell 2");

        // Clicking at HIGH render Y (top of screen) should hit cell 0
        assert_eq!(pos.window_at(550.0), Some(0), "high render Y (top of screen) hits cell 0");

        // Clicking at LOW render Y (bottom of screen) should hit cell 2
        assert_eq!(pos.window_at(200.0), Some(2), "low render Y (bottom of screen) hits cell 2");

        // Below cell 2 should hit nothing
        assert_eq!(pos.window_at(149.0), None, "below all cells hits nothing");
    }

    #[test]
    fn test_windows_stack_vertically_not_overlap() {
        let heights = vec![200, 300, 150];
        let pos = MockPositioning {
            screen_height: 800,
            scroll_offset: 0.0,
            layout_heights: heights.clone(),
        };

        let ranges = pos.window_ranges();

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
    fn test_point_in_only_one_window() {
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            layout_heights: vec![100, 150, 200],
        };

        // Check every pixel in the cell range
        let total_height: i32 = pos.layout_heights.iter().sum();
        let bottom_y = pos.screen_height - total_height;

        for y in bottom_y..pos.screen_height {
            let result = pos.window_at(y as f64);
            assert!(result.is_some(), "render Y={} should hit a cell", y);
            let idx = result.unwrap();
            assert!(idx < 3, "cell index should be valid");
        }

        // Below content should hit nothing
        assert_eq!(pos.window_at((bottom_y - 1) as f64), None);
        // At/above screen top should hit nothing
        assert_eq!(pos.window_at(600.0), None);
    }

    #[test]
    fn test_click_below_last_window() {
        // Verify we can detect when a click is below all cells
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 0.0,
            layout_heights: vec![200, 300],
        };

        // Cell 0: content_y=0,   height=200 → render_y=400, render_end=600
        // Cell 1: content_y=200, height=300 → render_y=100, render_end=400
        // Total content height = 500
        // Last cell bottom in render coords = 600 - 500 = 100

        let ranges = pos.window_ranges();
        assert_eq!(ranges[1], (100.0, 400.0), "last cell (cell 1) range");

        // Click at Y=99 (below last cell's render_y of 100) should hit nothing
        assert_eq!(pos.window_at(99.0), None, "click below last cell hits nothing");

        // Click at Y=100 (at last cell's render_y) should hit last cell
        assert_eq!(pos.window_at(100.0), Some(1), "click at last cell bottom hits last cell");

        // Calculate last_window_bottom (same logic as the implementation)
        let screen_height = pos.screen_height as f64;
        let mut content_y = -pos.scroll_offset;
        for &height in &pos.layout_heights {
            content_y += height as f64;
        }
        let last_window_bottom = screen_height - content_y;
        assert_eq!(last_window_bottom, 100.0, "last cell bottom should be at render Y=100");

        // Clicks below last_window_bottom should trigger "focus last cell" logic
        assert!(50.0 < last_window_bottom, "Y=50 is below last cell");
        assert!(99.0 < last_window_bottom, "Y=99 is below last cell");
        assert!(!(100.0 < last_window_bottom), "Y=100 is NOT below last cell (it's at the edge)");
    }

    #[test]
    fn test_click_below_with_scroll() {
        // Verify click detection below cells works correctly when scrolled
        let pos = MockPositioning {
            screen_height: 600,
            scroll_offset: 100.0,
            layout_heights: vec![200, 300],
        };

        // With scroll=100:
        // content_y starts at -100
        // Cell 0: content_y=-100, height=200 → render_y=500, render_end=700 (partially off screen)
        //         content_y becomes -100 + 200 = 100
        // Cell 1: content_y=100, height=300 → render_y=200, render_end=500
        //         content_y becomes 100 + 300 = 400
        // Last cell bottom = screen_height - final_content_y = 600 - 400 = 200

        let ranges = pos.window_ranges();
        assert_eq!(ranges[1], (200.0, 500.0), "last cell with scroll");

        // Calculate last_window_bottom with scroll
        let screen_height = pos.screen_height as f64;
        let mut content_y = -pos.scroll_offset;
        for &height in &pos.layout_heights {
            content_y += height as f64;
        }
        let last_window_bottom = screen_height - content_y;
        assert_eq!(last_window_bottom, 200.0, "last cell bottom should be at render Y=200");

        // Clicks below 200 should trigger "focus last cell"
        assert!(50.0 < last_window_bottom);
        assert!(199.0 < last_window_bottom);
    }

    #[test]
    fn test_new_terminals_insert_above_focused() {
        use crate::terminal_manager::TerminalId;
        use crate::state::{StackWindow, LayoutNode};

        // Simulate cell insertion behavior
        let mut layout_nodes: Vec<LayoutNode> = Vec::new();
        let mut focused_index: Option<usize> = None;

        // Helper to add terminal with the same logic as add_terminal
        let add_terminal = |id: u32, nodes: &mut Vec<LayoutNode>, focused: &mut Option<usize>| {
            let insert_index = focused.unwrap_or(nodes.len());
            nodes.insert(insert_index, LayoutNode {
                cell: StackWindow::Terminal(TerminalId(id)),
                height: 0
            });
            *focused = Some(focused.map(|idx| idx + 1).unwrap_or(insert_index));
        };

        // Add first terminal - should be focused
        add_terminal(0, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 1);
        assert_eq!(focused_index, Some(0));
        assert!(matches!(layout_nodes[0].cell, StackWindow::Terminal(TerminalId(0))));

        // Add second terminal - should appear above T0, focus stays on T0
        add_terminal(1, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 2);
        assert_eq!(focused_index, Some(1), "focus should move to index 1 (still T0)");
        assert!(matches!(layout_nodes[0].cell, StackWindow::Terminal(TerminalId(1))), "T1 should be at index 0 (top)");
        assert!(matches!(layout_nodes[1].cell, StackWindow::Terminal(TerminalId(0))), "T0 should be at index 1");

        // Add third terminal - should appear above T0 (at index 1), focus stays on T0
        add_terminal(2, &mut layout_nodes, &mut focused_index);
        assert_eq!(layout_nodes.len(), 3);
        assert_eq!(focused_index, Some(2), "focus should move to index 2 (still T0)");
        assert!(matches!(layout_nodes[0].cell, StackWindow::Terminal(TerminalId(1))), "T1 should be at index 0");
        assert!(matches!(layout_nodes[1].cell, StackWindow::Terminal(TerminalId(2))), "T2 should be at index 1");
        assert!(matches!(layout_nodes[2].cell, StackWindow::Terminal(TerminalId(0))), "T0 should be at index 2 (bottom)");
    }

    #[test]
    fn focus_next_skips_hidden_terminals() {
        // Test helper: simulate focus_next logic
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)), // visible
            StackWindow::Terminal(TerminalId(1)), // hidden
            StackWindow::Terminal(TerminalId(2)), // visible
        ];

        let current_index = 0;
        let is_visible = |id: TerminalId| id.0 != 1; // T1 is hidden

        // Simulate focus_next logic: find next visible window
        let next_index = (current_index + 1..layout_nodes.len())
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            next_index,
            Some(2),
            "focus_next should skip hidden terminal at index 1 and land on index 2"
        );
    }

    #[test]
    fn focus_prev_skips_hidden_terminals() {
        // Test helper: simulate focus_prev logic
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)), // visible
            StackWindow::Terminal(TerminalId(1)), // hidden
            StackWindow::Terminal(TerminalId(2)), // visible
        ];

        let current_index = 2;
        let is_visible = |id: TerminalId| id.0 != 1; // T1 is hidden

        // Simulate focus_prev logic: find previous visible window
        let prev_index = (0..current_index)
            .rev()
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            prev_index,
            Some(0),
            "focus_prev should skip hidden terminal at index 1 and land on index 0"
        );
    }

    #[test]
    fn focus_next_stops_at_boundary_when_remaining_hidden() {
        // T0 (visible), T1 (hidden)
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)),
            StackWindow::Terminal(TerminalId(1)),
        ];

        let current_index = 0;
        let is_visible = |id: TerminalId| id.0 != 1; // T1 is hidden

        // Simulate focus_next: should find no visible window after current
        let next_index = (current_index + 1..layout_nodes.len())
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            next_index, None,
            "focus_next should return None when all remaining terminals are hidden"
        );
    }

    #[test]
    fn focus_prev_stops_at_boundary_when_remaining_hidden() {
        // T0 (hidden), T1 (visible)
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)),
            StackWindow::Terminal(TerminalId(1)),
        ];

        let current_index = 1;
        let is_visible = |id: TerminalId| id.0 != 0; // T0 is hidden

        // Simulate focus_prev: should find no visible window before current
        let prev_index = (0..current_index)
            .rev()
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            prev_index, None,
            "focus_prev should return None when all previous terminals are hidden"
        );
    }

    #[test]
    fn focus_next_skips_multiple_hidden_in_sequence() {
        // T0 (visible), T1 (hidden), T2 (hidden), T3 (visible)
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)),
            StackWindow::Terminal(TerminalId(1)),
            StackWindow::Terminal(TerminalId(2)),
            StackWindow::Terminal(TerminalId(3)),
        ];

        let current_index = 0;
        let is_visible = |id: TerminalId| id.0 != 1 && id.0 != 2; // T1, T2 hidden

        // Simulate focus_next: should skip both T1 and T2
        let next_index = (current_index + 1..layout_nodes.len())
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            next_index,
            Some(3),
            "focus_next should skip multiple consecutive hidden terminals"
        );
    }

    #[test]
    fn focus_prev_skips_multiple_hidden_in_sequence() {
        // T0 (visible), T1 (hidden), T2 (hidden), T3 (visible)
        let layout_nodes = vec![
            StackWindow::Terminal(TerminalId(0)),
            StackWindow::Terminal(TerminalId(1)),
            StackWindow::Terminal(TerminalId(2)),
            StackWindow::Terminal(TerminalId(3)),
        ];

        let current_index = 3;
        let is_visible = |id: TerminalId| id.0 != 1 && id.0 != 2; // T1, T2 hidden

        // Simulate focus_prev: should skip both T2 and T1
        let prev_index = (0..current_index)
            .rev()
            .find(|&i| match &layout_nodes[i] {
                StackWindow::Terminal(id) => is_visible(*id),
                StackWindow::External(_) => true,
            });

        assert_eq!(
            prev_index,
            Some(0),
            "focus_prev should skip multiple consecutive hidden terminals"
        );
    }

    // ==========================================================================
    // Multi-window GUI spawn tests
    // ==========================================================================
    // These tests verify that multi-window apps (like WebKitGTK-based surf)
    // have all their windows inserted at the correct position relative to
    // the output terminal.

    /// Test that reading pending_window_output_terminal doesn't consume it.
    /// This is critical for multi-window apps where multiple windows arrive
    /// and all need to be positioned relative to the same output terminal.
    #[test]
    fn pending_output_terminal_not_consumed_on_read() {
        // Simulate the pattern used in add_window():
        // let output_terminal = self.pending_window_output_terminal;  // READ, not take()
        let pending: Option<TerminalId> = Some(TerminalId(42));

        // First read (simulating first window arriving)
        let first_read = pending;
        assert_eq!(first_read, Some(TerminalId(42)));

        // Second read (simulating second window arriving)
        // With our fix, pending is still available
        let second_read = pending;
        assert_eq!(second_read, Some(TerminalId(42)));

        // Both windows should see the same output terminal
        assert_eq!(first_read, second_read);
    }

    /// Test that take() DOES consume the value (contrast with above).
    /// This documents the old buggy behavior we fixed.
    #[test]
    fn option_take_does_consume_value() {
        let mut pending: Option<TerminalId> = Some(TerminalId(42));

        // First take (old buggy behavior)
        let first_take = pending.take();
        assert_eq!(first_take, Some(TerminalId(42)));

        // Second take - value is gone!
        let second_take = pending.take();
        assert_eq!(second_take, None);

        // This is why second window would be inserted at wrong position
    }

    /// Test multi-window insert positions.
    /// When output terminal is at position 0, all GUI windows should be
    /// inserted at position 0, pushing each other down.
    #[test]
    fn multi_window_insert_all_above_output_terminal() {
        // Simulate layout: [output_term, launcher]
        // Output terminal at index 0
        let output_terminal_pos = 0;

        // Simulate inserting windows at output terminal position
        let mut layout: Vec<&str> = vec!["output_term", "launcher"];

        // First window arrives, inserted at position 0
        layout.insert(output_terminal_pos, "window_A");
        assert_eq!(layout, vec!["window_A", "output_term", "launcher"]);

        // For second window, output_term is now at position 1
        // But with our fix, we still use the ORIGINAL output terminal's
        // current position (found by ID, not stored index)
        let output_term_current_pos = layout.iter().position(|&x| x == "output_term").unwrap();
        assert_eq!(output_term_current_pos, 1);

        // Second window inserted at output terminal's current position
        layout.insert(output_term_current_pos, "window_B");
        assert_eq!(layout, vec!["window_A", "window_B", "output_term", "launcher"]);

        // Both windows are above output_term (lower indices = higher on screen)
    }

    // ==========================================================================
    // Surface coordinate tests (title bar offset)
    // ==========================================================================

    /// Test surface position calculation for SSD (server-side decorated) windows.
    /// The surface starts BELOW our title bar, so screen position must include offset.
    #[test]
    fn surface_position_includes_title_bar_for_ssd() {
        use crate::title_bar::TITLE_BAR_HEIGHT;

        let content_y = 100.0;  // Cell starts at content Y = 100
        let uses_csd = false;   // Server-side decorations (we draw title bar)

        // Calculate title bar offset
        let title_bar_offset = if uses_csd { 0.0 } else { TITLE_BAR_HEIGHT as f64 };

        // Surface position in screen coords
        let screen_surface_y = content_y + title_bar_offset;

        // Surface should be below the title bar
        assert_eq!(screen_surface_y, 100.0 + TITLE_BAR_HEIGHT as f64);
        assert!(screen_surface_y > content_y, "surface should be below cell top");
    }

    /// Test surface position for CSD (client-side decorated) windows.
    /// No title bar offset needed - client handles its own decorations.
    #[test]
    fn surface_position_no_offset_for_csd() {
        let content_y = 100.0;  // Cell starts at content Y = 100
        let uses_csd = true;    // Client-side decorations

        // Calculate title bar offset
        let title_bar_offset = if uses_csd { 0.0 } else { 24.0 };

        // Surface position in screen coords
        let screen_surface_y = content_y + title_bar_offset;

        // Surface should be at cell top (no offset)
        assert_eq!(screen_surface_y, content_y);
    }

    /// Test that relative Y calculation accounts for title bar.
    /// When user clicks at screen Y, we need correct surface-local Y.
    #[test]
    fn relative_y_calculation_with_title_bar() {
        use crate::title_bar::TITLE_BAR_HEIGHT;

        let output_height = 600.0;
        let content_y = 100.0;
        let uses_csd = false;

        // Click at screen Y = 150 (50 pixels below cell top at 100)
        let click_screen_y = 150.0;

        // Convert to render coords (Y-flip)
        let click_render_y = output_height - click_screen_y;  // 450

        // Cell's render_end (top of cell in render coords)
        let render_end = output_height - content_y;  // 500

        // Calculate title bar offset
        let title_bar_offset = if uses_csd { 0.0 } else { TITLE_BAR_HEIGHT as f64 };

        // Relative Y within surface (accounting for title bar)
        let relative_y = render_end - click_render_y - title_bar_offset;

        // Click was 50px below cell top, minus title bar = position within surface
        let expected_surface_y = 50.0 - title_bar_offset;
        assert_eq!(relative_y, expected_surface_y);

        // If title bar is 24px, surface-local Y should be 50 - 24 = 26
        // (click is 26px into the actual surface content)
    }
}
