//! Input handling for keyboard and scroll events

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::coords::ScreenY;
use crate::state::{ColumnCell, ColumnCompositor};
use crate::terminal_manager::TerminalManager;

/// Left mouse button code (BTN_LEFT in evdev)
const BTN_LEFT: u32 = 0x110;

/// Scroll amount per key press (pixels)
const SCROLL_STEP: f64 = 50.0;

/// Scroll amount per scroll wheel tick (pixels)
const SCROLL_WHEEL_MULTIPLIER: f64 = 15.0;

/// Convert render coordinates to terminal grid coordinates (col, row)
///
/// - `render_x`, `render_y`: Position in render coordinates (Y=0 at bottom)
/// - `cell_render_y`: The terminal cell's render Y position (bottom of cell)
/// - `cell_height`: The terminal cell's height in pixels
/// - `char_width`, `char_height`: Character cell dimensions from the font
fn render_to_grid_coords(
    render_x: f64,
    render_y: f64,
    cell_render_y: f64,
    cell_height: f64,
    char_width: u32,
    char_height: u32,
) -> (usize, usize) {
    // Convert render coords to terminal-local coords
    // Terminal has Y=0 at top, render has Y=0 at bottom
    let cell_render_end = cell_render_y + cell_height;
    let local_y = (cell_render_end - render_y).max(0.0);
    let local_x = render_x.max(0.0);

    // Convert to grid coordinates
    let col = (local_x / char_width as f64) as usize;
    let row = (local_y / char_height as f64) as usize;

    (col, row)
}

