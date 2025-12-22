//! Input handling for keyboard and scroll events

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::coords::{RenderY, ScreenPoint};
use crate::state::ColumnCompositor;
use crate::terminal_manager::TerminalManager;

/// Scroll amount per key press (pixels)
const SCROLL_STEP: f64 = 50.0;

/// Scroll amount per scroll wheel tick (pixels)
const SCROLL_WHEEL_MULTIPLIER: f64 = 15.0;

impl ColumnCompositor {
    /// Process an input event (legacy, no terminal support)
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => self.handle_keyboard_event(event, None),
            InputEvent::PointerMotion { event } => self.handle_pointer_motion(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_motion_absolute(event)
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button(event, None),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis(event),
            _ => {}
        }
    }

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
                self.handle_pointer_motion_absolute(event)
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
        if self.external_window_focused {
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

        // Forward to focused terminal if we got bytes
        if let Some((handled, Some(bytes))) = result {
            if !handled {
                if let Some(terminals) = terminals {
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
    ) {
        let output_size = self.output_size;

        // Input from Winit is in screen coordinates (Y=0 at top)
        let screen_point = ScreenPoint::new(
            event.x_transformed(output_size.w),
            event.y_transformed(output_size.h),
        );

        // Convert to render coordinates (Y=0 at bottom, for OpenGL/Smithay)
        // This is the canonical Y-flip between screen and render coordinate systems
        let render_point = screen_point.to_render(output_size.h);

        tracing::trace!(
            screen_y = screen_point.y.value(),
            render_y = render_point.y.value(),
            output_height = output_size.h,
            "screen -> render Y conversion"
        );

        // Convert to tuple for Smithay APIs
        let position = (render_point.x, render_point.y.value());

        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();

        let under = self.surface_under(Point::from(position));

        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: position.into(),
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

        // Focus window on click
        if state == ButtonState::Pressed {
            let pointer_location = pointer.current_location();

            // Log detailed position info for debugging
            tracing::debug!(
                pointer_location = ?(pointer_location.x, pointer_location.y),
                output_size = ?(self.output_size.w, self.output_size.h),
                terminal_height = self.terminal_total_height,
                scroll_offset = self.scroll_offset,
                window_count = self.windows.len(),
                "handle_pointer_button: click pressed"
            );

            if let Some(index) = self.window_at(pointer_location) {
                // Clicked on an external Wayland window
                tracing::info!(
                    index,
                    pointer_y = pointer_location.y,
                    "handle_pointer_button: hit window"
                );

                self.focused_index = Some(index);
                self.external_window_focused = true;

                if let Some(keyboard) = self.seat.get_keyboard() {
                    if let Some(entry) = self.windows.get(index) {
                        keyboard.set_focus(
                            self,
                            Some(entry.toplevel.wl_surface().clone()),
                            serial,
                        );
                        tracing::info!(index, "focused external window");
                    }
                }
            } else if self.is_on_terminal(pointer_location) {
                // Clicked on an internal terminal
                self.external_window_focused = false;

                // Clear keyboard focus from external windows
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, None, serial);
                }

                // Focus the clicked terminal
                // pointer_location.y is in render coordinates (Y=0 at bottom)
                if let Some(terminals) = terminals {
                    let render_y = RenderY::new(pointer_location.y);
                    if let Some(id) = terminals.terminal_at_y(render_y, self.scroll_offset) {
                        terminals.focus(id);
                        tracing::info!(?id, "focused internal terminal");
                    }
                }
            } else {
                tracing::debug!(
                    pointer_y = pointer_location.y,
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
            // Negate because we flipped Y coordinates for OpenGL compatibility
            self.scroll_requested -= vertical * SCROLL_WHEEL_MULTIPLIER;
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

    /// Find the surface under a point
    fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<f64, Logical>)> {
        // Find which window is under the point
        let index = self.window_at(point);

        tracing::info!(
            screen_point = ?point,
            terminal_height = self.terminal_total_height,
            scroll_offset = self.scroll_offset,
            cached_heights = ?self.cached_window_heights,
            window_count = self.windows.len(),
            hit_index = ?index,
            "surface_under: checking point"
        );

        let index = index?;
        let entry = self.windows.get(index)?;

        // Log window geometry info
        let bbox = entry.window.bbox();
        let geometry = entry.window.geometry();
        tracing::info!(
            index,
            bbox = ?(bbox.loc.x, bbox.loc.y, bbox.size.w, bbox.size.h),
            geometry = ?(geometry.loc.x, geometry.loc.y, geometry.size.w, geometry.size.h),
            "surface_under: window geometry"
        );

        // Calculate the window's screen Y position using cached heights
        let terminal_height = self.terminal_total_height as f64;
        let mut window_y = terminal_height - self.scroll_offset;
        for (i, &h) in self.cached_window_heights.iter().enumerate() {
            if i == index {
                break;
            }
            window_y += h as f64;
        }

        let window_height = self.cached_window_heights.get(index).copied().unwrap_or(0);

        // Calculate relative position within the window.
        // Note: We flip the SOURCE during rendering to correct for OpenGL's Y-up,
        // but the DESTINATION positions (element geometry) remain unchanged.
        // Hit detection uses destination geometry, so no flip is needed here.
        //
        // surface_under expects coordinates relative to the window's origin.
        // Our windows are positioned at screen Y = window_y, so:
        let relative_x = point.x;
        let relative_y = point.y - window_y;
        let relative_point: Point<f64, Logical> = Point::from((relative_x, relative_y));

        tracing::info!(
            index,
            window_y,
            window_height,
            geometry_offset = ?(geometry.loc.x, geometry.loc.y),
            relative_point = ?(relative_point.x, relative_point.y),
            "surface_under: calculated relative point"
        );

        let result = entry
            .window
            .surface_under(relative_point, smithay::desktop::WindowSurfaceType::ALL);

        tracing::info!(
            found_surface = result.is_some(),
            surface_point = ?result.as_ref().map(|(_, pt)| (pt.x, pt.y)),
            "surface_under: result"
        );

        result.map(|(surface, pt)| (surface, Point::from((pt.x as f64, pt.y as f64))))
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
