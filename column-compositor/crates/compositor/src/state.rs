//! Compositor state machine
//!
//! This module contains the main compositor state, implementing explicit state
//! tracking to prevent the bugs encountered in v1.

use smithay::delegate_compositor;
use smithay::delegate_data_device;
use smithay::delegate_seat;
use smithay::delegate_shm;
use smithay::delegate_xdg_shell;
use smithay::desktop::{Space, Window};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_buffer::WlBuffer;
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Physical, Point, Size, SERIAL_COUNTER};
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

use crate::layout::ColumnLayout;

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

    /// Desktop space for managing windows
    pub space: Space<Window>,

    /// Our managed windows in column order
    pub windows: Vec<WindowEntry>,

    /// Current scroll offset (pixels from top)
    pub scroll_offset: f64,

    /// Index of focused window
    pub focused_index: Option<usize>,

    /// Cached layout calculation
    pub layout: ColumnLayout,

    /// Output dimensions
    pub output_size: Size<i32, Physical>,

    /// The seat
    pub seat: Seat<Self>,

    /// Running state
    pub running: bool,
}

/// A window entry in our column
pub struct WindowEntry {
    /// The toplevel surface
    pub toplevel: ToplevelSurface,

    /// The window wrapper for space management
    pub window: Window,

    /// Explicit state machine
    pub state: WindowState,
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

    /// Get the target height (for animations/transitions)
    pub fn target_height(&self) -> u32 {
        match self {
            Self::Active { height } => *height,
            Self::PendingResize { requested_height, .. } => *requested_height,
            Self::AwaitingCommit { target_height, .. } => *target_height,
        }
    }
}

impl ColumnCompositor {
    /// Create a new compositor state
    pub fn new(
        display: Display<Self>,
        loop_handle: LoopHandle<'static, Self>,
        output_size: Size<i32, Physical>,
    ) -> Self {
        let display_handle = display.handle();

        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);

        let seat = seat_state.new_wl_seat(&display_handle, "seat0");

        Self {
            display_handle,
            loop_handle,
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            space: Space::default(),
            windows: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
            layout: ColumnLayout::empty(),
            output_size,
            seat,
            running: true,
        }
    }

    /// Recalculate layout after any change
    pub fn recalculate_layout(&mut self) {
        self.layout = ColumnLayout::calculate(
            &self.windows,
            self.output_size.h as u32,
            self.scroll_offset,
        );

        // Update window positions in space
        for (i, entry) in self.windows.iter().enumerate() {
            if let Some(pos) = self.layout.window_positions.get(i) {
                // All windows have the same width (full output width)
                let loc = Point::from((0, pos.y));
                self.space.map_element(entry.window.clone(), loc, false);
            }
        }
    }

    /// Add a new window at the focused position
    pub fn add_window(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new_wayland_window(toplevel.clone());

        // Default initial height (will be resized based on content)
        let initial_height = 200u32;

        let entry = WindowEntry {
            toplevel,
            window: window.clone(),
            state: WindowState::Active {
                height: initial_height,
            },
        };

        // Insert after focused window, or at end if none focused
        let insert_index = self.focused_index.map(|i| i + 1).unwrap_or(self.windows.len());
        self.windows.insert(insert_index, entry);

        // Focus the new window
        self.focused_index = Some(insert_index);

        self.recalculate_layout();

        tracing::info!(
            window_count = self.windows.len(),
            focused = ?self.focused_index,
            "window added"
        );
    }

    /// Remove a window
    pub fn remove_window(&mut self, surface: &WlSurface) {
        if let Some(index) = self
            .windows
            .iter()
            .position(|e| e.toplevel.wl_surface() == surface)
        {
            let entry = self.windows.remove(index);
            self.space.unmap_elem(&entry.window);

            // Update focus
            if self.windows.is_empty() {
                self.focused_index = None;
            } else if let Some(focused) = self.focused_index {
                if focused >= self.windows.len() {
                    self.focused_index = Some(self.windows.len() - 1);
                } else if focused > index {
                    self.focused_index = Some(focused - 1);
                }
            }

            self.recalculate_layout();

            tracing::info!(
                window_count = self.windows.len(),
                focused = ?self.focused_index,
                "window removed"
            );
        }
    }

    /// Request a window resize
    pub fn request_resize(&mut self, index: usize, new_height: u32) {
        if let Some(entry) = self.windows.get_mut(index) {
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
    }

    /// Handle window commit - check for resize completion
    pub fn handle_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self
            .windows
            .iter()
            .position(|e| e.toplevel.wl_surface() == surface)
        else {
            return;
        };

        let entry = &mut self.windows[index];

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
                    self.recalculate_layout();
                }
                WindowState::AwaitingCommit { target_height, .. }
                    if new_height == *target_height =>
                {
                    entry.state = WindowState::Active { height: new_height };
                    tracing::debug!(index, height = new_height, "resize completed");
                    self.recalculate_layout();
                }
                WindowState::Active { height } if new_height != *height => {
                    entry.state = WindowState::Active { height: new_height };
                    tracing::debug!(index, height = new_height, "size changed");
                    self.recalculate_layout();
                }
                _ => {}
            }
        }
    }

    /// Scroll by a delta
    pub fn scroll(&mut self, delta: f64) {
        let max_scroll = (self.layout.total_height as f64 - self.output_size.h as f64).max(0.0);
        self.scroll_offset = (self.scroll_offset + delta).clamp(0.0, max_scroll);
        self.recalculate_layout();
    }

    /// Focus previous window
    pub fn focus_prev(&mut self) {
        if let Some(current) = self.focused_index {
            if current > 0 {
                self.focused_index = Some(current - 1);
                self.ensure_focused_visible();
            }
        }
    }

    /// Focus next window
    pub fn focus_next(&mut self) {
        if let Some(current) = self.focused_index {
            if current + 1 < self.windows.len() {
                self.focused_index = Some(current + 1);
                self.ensure_focused_visible();
            }
        }
    }

    /// Ensure the focused window is visible
    fn ensure_focused_visible(&mut self) {
        if let Some(index) = self.focused_index {
            if let Some(new_scroll) = self.layout.scroll_to_show(index, self.output_size.h as u32) {
                self.scroll_offset = new_scroll;
                self.recalculate_layout();
            }
        }
    }

    /// Get the window under a point
    pub fn window_at(&self, point: Point<f64, smithay::utils::Logical>) -> Option<usize> {
        for (i, pos) in self.layout.window_positions.iter().enumerate() {
            let y = pos.y as f64;
            if point.y >= y && point.y < y + pos.height as f64 {
                return Some(i);
            }
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
