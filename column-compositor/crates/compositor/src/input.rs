//! Input handling for keyboard and scroll events

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::state::ColumnCompositor;

/// Scroll amount per key press (pixels)
const SCROLL_STEP: f64 = 50.0;

/// Scroll amount per scroll wheel tick (pixels)
const SCROLL_WHEEL_MULTIPLIER: f64 = 15.0;

impl ColumnCompositor {
    /// Process an input event
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => self.handle_keyboard_event(event),
            InputEvent::PointerMotion { event } => self.handle_pointer_motion(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_motion_absolute(event)
            }
            InputEvent::PointerButton { event } => self.handle_pointer_button(event),
            InputEvent::PointerAxis { event } => self.handle_pointer_axis(event),
            _ => {}
        }
    }

    fn handle_keyboard_event<I: InputBackend>(&mut self, event: impl KeyboardKeyEvent<I>) {
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);
        let keycode = event.key_code();
        let state = event.state();

        let keyboard = self.seat.get_keyboard().unwrap();

        // Process through keyboard for modifier tracking and client delivery
        keyboard.input::<(), _>(
            self,
            keycode,
            state,
            serial,
            time,
            |state, modifiers, keysym| {
                // Handle compositor keybindings
                if state.handle_compositor_binding(modifiers, keysym.modified_sym(), event.state())
                {
                    FilterResult::Intercept(())
                } else {
                    FilterResult::Forward
                }
            },
        );
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
