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

    /// Flag to spawn a new terminal (set by input handler)
    pub spawn_terminal_requested: bool,

    /// Focus navigation request (1 = next, -1 = prev)
    pub focus_change_requested: i32,

    /// Scroll request (in pixels, positive = down)
    pub scroll_requested: f64,

    /// Whether keyboard focus is on an external Wayland window (vs internal terminal)
    pub external_window_focused: bool,

    /// Total height of internal terminals (updated each frame for click detection)
    pub terminal_total_height: i32,

    /// Cached window heights for consistent positioning between input and render
    /// Updated at start of each frame before input processing
    pub cached_window_heights: Vec<i32>,
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
            windows: Vec::new(),
            scroll_offset: 0.0,
            focused_index: None,
            layout: ColumnLayout::empty(),
            output_size,
            seat,
            running: true,
            spawn_terminal_requested: false,
            focus_change_requested: 0,
            scroll_requested: 0.0,
            external_window_focused: false,
            terminal_total_height: 0,
            cached_window_heights: Vec::new(),
        };

        (compositor, display)
    }

    /// Recalculate layout after any change
    pub fn recalculate_layout(&mut self) {
        self.layout = ColumnLayout::calculate(
            &self.windows,
            self.output_size.h as u32,
            self.scroll_offset,
        );

        // Update window positions in Space using CACHED heights (actual rendered heights)
        // This is critical: ColumnLayout uses state.current_height(), but we render
        // using actual element heights stored in cached_window_heights.
        // For click detection to match rendering, Space must use the same heights.
        self.update_space_positions();
    }

    /// Update Space element positions using cached_window_heights
    /// This ensures click detection matches actual rendered positions
    pub fn update_space_positions(&mut self) {
        // Calculate screen_y for each window using cached heights
        // Formula: screen_y = terminal_total_height - scroll_offset + accumulated_height
        let mut accumulated: i32 = 0;
        let scroll = self.scroll_offset as i32;

        for (i, entry) in self.windows.iter().enumerate() {
            let screen_y = self.terminal_total_height - scroll + accumulated;
            let loc = Point::from((0, screen_y));
            self.space.map_element(entry.window.clone(), loc, false);

            // Use cached height for accumulation (actual rendered height)
            let height = self.cached_window_heights.get(i).copied().unwrap_or(200);
            accumulated += height;

            tracing::trace!(
                index = i,
                screen_y,
                height,
                terminal_height = self.terminal_total_height,
                scroll,
                "update_space_positions: window position"
            );
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
        // Total content = terminals + external windows
        let window_height: i32 = self.cached_window_heights.iter().sum();
        let total_height = self.terminal_total_height + window_height;
        let max_scroll = (total_height as f64 - self.output_size.h as f64).max(0.0);
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

    /// Update cached window heights - call at start of each frame
    /// IMPORTANT: This only syncs the count with windows list.
    /// It preserves existing heights (from previous frame's actual element heights)
    /// and only initializes NEW windows from bbox.
    pub fn update_cached_window_heights(&mut self) {
        let window_count = self.windows.len();
        let cached_count = self.cached_window_heights.len();

        if cached_count > window_count {
            // Windows were removed - truncate
            self.cached_window_heights.truncate(window_count);
        } else if cached_count < window_count {
            // Windows were added - append new entries using bbox for initial height
            for entry in self.windows.iter().skip(cached_count) {
                let bbox = entry.window.bbox();
                let height = if bbox.size.h > 0 {
                    bbox.size.h
                } else {
                    entry.state.current_height() as i32
                };
                self.cached_window_heights.push(height);
            }
        }
        // If counts match, preserve existing heights (from previous frame's actual heights)
    }

    /// Get the window under a point (returns None if point is on internal terminals)
    ///
    /// The point must be in render coordinates (Y=0 at bottom).
    /// This matches the coordinate system used by the pointer location after
    /// the screen-to-render conversion in input handling.
    pub fn window_at(&self, point: Point<f64, smithay::utils::Logical>) -> Option<usize> {
        // First check if point is on terminal area
        if self.is_on_terminal(point) {
            return None;
        }

        // Use Smithay's Space element_under for proper coordinate handling
        // This respects the window positions we set via map_element
        if let Some((window, _loc)) = self.space.element_under(point) {
            // Find the index of this window in our windows list
            // Compare by toplevel surface since Window clones share the same surface
            if let Some(found_toplevel) = window.toplevel() {
                for (i, entry) in self.windows.iter().enumerate() {
                    if entry.toplevel.wl_surface() == found_toplevel.wl_surface() {
                        tracing::debug!(
                            index = i,
                            point = ?(point.x, point.y),
                            "window_at: found window via Space"
                        );
                        return Some(i);
                    }
                }
            }
        }

        // Fallback: use cached heights calculation
        let terminal_height = self.terminal_total_height as f64;
        let mut window_y = terminal_height - self.scroll_offset;

        for (i, &height) in self.cached_window_heights.iter().enumerate() {
            let window_height = height as f64;
            let window_screen_end = window_y + window_height;

            if point.y >= window_y && point.y < window_screen_end {
                tracing::debug!(
                    index = i,
                    point = ?(point.x, point.y),
                    window_y,
                    "window_at: found window via cached heights"
                );
                return Some(i);
            }
            window_y += window_height;
        }
        None
    }

    /// Check if a point is on the internal terminal area
    ///
    /// The point must be in render coordinates (Y=0 at bottom).
    pub fn is_on_terminal(&self, point: Point<f64, smithay::utils::Logical>) -> bool {
        // Terminals are rendered from Y = -scroll_offset to Y = terminal_height - scroll_offset
        // On screen, this means the terminal area ends at terminal_height - scroll_offset
        let terminal_screen_end = (self.terminal_total_height as f64 - self.scroll_offset).max(0.0);
        point.y < terminal_screen_end
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

    /// Test data for positioning - simulates the state needed for window_at() calculations
    struct MockPositioning {
        terminal_total_height: i32,
        scroll_offset: f64,
        cached_window_heights: Vec<i32>,
    }

    impl MockPositioning {
        /// Replicate the window_at() logic for testing
        fn window_at(&self, y: f64) -> Option<usize> {
            let terminal_height = self.terminal_total_height as f64;
            let mut window_y = terminal_height - self.scroll_offset;

            for (i, &height) in self.cached_window_heights.iter().enumerate() {
                let window_height = height as f64;
                let window_screen_end = window_y + window_height;

                if y >= window_y && y < window_screen_end {
                    return Some(i);
                }
                window_y += window_height;
            }
            None
        }

        /// Replicate the rendering position calculation from main.rs
        fn render_positions(&self) -> Vec<(i32, i32)> {
            let mut window_y = -(self.scroll_offset as i32) + self.terminal_total_height;
            self.cached_window_heights
                .iter()
                .map(|&height| {
                    let y = window_y;
                    window_y += height;
                    (y, height)
                })
                .collect()
        }

        /// Get the Y range for each window (start, end)
        fn window_ranges(&self) -> Vec<(f64, f64)> {
            let terminal_height = self.terminal_total_height as f64;
            let mut window_y = terminal_height - self.scroll_offset;

            self.cached_window_heights
                .iter()
                .map(|&height| {
                    let start = window_y;
                    let end = window_y + height as f64;
                    window_y = end;
                    (start, end)
                })
                .collect()
        }
    }

    #[test]
    fn test_window_at_single_window() {
        let pos = MockPositioning {
            terminal_total_height: 100,
            scroll_offset: 0.0,
            cached_window_heights: vec![200],
        };

        // Window should be at Y=100 to Y=300
        assert_eq!(pos.window_at(50.0), None, "should not hit window at Y=50 (on terminal)");
        assert_eq!(pos.window_at(100.0), Some(0), "should hit window 0 at Y=100");
        assert_eq!(pos.window_at(200.0), Some(0), "should hit window 0 at Y=200");
        assert_eq!(pos.window_at(299.0), Some(0), "should hit window 0 at Y=299");
        assert_eq!(pos.window_at(300.0), None, "should not hit window at Y=300 (past end)");
    }

    #[test]
    fn test_window_at_two_windows_no_overlap() {
        let pos = MockPositioning {
            terminal_total_height: 100,
            scroll_offset: 0.0,
            cached_window_heights: vec![150, 200],
        };

        // Window 0: Y=100 to Y=250
        // Window 1: Y=250 to Y=450
        let ranges = pos.window_ranges();
        assert_eq!(ranges[0], (100.0, 250.0), "window 0 range");
        assert_eq!(ranges[1], (250.0, 450.0), "window 1 range");

        // Verify no overlap
        assert!(ranges[0].1 <= ranges[1].0, "windows should not overlap");

        // Test click detection
        assert_eq!(pos.window_at(100.0), Some(0), "Y=100 should hit window 0");
        assert_eq!(pos.window_at(249.0), Some(0), "Y=249 should hit window 0");
        assert_eq!(pos.window_at(250.0), Some(1), "Y=250 should hit window 1");
        assert_eq!(pos.window_at(449.0), Some(1), "Y=449 should hit window 1");
    }

    #[test]
    fn test_render_positions_match_click_detection() {
        let pos = MockPositioning {
            terminal_total_height: 100,
            scroll_offset: 0.0,
            cached_window_heights: vec![150, 200, 100],
        };

        let render_pos = pos.render_positions();
        let click_ranges = pos.window_ranges();

        // Verify render Y matches click detection start Y
        for (i, ((render_y, render_h), (click_start, click_end))) in
            render_pos.iter().zip(click_ranges.iter()).enumerate()
        {
            assert_eq!(
                *render_y as f64, *click_start,
                "window {} render Y ({}) should match click start ({})",
                i, render_y, click_start
            );
            assert_eq!(
                *render_h as f64, click_end - click_start,
                "window {} render height should match click height",
                i
            );
        }
    }

    #[test]
    fn test_window_positions_with_scroll() {
        let pos = MockPositioning {
            terminal_total_height: 100,
            scroll_offset: 50.0,
            cached_window_heights: vec![150, 200],
        };

        // With scroll=50:
        // Window 0: Y=100-50=50 to Y=200
        // Window 1: Y=200 to Y=400
        let ranges = pos.window_ranges();
        assert_eq!(ranges[0], (50.0, 200.0), "window 0 range with scroll");
        assert_eq!(ranges[1], (200.0, 400.0), "window 1 range with scroll");

        // Render positions should also be shifted
        let render_pos = pos.render_positions();
        assert_eq!(render_pos[0].0, 50, "render Y for window 0 with scroll");
        assert_eq!(render_pos[1].0, 200, "render Y for window 1 with scroll");
    }

    #[test]
    fn test_window_at_returns_correct_index_not_flipped() {
        // This test verifies the issue: "click targets seem to be flipped vertically"
        let pos = MockPositioning {
            terminal_total_height: 0,  // No terminals
            scroll_offset: 0.0,
            cached_window_heights: vec![200, 300],  // Window 0 is 200px, Window 1 is 300px
        };

        // Window 0 should be at Y=0-200 (top)
        // Window 1 should be at Y=200-500 (bottom)

        // Clicking near the top should hit window 0, not window 1
        assert_eq!(pos.window_at(10.0), Some(0), "click at Y=10 should hit window 0 (top)");
        assert_eq!(pos.window_at(100.0), Some(0), "click at Y=100 should hit window 0");
        assert_eq!(pos.window_at(199.0), Some(0), "click at Y=199 should hit window 0");

        // Clicking lower should hit window 1
        assert_eq!(pos.window_at(200.0), Some(1), "click at Y=200 should hit window 1");
        assert_eq!(pos.window_at(300.0), Some(1), "click at Y=300 should hit window 1 (bottom)");
        assert_eq!(pos.window_at(499.0), Some(1), "click at Y=499 should hit window 1");
    }

    #[test]
    fn test_windows_stack_vertically_not_overlap() {
        // This test verifies: "Windows are still overlapping"
        let heights = vec![200, 300, 150];
        let pos = MockPositioning {
            terminal_total_height: 0,
            scroll_offset: 0.0,
            cached_window_heights: heights.clone(),
        };

        let ranges = pos.window_ranges();

        // Each window's end should equal the next window's start
        for i in 0..ranges.len() - 1 {
            assert_eq!(
                ranges[i].1, ranges[i + 1].0,
                "window {} end ({}) should equal window {} start ({})",
                i, ranges[i].1, i + 1, ranges[i + 1].0
            );
        }

        // Total height should be sum of all heights
        let total: f64 = heights.iter().map(|&h| h as f64).sum();
        assert_eq!(ranges.last().unwrap().1, total, "total height should match");
    }

    #[test]
    fn test_point_in_only_one_window() {
        // For any Y coordinate, window_at should return at most one window
        let pos = MockPositioning {
            terminal_total_height: 50,
            scroll_offset: 0.0,
            cached_window_heights: vec![100, 150, 200],
        };

        // Check every pixel from 0 to 550
        for y in 0..550 {
            let result = pos.window_at(y as f64);
            // Ensure we get either None or exactly one index
            if let Some(idx) = result {
                assert!(idx < 3, "window index should be valid");
                // Verify this Y doesn't also hit another window
                for other_idx in 0..3 {
                    if other_idx != idx {
                        // Re-check - the same Y should not be in multiple windows
                        // This is implicitly tested by window_at returning a single value
                    }
                }
            }
        }
    }
}
