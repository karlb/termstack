//! Input handling for keyboard and scroll events

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

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
            InputEvent::PointerButton { event } => self.handle_pointer_button(event),
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
            InputEvent::PointerAxis { .. } => tracing::trace!("INPUT: PointerAxis"),
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
            InputEvent::PointerButton { event } => self.handle_pointer_button(event),
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

        tracing::info!(?keycode, ?key_state, "processing keyboard event");

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
                    // Get focused terminal (first one for now)
                    let ids = terminals.ids();
                    tracing::info!(?ids, "terminal ids");
                    if let Some(&id) = ids.first() {
                        if let Some(terminal) = terminals.get_mut(id) {
                            tracing::info!(?bytes, "writing to terminal");
                            let _ = terminal.write(&bytes);
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
        // Use the regular binding handler first
        if self.handle_compositor_binding(modifiers, keysym, state) {
            return true;
        }

        // Additional bindings for terminal management
        if state != KeyState::Pressed {
            return false;
        }

        if modifiers.logo {
            match keysym {
                // Super+Return: Signal to spawn new terminal (handled in main loop)
                Keysym::Return => {
                    self.spawn_terminal_requested = true;
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

        // Super (Mod4) + key bindings
        if modifiers.logo {
            match keysym {
                // Super+Q: Quit compositor
                Keysym::q | Keysym::Q => {
                    tracing::info!("quit requested");
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
        let position = (
            event.x_transformed(output_size.w),
            event.y_transformed(output_size.h),
        );

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
    }

    fn handle_pointer_button<I: InputBackend>(&mut self, event: impl PointerButtonEvent<I>) {
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = event.state();

        let pointer = self.seat.get_pointer().unwrap();

        // Focus window on click
        if state == ButtonState::Pressed {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let pointer_location = pointer.current_location();
                if let Some(index) = self.window_at(pointer_location) {
                    self.focused_index = Some(index);
                    if let Some(entry) = self.windows.get(index) {
                        keyboard.set_focus(
                            self,
                            Some(entry.toplevel.wl_surface().clone()),
                            serial,
                        );
                    }
                }
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state: state.into(),
                serial,
                time: event.time_msec(),
            },
        );
    }

    fn handle_pointer_axis<I: InputBackend>(&mut self, event: impl PointerAxisEvent<I>) {
        let source = event.source();

        // Handle vertical scroll for column navigation
        let vertical = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) / 120.0 * 3.0);

        if vertical != 0.0 {
            // Scroll the column view
            self.scroll(vertical * SCROLL_WHEEL_MULTIPLIER);
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
    }

    /// Find the surface under a point
    fn surface_under(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<f64, Logical>)> {
        // Find which window is under the point
        let index = self.window_at(point)?;
        let entry = self.windows.get(index)?;
        let pos = self.layout.window_positions.get(index)?;

        // Get the surface at the relative position within the window
        let relative_point: Point<f64, Logical> = Point::from((point.x, point.y - pos.y as f64));

        entry
            .window
            .surface_under(relative_point, smithay::desktop::WindowSurfaceType::ALL)
            .map(|(surface, pt)| (surface, Point::from((pt.x as f64, pt.y as f64))))
    }
}

/// Convert a keysym to bytes for sending to a terminal
fn keysym_to_bytes(keysym: Keysym, modifiers: &ModifiersState) -> Vec<u8> {
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
            if raw >= 0x20 && raw < 0x7f {
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