impl ColumnCompositor {
    /// Process an input event with terminal support
    pub fn process_input_event_with_terminals<I: InputBackend>(
        &mut self,
        event: InputEvent<I>,
        terminals: &mut TerminalManager,
    ) {
        match &event {
            InputEvent::Keyboard { .. } => tracing::info!("INPUT: Keyboard event!"),
            InputEvent::PointerMotion { .. } => tracing::trace!("INPUT: PointerMotion"),
            InputEvent::PointerMotionAbsolute { .. } => tracing::trace!("INPUT: PointerMotionAbsolute"),
            InputEvent::PointerButton { .. } => tracing::info!("INPUT: PointerButton"),
            InputEvent::PointerAxis { .. } => tracing::info!("INPUT: PointerAxis"),
            _ => tracing::info!("INPUT: Other event"),
        }
        match event {
            InputEvent::Keyboard { event } => {
                self.handle_keyboard_event(event, Some(terminals));
            }
            InputEvent::PointerMotion { event } => self.handle_pointer_motion(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_motion_absolute(event, terminals)
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button(event, Some(terminals)),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis(event),
            _ => {}
        }
    }

    fn handle_keyboard_event<I: InputBackend>(
        &mut self,
        event: impl KeyboardKeyEvent<I>,
        terminals: Option<&mut TerminalManager>,
    ) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let keycode = event.key_code();
        let key_state = event.state();

        tracing::debug!(?keycode, ?key_state, "keyboard event received");

        let keyboard = self.seat.get_keyboard().unwrap();

        // If an external Wayland window has focus, forward events via Wayland protocol
        if self.is_external_focused() {
            // Still check for compositor bindings first
            let binding_handled = keyboard.input::<bool, _>(
                self,
                keycode,
                key_state,
                serial,
                time,
                |state, modifiers, keysym| {
                    let sym = keysym.modified_sym();
                    if state.handle_compositor_binding_with_terminals(modifiers, sym, key_state) {
                        FilterResult::Intercept(true)
                    } else {
                        // Forward to the focused Wayland surface
                        FilterResult::Forward
                    }
                },
            );

            if binding_handled == Some(true) {
                tracing::info!("compositor binding handled (external window focused)");
            }
            return;
        }

        tracing::info!(?keycode, ?key_state, "processing keyboard event for terminal");

        // Process through keyboard for modifier tracking
        let result = keyboard.input::<(bool, Option<Vec<u8>>), _>(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, modifiers, keysym| {
                let sym = keysym.modified_sym();
                tracing::info!(?sym, ?modifiers, "keysym processed");

                // Handle compositor keybindings
                if state.handle_compositor_binding_with_terminals(modifiers, sym, key_state)
                {
                    tracing::info!("compositor binding handled");
                    FilterResult::Intercept((true, None))
                } else if key_state == KeyState::Pressed {
                    // Convert keysym to bytes for terminal
                    let bytes = keysym_to_bytes(sym, modifiers);
                    tracing::info!(?bytes, "converted to bytes");
                    if !bytes.is_empty() {
                        FilterResult::Intercept((false, Some(bytes)))
                    } else {
                        FilterResult::Forward
                    }
                } else {
                    FilterResult::Forward
                }
            },
        );

        tracing::info!(?result, "keyboard.input result");

        // Handle keyboard input and clipboard operations (requires terminal access)
        if let Some(terminals) = terminals {
            // Forward to focused terminal if we got bytes
            if let Some((handled, Some(bytes))) = result {
                if !handled {
                    let focused_id = terminals.focused;
                    let term_count = terminals.count();
                    tracing::info!(focused = ?focused_id, term_count, ?bytes, "forwarding input to terminal");
                    if let Some(terminal) = terminals.get_focused_mut() {
                        if let Err(e) = terminal.write(&bytes) {
                            tracing::error!(?e, "failed to write to terminal");
                        } else {
                            tracing::info!("write succeeded");
                        }
                    } else {
                        tracing::warn!("no focused terminal to write to");
                    }
                }
            }

            // Paste from clipboard
            if self.pending_paste {
                self.pending_paste = false;
                if let Some(ref mut clipboard) = self.clipboard {
                    match clipboard.get_text() {
                        Ok(text) => {
                            if let Some(terminal) = terminals.get_focused_mut() {
                                if let Err(e) = terminal.write(text.as_bytes()) {
                                    tracing::error!(?e, "failed to paste to terminal");
                                } else {
                                    tracing::info!(len = text.len(), "pasted text to terminal");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(?e, "failed to get clipboard text");
                        }
                    }
                }
            }

            // Copy selected text to clipboard (or entire content if no selection)
            if self.pending_copy {
                self.pending_copy = false;
                if let Some(ref mut clipboard) = self.clipboard {
                    if let Some(terminal) = terminals.get_focused_mut() {
                        // Prefer selection text, fall back to entire grid content
                        let text = if let Some(selected) = terminal.terminal.selection_text() {
                            tracing::info!(len = selected.len(), "copying selection to clipboard");
                            selected
                        } else {
                            let lines = terminal.terminal.grid_content();
                            let text = lines.join("\n");
                            tracing::info!(len = text.len(), "copying entire terminal content to clipboard (no selection)");
                            text
                        };

                        if let Err(e) = clipboard.set_text(text.clone()) {
                            tracing::error!(?e, "failed to copy to clipboard");
                        }
                    }
                }
            }
        }
    }

    /// Handle compositor-level keybindings with terminal spawning
    fn handle_compositor_binding_with_terminals(
        &mut self,
        modifiers: &ModifiersState,
        keysym: Keysym,
        state: KeyState,
    ) -> bool {
        tracing::debug!(?modifiers, ?keysym, ?state, "checking compositor binding");

        // Use the regular binding handler first
        if self.handle_compositor_binding(modifiers, keysym, state) {
            return true;
        }

        // Additional bindings for terminal management
        if state != KeyState::Pressed {
            return false;
        }

        tracing::debug!(logo = modifiers.logo, ctrl = modifiers.ctrl, shift = modifiers.shift,
                        "checking modifiers for spawn");

        // Super+Return or Super+T: Signal to spawn new terminal
        if modifiers.logo {
            match keysym {
                Keysym::Return | Keysym::t | Keysym::T => {
                    tracing::info!("spawn terminal binding triggered (Super)");
                    self.spawn_terminal_requested = true;
                    return true;
                }
                _ => {}
            }
        }

        // Alternative: Ctrl+Shift+T or Ctrl+Shift+Return to spawn terminal
        // (useful when Super is grabbed by parent compositor)
        if modifiers.ctrl && modifiers.shift {
            match keysym {
                Keysym::Return | Keysym::t | Keysym::T => {
                    tracing::info!("spawn terminal binding triggered (Ctrl+Shift)");
                    self.spawn_terminal_requested = true;
                    return true;
                }
                // Focus switching: Ctrl+Shift+J/K or Ctrl+Shift+Down/Up
                Keysym::j | Keysym::J | Keysym::Down => {
                    tracing::info!("focus next terminal");
                    self.focus_change_requested = 1;
                    return true;
                }
                Keysym::k | Keysym::K | Keysym::Up => {
                    tracing::info!("focus prev terminal");
                    self.focus_change_requested = -1;
                    return true;
                }
                // Scrolling: Ctrl+Shift+Page Up/Down
                Keysym::Page_Down => {
                    self.scroll_requested = SCROLL_STEP * 10.0;
                    return true;
                }
                Keysym::Page_Up => {
                    self.scroll_requested = -SCROLL_STEP * 10.0;
                    return true;
                }
                // Clipboard: Ctrl+Shift+V (paste) / Ctrl+Shift+C (copy)
                Keysym::v | Keysym::V => {
                    tracing::info!("paste from clipboard requested");
                    self.pending_paste = true;
                    return true;
                }
                Keysym::c | Keysym::C => {
                    tracing::info!("copy to clipboard requested");
                    self.pending_copy = true;
                    return true;
                }
                _ => {}
            }
        }

        // Page Up/Down without modifiers for scrolling
        if !modifiers.ctrl && !modifiers.alt && !modifiers.logo {
            match keysym {
                Keysym::Page_Down => {
                    self.scroll_requested = SCROLL_STEP * 10.0;
                    return true;
                }
                Keysym::Page_Up => {
                    self.scroll_requested = -SCROLL_STEP * 10.0;
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    /// Handle compositor-level keybindings
    /// Returns true if the binding was handled
    fn handle_compositor_binding(
        &mut self,
        modifiers: &ModifiersState,
        keysym: Keysym,
        state: KeyState,
    ) -> bool {
        // Only process on key press, not release
        if state != KeyState::Pressed {
            return false;
        }

        // Ctrl+Shift+Q: Alternative quit (when Super is grabbed)
        if modifiers.ctrl && modifiers.shift && matches!(keysym, Keysym::q | Keysym::Q) {
            tracing::info!("quit requested (Ctrl+Shift+Q)");
            self.running = false;
            return true;
        }

        // Super (Mod4) + key bindings
        if modifiers.logo {
            match keysym {
                // Super+Q: Quit compositor
                Keysym::q | Keysym::Q => {
                    tracing::info!("quit requested (Super+Q)");
                    self.running = false;
                    return true;
                }

                // Super+J: Focus next window
                Keysym::j | Keysym::J => {
                    self.focus_next();
                    return true;
                }

                // Super+K: Focus previous window
                Keysym::k | Keysym::K => {
                    self.focus_prev();
                    return true;
                }

                // Super+Down: Scroll down
                Keysym::Down => {
                    self.scroll(SCROLL_STEP);
                    return true;
                }

                // Super+Up: Scroll up
                Keysym::Up => {
                    self.scroll(-SCROLL_STEP);
                    return true;
                }

                // Super+Home: Scroll to top
                Keysym::Home => {
                    self.scroll_offset = 0.0;
                    self.recalculate_layout();
                    return true;
                }

                // Super+End: Scroll to bottom
                Keysym::End => {
                    let max_scroll =
                        (self.layout.total_height as f64 - self.output_size.h as f64).max(0.0);
                    self.scroll_offset = max_scroll;
                    self.recalculate_layout();
                    return true;
                }

                _ => {}
            }
        }

        // Page Up/Down without modifiers for scrolling
        match keysym {
            Keysym::Page_Up => {
                self.scroll(-self.output_size.h as f64 * 0.9);
                return true;
            }
            Keysym::Page_Down => {
                self.scroll(self.output_size.h as f64 * 0.9);
                return true;
            }
            _ => {}
        }

        false
    }

    fn handle_pointer_motion<I: InputBackend>(&mut self, _event: impl smithay::backend::input::PointerMotionEvent<I>) {
        // Relative motion handling (for mouse movement)
        // Not critical for initial implementation
    }

    fn handle_pointer_motion_absolute<I: InputBackend>(
        &mut self,
        event: impl AbsolutePositionEvent<I>,
        terminals: &mut TerminalManager,
    ) {
        let output_size = self.output_size;

        // Input from Winit is in screen coordinates (Y=0 at top)
        let screen_x = event.x_transformed(output_size.w);
        let screen_y = ScreenY::new(event.y_transformed(output_size.h));

        // Convert to render coordinates (Y=0 at bottom) for hit detection
        let render_y = screen_y.to_render(output_size.h).value();

        tracing::trace!(
            screen_y = screen_y.value(),
            render_y,
            output_height = output_size.h,
            "pointer motion"
        );

        // Update selection if we're in a drag operation
        if let Some((term_id, cell_render_y, cell_height)) = self.selecting {
            if let Some(managed) = terminals.get(term_id) {
                let (char_width, char_height) = managed.terminal.cell_size();
                let (col, row) = render_to_grid_coords(
                    screen_x,
                    render_y,
                    cell_render_y as f64,
                    cell_height as f64,
                    char_width,
                    char_height,
                );

                managed.terminal.update_selection(col, row);
                tracing::trace!(col, row, "selection updated");
            }
        }

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();

        // Hit detection uses render coordinates (matches our window positions)
        let render_position = Point::from((screen_x, render_y));
        let under = self.surface_under(render_position);

        // Send SCREEN coordinates to clients via pointer.motion
        // Clients expect Y=0 at top, Y increasing downward
        let screen_position = (screen_x, screen_y.value());

        // Debug: show what surface-local coords will be computed
        if let Some((_, surface_pos)) = &under {
            let local_x = screen_x - surface_pos.x;
            let local_y = screen_y.value() - surface_pos.y;
            tracing::debug!(
                screen_x,
                screen_y = screen_y.value(),
                surface_x = surface_pos.x,
                surface_y = surface_pos.y,
                local_x,
                local_y,
                "motion: screen coords and computed surface-local"
            );
        }

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: screen_position.into(),
                serial,
                time: event.time_msec(),
            },
        );

        // Frame event signals end of this event batch to the client
        pointer.frame(self);
    }

    fn handle_pointer_button<I: InputBackend>(
        &mut self,
        event: impl PointerButtonEvent<I>,
        terminals: Option<&mut TerminalManager>,
    ) {
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = event.state();

        let pointer = self.seat.get_pointer().unwrap();

        // Handle left mouse button for selection
        if button == BTN_LEFT && state == ButtonState::Released {
            // End selection drag (selection remains for copying)
            if self.selecting.is_some() {
                tracing::info!("selection drag ended");
                self.selecting = None;
            }
        }

        // Focus window on click
        if state == ButtonState::Pressed {
            // Pointer location from Smithay is in screen coordinates (Y=0 at top)
            // because that's what we send to pointer.motion()
            let screen_location = pointer.current_location();
            let screen_y = ScreenY::new(screen_location.y);

            // Convert to render coordinates (Y=0 at bottom) for hit detection
            let render_y = screen_y.to_render(self.output_size.h);
            let render_location: Point<f64, Logical> = Point::from((screen_location.x, render_y.value()));

            // Log detailed position info for debugging
            tracing::debug!(
                screen_location = ?(screen_location.x, screen_location.y),
                render_location = ?(render_location.x, render_location.y),
                output_size = ?(self.output_size.w, self.output_size.h),
                scroll_offset = self.scroll_offset,
                cell_count = self.cells.len(),
                "handle_pointer_button: click pressed"
            );

            if let Some(index) = self.cell_at(render_location) {
                // Clicked on a cell - focus it
                self.focused_index = Some(index);

                // Extract cell info before doing mutable operations
                let cell_info = match &self.cells[index] {
                    ColumnCell::External(entry) => {
                        Some((true, Some(entry.toplevel.wl_surface().clone()), None))
                    }
                    ColumnCell::Terminal(id) => {
                        Some((false, None, Some(*id)))
                    }
                };

                if let Some((is_external, surface, terminal_id)) = cell_info {
                    if is_external {
                        tracing::info!(
                            index,
                            render_y = render_location.y,
                            "handle_pointer_button: hit external window"
                        );

                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, surface, serial);
                        }
                    } else if let Some(id) = terminal_id {
                        tracing::info!(
                            index,
                            terminal_id = ?id,
                            render_y = render_location.y,
                            "handle_pointer_button: hit terminal"
                        );

                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, None, serial);
                        }

                        // Start selection on left button press
                        if button == BTN_LEFT {
                            if let Some(terminals) = &terminals {
                                if let Some(managed) = terminals.get(id) {
                                    // Calculate cell position for coordinate conversion
                                    let (cell_render_y, cell_height) =
                                        self.get_cell_render_position(index);

                                    let (char_width, char_height) = managed.terminal.cell_size();
                                    let (col, row) = render_to_grid_coords(
                                        render_location.x,
                                        render_location.y,
                                        cell_render_y,
                                        cell_height as f64,
                                        char_width,
                                        char_height,
                                    );

                                    tracing::info!(col, row, "starting selection");

                                    // Clear any previous selection and start new one
                                    managed.terminal.clear_selection();
                                    managed.terminal.start_selection(col, row);

                                    // Store selection state for drag tracking
                                    self.selecting = Some((
                                        id,
                                        cell_render_y as i32,
                                        cell_height,
                                    ));
                                }
                            }
                        }

                        if let Some(terminals) = terminals {
                            terminals.focus(id);
                        }
                    }
                }
            } else {
                tracing::debug!(
                    render_y = render_location.y,
                    screen_y = screen_location.y,
                    "handle_pointer_button: click not on terminal or window"
                );
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time: event.time_msec(),
            },
        );

        // Frame event signals end of this event batch to the client
        pointer.frame(self);
    }

    fn handle_pointer_axis<I: InputBackend>(&mut self, event: impl PointerAxisEvent<I>) {
        let source = event.source();

        // Handle vertical scroll for column navigation
        let vertical = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) / 120.0 * 3.0);

        if vertical != 0.0 {
            // Queue scroll for main loop (uses terminal_manager's total height)
            // Positive vertical = wheel down = scroll content down (increase offset)
            self.scroll_requested += vertical * SCROLL_WHEEL_MULTIPLIER;
            tracing::info!(
                vertical,
                scroll_requested = self.scroll_requested,
                "handle_pointer_axis: scroll event"
            );
        }

        // Forward horizontal scroll to clients
        let horizontal = event
            .amount(Axis::Horizontal)
            .unwrap_or_else(|| event.amount_v120(Axis::Horizontal).unwrap_or(0.0) / 120.0 * 3.0);

        let pointer = self.seat.get_pointer().unwrap();

        let mut frame = AxisFrame::new(event.time_msec()).source(source);

        if horizontal != 0.0 {
            frame = frame.value(Axis::Horizontal, horizontal);
        }

        // Note: we don't forward vertical scroll to clients since we use it for column scroll
        // In a more sophisticated implementation, we might forward to focused window instead

        pointer.axis(self, frame);

        // Frame event signals end of this event batch to the client
        pointer.frame(self);
    }

    /// Find the surface under a point (only for external windows)
    ///
    /// `point` is in RENDER coordinates (Y=0 at bottom, for OpenGL).
    fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<f64, Logical>)> {
        // Find which cell is under the point (only external windows have surfaces)
        let index = self.window_at(point)?;

        let crate::state::ColumnCell::External(entry) = &self.cells[index] else {
            return None;
        };

        tracing::debug!(
            render_point = ?point,
            scroll_offset = self.scroll_offset,
            cached_heights = ?self.cached_cell_heights,
            cell_count = self.cells.len(),
            hit_index = index,
            "surface_under: checking point"
        );

        let output_height = self.output_size.h as f64;

        // Calculate the cell's content_y position (Y from top in content space)
        let mut content_y = -self.scroll_offset;
        for (i, &h) in self.cached_cell_heights.iter().enumerate() {
            if i == index {
                break;
            }
            content_y += h as f64;
        }

        let cell_height = self.get_cell_height(index).unwrap_or(0) as f64;

        // With Y-flip, the cell's position in render coordinates:
        // - render_y = output_height - content_y - height (bottom of cell in render)
        // - render_end = output_height - content_y (top of cell in render)
        //
        // For client-local coordinates (Y=0 at top of window):
        // - Client Y=0 corresponds to render_end (top of cell in render coords)
        // - client_local_y = render_end - point.y = (output_height - content_y) - point.y
        let render_end = output_height - content_y;
        let relative_x = point.x;
        let relative_y = render_end - point.y;
        let relative_point: Point<f64, Logical> = Point::from((relative_x, relative_y));

        tracing::debug!(
            index,
            content_y,
            cell_height,
            render_end,
            relative_y,
            relative_point = ?(relative_point.x, relative_point.y),
            "surface_under: calculated relative point"
        );

        let result = entry
            .window
            .surface_under(relative_point, smithay::desktop::WindowSurfaceType::ALL);

        tracing::debug!(
            found_surface = result.is_some(),
            surface_point = ?result.as_ref().map(|(_, pt)| (pt.x, pt.y)),
            "surface_under: result"
        );

        // Return surface position in SCREEN coordinates (Y=0 at top)
        // This must match the coordinate system of MotionEvent.location
        //
        // The cell's top in screen coords: screen_y = output_height - render_end
        //                                          = output_height - (output_height - content_y)
        //                                          = content_y
        // But with scroll, content_y can be negative (scrolled off top)
        let screen_surface_y = content_y;

        result.map(|(surface, _pt)| (surface, Point::from((0.0, screen_surface_y))))
    }
}

/// Convert a keysym to bytes for sending to a terminal
fn keysym_to_bytes(keysym: Keysym, modifiers: &ModifiersState) -> Vec<u8> {
    // Filter out modifier-only keys (they don't produce characters)
    match keysym {
        Keysym::Shift_L | Keysym::Shift_R |
        Keysym::Control_L | Keysym::Control_R |
        Keysym::Alt_L | Keysym::Alt_R |
        Keysym::Super_L | Keysym::Super_R |
        Keysym::Meta_L | Keysym::Meta_R |
        Keysym::Caps_Lock | Keysym::Num_Lock | Keysym::Scroll_Lock |
        Keysym::ISO_Level3_Shift | Keysym::ISO_Level5_Shift |
        Keysym::Hyper_L | Keysym::Hyper_R => {
            return vec![];
        }
        _ => {}
    }

    // Handle control characters
    if modifiers.ctrl {
        let c = match keysym {
            Keysym::a | Keysym::A => Some(1),   // Ctrl+A
            Keysym::b | Keysym::B => Some(2),   // Ctrl+B
            Keysym::c | Keysym::C => Some(3),   // Ctrl+C
            Keysym::d | Keysym::D => Some(4),   // Ctrl+D
            Keysym::e | Keysym::E => Some(5),   // Ctrl+E
            Keysym::f | Keysym::F => Some(6),   // Ctrl+F
            Keysym::g | Keysym::G => Some(7),   // Ctrl+G
            Keysym::h | Keysym::H => Some(8),   // Ctrl+H (backspace)
            Keysym::i | Keysym::I => Some(9),   // Ctrl+I (tab)
            Keysym::j | Keysym::J => Some(10),  // Ctrl+J (newline)
            Keysym::k | Keysym::K => Some(11),  // Ctrl+K
            Keysym::l | Keysym::L => Some(12),  // Ctrl+L
            Keysym::m | Keysym::M => Some(13),  // Ctrl+M (carriage return)
            Keysym::n | Keysym::N => Some(14),  // Ctrl+N
            Keysym::o | Keysym::O => Some(15),  // Ctrl+O
            Keysym::p | Keysym::P => Some(16),  // Ctrl+P
            Keysym::q | Keysym::Q => Some(17),  // Ctrl+Q
            Keysym::r | Keysym::R => Some(18),  // Ctrl+R
            Keysym::s | Keysym::S => Some(19),  // Ctrl+S
            Keysym::t | Keysym::T => Some(20),  // Ctrl+T
            Keysym::u | Keysym::U => Some(21),  // Ctrl+U
            Keysym::v | Keysym::V => Some(22),  // Ctrl+V
            Keysym::w | Keysym::W => Some(23),  // Ctrl+W
            Keysym::x | Keysym::X => Some(24),  // Ctrl+X
            Keysym::y | Keysym::Y => Some(25),  // Ctrl+Y
            Keysym::z | Keysym::Z => Some(26),  // Ctrl+Z
            Keysym::bracketleft => Some(27),    // Ctrl+[ (escape)
            Keysym::backslash => Some(28),      // Ctrl+\
            Keysym::bracketright => Some(29),   // Ctrl+]
            Keysym::asciicircum => Some(30),    // Ctrl+^
            Keysym::underscore => Some(31),     // Ctrl+_
            _ => None,
        };
        if let Some(byte) = c {
            return vec![byte];
        }
    }

    // Handle special keys
    match keysym {
        Keysym::Return => vec![b'\r'],
        Keysym::BackSpace => vec![0x7f],  // DEL
        Keysym::Tab => vec![b'\t'],
        Keysym::Escape => vec![0x1b],
        Keysym::space => vec![b' '],

        // Arrow keys (send escape sequences)
        Keysym::Up => vec![0x1b, b'[', b'A'],
        Keysym::Down => vec![0x1b, b'[', b'B'],
        Keysym::Right => vec![0x1b, b'[', b'C'],
        Keysym::Left => vec![0x1b, b'[', b'D'],

        // Home/End
        Keysym::Home => vec![0x1b, b'[', b'H'],
        Keysym::End => vec![0x1b, b'[', b'F'],

        // Page Up/Down
        Keysym::Page_Up => vec![0x1b, b'[', b'5', b'~'],
        Keysym::Page_Down => vec![0x1b, b'[', b'6', b'~'],

        // Insert/Delete
        Keysym::Insert => vec![0x1b, b'[', b'2', b'~'],
        Keysym::Delete => vec![0x1b, b'[', b'3', b'~'],

        // Function keys
        Keysym::F1 => vec![0x1b, b'O', b'P'],
        Keysym::F2 => vec![0x1b, b'O', b'Q'],
        Keysym::F3 => vec![0x1b, b'O', b'R'],
        Keysym::F4 => vec![0x1b, b'O', b'S'],
        Keysym::F5 => vec![0x1b, b'[', b'1', b'5', b'~'],
        Keysym::F6 => vec![0x1b, b'[', b'1', b'7', b'~'],
        Keysym::F7 => vec![0x1b, b'[', b'1', b'8', b'~'],
        Keysym::F8 => vec![0x1b, b'[', b'1', b'9', b'~'],
        Keysym::F9 => vec![0x1b, b'[', b'2', b'0', b'~'],
        Keysym::F10 => vec![0x1b, b'[', b'2', b'1', b'~'],
        Keysym::F11 => vec![0x1b, b'[', b'2', b'3', b'~'],
        Keysym::F12 => vec![0x1b, b'[', b'2', b'4', b'~'],

        // Regular characters
        _ => {
            // Try to get the UTF-8 representation
            let raw = keysym.raw();
            if (0x20..0x7f).contains(&raw) {
                // ASCII printable
                vec![raw as u8]
            } else if raw >= 0x100 {
                // Unicode - convert to UTF-8
                if let Some(c) = char::from_u32(raw) {
                    let mut buf = [0u8; 4];
                    c.encode_utf8(&mut buf).as_bytes().to_vec()
                } else {
                    vec![]
                }
            } else {
                vec![]
            }
        }
    }
}
