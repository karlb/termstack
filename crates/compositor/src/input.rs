//! Input handling for keyboard and scroll events
//!
//! # Responsibilities
//!
//! - Keyboard event processing (keybindings, focus, terminal input)
//! - Pointer event handling (clicks, scrolling, dragging)
//! - Coordinate conversion from screen to render space (Y-flip)
//! - Window hit testing and click detection
//! - Resize drag state management
//! - Text selection in terminals
//!
//! # NOT Responsible For
//!
//! - State mutations (delegates to `state.rs` methods)
//! - Layout calculation (uses `layout.rs` for positions)
//! - Terminal content (delegates to `terminal_manager/`)

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, Event, InputBackend, InputEvent,
    KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::input::keyboard::{FilterResult, Keysym, ModifiersState};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use crate::coords::{RenderY, ScreenY};
use crate::render::FOCUS_INDICATOR_WIDTH;
use crate::state::{StackWindow, TermStack, ResizeDrag, SurfaceKind, MIN_WINDOW_HEIGHT};
use crate::terminal_manager::TerminalManager;
use crate::title_bar::{CLOSE_BUTTON_WIDTH, TITLE_BAR_HEIGHT};

use crate::terminal_manager::TerminalId;

/// Left mouse button code (BTN_LEFT in evdev)
const BTN_LEFT: u32 = 0x110;

/// Scroll amount per key press (pixels)
const SCROLL_STEP: f64 = 50.0;

/// Scroll amount per scroll wheel tick (pixels)
const SCROLL_WHEEL_MULTIPLIER: f64 = 15.0;

/// Check if a click is on the close button in a title bar
fn is_click_on_close_button(
    render_y: f64,
    window_render_top: f64,
    click_x: f64,
    output_width: i32,
    has_ssd: bool,
    button: u32,
) -> bool {
    if !has_ssd || button != BTN_LEFT {
        return false;
    }

    let title_bar_top = window_render_top;
    let title_bar_bottom = title_bar_top - TITLE_BAR_HEIGHT as f64;
    let in_title_bar = render_y <= title_bar_top && render_y > title_bar_bottom;

    let close_btn_left = (output_width - CLOSE_BUTTON_WIDTH as i32) as f64;
    let in_close_btn = click_x >= close_btn_left;

    in_title_bar && in_close_btn
}

/// Convert render coordinates to terminal grid coordinates (col, row)
///
/// - `render_x`, `render_y`: Position in render coordinates (Y=0 at bottom)
/// - `window_render_y`: The terminal cell's render Y position (bottom of cell)
/// - `window_height`: The terminal cell's height in pixels
/// - `char_width`, `char_height`: Character cell dimensions from the font
fn render_to_grid_coords(
    render_x: f64,
    render_y: f64,
    window_render_y: f64,
    window_height: f64,
    char_width: u32,
    char_height: u32,
) -> (usize, usize) {
    // Convert render coords to terminal-local coords
    // Terminal has Y=0 at top, render has Y=0 at bottom
    // Content is offset by FOCUS_INDICATOR_WIDTH from left edge
    let window_render_end = window_render_y + window_height;
    let local_y = (window_render_end - render_y).max(0.0);
    let local_x = (render_x - FOCUS_INDICATOR_WIDTH as f64).max(0.0);

    // Convert to grid coordinates
    let col = (local_x / char_width as f64) as usize;
    let row = (local_y / char_height as f64) as usize;

    (col, row)
}

/// Start a text selection on a terminal at the given render coordinates
///
/// Returns the selection tracking state (terminal id, cell render y, cell height)
/// which should be stored in `compositor.selecting` for drag tracking.
fn start_terminal_selection(
    compositor: &TermStack,
    terminals: &mut TerminalManager,
    terminal_id: TerminalId,
    window_index: usize,
    render_x: f64,
    render_y: f64,
) -> Option<(TerminalId, i32, i32)> {
    let managed = terminals.get_mut(terminal_id)?;
    let (window_render_y, window_height) = compositor.get_window_render_position(window_index);

    let (char_width, char_height) = managed.terminal.cell_size();
    let (col, row) = render_to_grid_coords(
        render_x,
        render_y,
        window_render_y,
        window_height as f64,
        char_width,
        char_height,
    );

    // Clear any previous selection and start new one
    managed.terminal.clear_selection();
    managed.terminal.start_selection(col, row);
    managed.mark_dirty(); // Re-render to show selection highlight

    Some((terminal_id, window_render_y as i32, window_height))
}

/// Update an ongoing selection during pointer drag
fn update_terminal_selection(
    terminals: &mut TerminalManager,
    terminal_id: TerminalId,
    window_render_y: i32,
    window_height: i32,
    render_x: f64,
    render_y: f64,
) {
    if let Some(managed) = terminals.get_mut(terminal_id) {
        let (char_width, char_height) = managed.terminal.cell_size();
        let (col, row) = render_to_grid_coords(
            render_x,
            render_y,
            window_render_y as f64,
            window_height as f64,
            char_width,
            char_height,
        );

        managed.terminal.update_selection(col, row);
        managed.mark_dirty(); // Re-render to show updated selection
    }
}

/// Compositor keybinding action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompositorAction {
    Quit,
    SpawnTerminal,
    FocusNext,
    FocusPrev,
    ScrollDown,
    ScrollUp,
    ScrollToTop,
    ScrollToBottom,
    PageDown,
    PageUp,
    Copy,
    Paste,
}

/// Parse compositor keybindings from modifiers and keysym
fn parse_compositor_keybinding(modifiers: &ModifiersState, keysym: Keysym) -> Option<CompositorAction> {
    // Ctrl+Shift bindings (work when Super is grabbed by parent compositor)
    if modifiers.ctrl && modifiers.shift && !modifiers.logo && !modifiers.alt {
        return match keysym {
            Keysym::q | Keysym::Q => Some(CompositorAction::Quit),
            Keysym::Return | Keysym::t | Keysym::T => Some(CompositorAction::SpawnTerminal),
            Keysym::j | Keysym::J | Keysym::Down => Some(CompositorAction::FocusNext),
            Keysym::k | Keysym::K | Keysym::Up => Some(CompositorAction::FocusPrev),
            Keysym::Page_Down => Some(CompositorAction::PageDown),
            Keysym::Page_Up => Some(CompositorAction::PageUp),
            Keysym::v | Keysym::V => Some(CompositorAction::Paste),
            Keysym::c | Keysym::C => Some(CompositorAction::Copy),
            _ => None,
        };
    }

    // Super (Mod4) bindings
    if modifiers.logo && !modifiers.ctrl && !modifiers.shift && !modifiers.alt {
        return match keysym {
            Keysym::q | Keysym::Q => Some(CompositorAction::Quit),
            Keysym::Return | Keysym::t | Keysym::T => Some(CompositorAction::SpawnTerminal),
            Keysym::j | Keysym::J => Some(CompositorAction::FocusNext),
            Keysym::k | Keysym::K => Some(CompositorAction::FocusPrev),
            Keysym::Down => Some(CompositorAction::ScrollDown),
            Keysym::Up => Some(CompositorAction::ScrollUp),
            Keysym::Home => Some(CompositorAction::ScrollToTop),
            Keysym::End => Some(CompositorAction::ScrollToBottom),
            _ => None,
        };
    }

    // Page Up/Down without modifiers
    if !modifiers.ctrl && !modifiers.shift && !modifiers.logo && !modifiers.alt {
        return match keysym {
            Keysym::Page_Up => Some(CompositorAction::PageUp),
            Keysym::Page_Down => Some(CompositorAction::PageDown),
            _ => None,
        };
    }

    None
}

impl TermStack {
    /// Process an input event with terminal support
    pub fn process_input_event_with_terminals<I: InputBackend>(
        &mut self,
        event: InputEvent<I>,
        terminals: &mut TerminalManager,
    ) {
        match event {
            InputEvent::Keyboard { event } => self.handle_keyboard_event(event, Some(terminals)),
            InputEvent::PointerMotion { event } => self.handle_pointer_motion(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.handle_pointer_motion_absolute(event, terminals)
            }
            InputEvent::PointerButton { event } => {
                tracing::info!(
                    button = event.button_code(),
                    state = ?event.state(),
                    "PointerButton event received"
                );
                self.handle_pointer_button(event, Some(terminals))
            }
            InputEvent::PointerAxis { event } => self.handle_pointer_axis(event, Some(terminals)),
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

        let keyboard = self.seat.get_keyboard().unwrap();

        // If an external Wayland window has focus, forward events via Wayland protocol
        // Note: When a popup grab is active, events are routed through PopupKeyboardGrab
        if self.is_external_focused() || keyboard.is_grabbed() {
            // Check keyboard grab state
            let has_keyboard_grab = keyboard.is_grabbed();
            let has_pointer_grab = self.seat.get_pointer().map(|p| p.is_grabbed()).unwrap_or(false);
            let focus_surface = keyboard.current_focus();
            let focus_id = focus_surface.as_ref().map(|f| format!("{:?}", f.id()));
            let focus_alive = focus_surface.as_ref().map(|f| f.is_alive()).unwrap_or(false);

            tracing::debug!(
                ?keycode,
                ?key_state,
                has_keyboard_grab,
                has_pointer_grab,
                ?focus_id,
                focus_alive,
                "keyboard event for external window/popup"
            );

            // Process through keyboard - grab will route events appropriately
            // Only intercept essential compositor bindings (focus switch, quit, spawn)
            // All other keys (including PageUp/Down) pass through to the focused window
            let input_result = keyboard.input::<bool, _>(
                self,
                keycode,
                key_state,
                serial,
                time,
                |state, modifiers, keysym| {
                    let sym = keysym.modified_sym();
                    if state.handle_global_compositor_binding(modifiers, sym, key_state) {
                        FilterResult::Intercept(true)
                    } else {
                        // Forward to the focused Wayland surface (or popup via grab)
                        tracing::debug!(?keysym, "forwarding key via keyboard.input()");
                        FilterResult::Forward
                    }
                },
            );

            tracing::debug!(
                ?input_result,
                "keyboard.input() completed"
            );

            return;
        }

        // Process through keyboard for modifier tracking
        let result = keyboard.input::<(bool, Option<Vec<u8>>), _>(
            self,
            keycode,
            key_state,
            serial,
            time,
            |state, modifiers, keysym| {
                let sym = keysym.modified_sym();

                // Handle compositor keybindings
                if state.handle_compositor_binding_with_terminals(modifiers, sym, key_state)
                {
                    FilterResult::Intercept((true, None))
                } else if key_state == KeyState::Pressed {
                    // Convert keysym to bytes for terminal
                    let bytes = keysym_to_bytes(sym, modifiers);
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

        // Handle key release - stop repeat
        if key_state == KeyState::Released {
            self.key_repeat = None;
        }

        // Handle keyboard input and clipboard operations (requires terminal access)
        if let Some(terminals) = terminals {
            // Forward to focused terminal if we got bytes
            if let Some((handled, Some(bytes))) = result {
                if !handled {
                    if let Some(terminal) = terminals.get_focused_mut(self.focused_window.as_ref()) {
                        // Only write if terminal process is still running
                        if terminal.has_exited() {
                            tracing::debug!("ignoring input to exited terminal");
                        } else if let Err(e) = terminal.write(&bytes) {
                            tracing::error!(?e, "failed to write to terminal");
                        } else {
                            // Set up key repeat for this key
                            let repeat_time = std::time::Instant::now()
                                + std::time::Duration::from_millis(self.repeat_delay_ms);
                            self.key_repeat = Some((bytes, repeat_time));
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
                            if let Some(terminal) = terminals.get_focused_mut(self.focused_window.as_ref()) {
                                // Only paste if terminal process is still running
                                if terminal.has_exited() {
                                    tracing::debug!("ignoring paste to exited terminal");
                                } else if let Err(e) = terminal.write(text.as_bytes()) {
                                    tracing::error!(?e, "failed to paste to terminal");
                                } else {
                                    tracing::debug!(len = text.len(), "pasted text to terminal");
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
                    if let Some(terminal) = terminals.get_focused_mut(self.focused_window.as_ref()) {
                        // Prefer selection text, fall back to entire grid content
                        let text = if let Some(selected) = terminal.terminal.selection_text() {
                            tracing::debug!(len = selected.len(), "copying selection to clipboard");
                            selected
                        } else {
                            let lines = terminal.terminal.grid_content();
                            let text = lines.join("\n");
                            tracing::debug!(len = text.len(), "copying entire terminal content to clipboard (no selection)");
                            text
                        };

                        if let Err(e) = clipboard.set_text(text) {
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
        if state != KeyState::Pressed {
            return false;
        }

        let Some(action) = parse_compositor_keybinding(modifiers, keysym) else {
            return false;
        };

        match action {
            CompositorAction::Quit => {
                tracing::info!("quit requested");
                self.running = false;
            }
            CompositorAction::SpawnTerminal => {
                tracing::debug!("spawn terminal binding triggered");
                self.spawn_terminal_requested = true;
            }
            CompositorAction::FocusNext => {
                tracing::debug!("focus next requested");
                self.focus_change_requested = 1;
            }
            CompositorAction::FocusPrev => {
                tracing::debug!("focus prev requested");
                self.focus_change_requested = -1;
            }
            CompositorAction::ScrollDown => {
                self.scroll_requested = SCROLL_STEP;
            }
            CompositorAction::ScrollUp => {
                self.scroll_requested = -SCROLL_STEP;
            }
            CompositorAction::ScrollToTop => {
                self.scroll_to_top();
            }
            CompositorAction::ScrollToBottom => {
                self.scroll_to_bottom();
            }
            CompositorAction::PageDown => {
                self.scroll_requested = self.output_size.h as f64 * 0.9;
            }
            CompositorAction::PageUp => {
                self.scroll_requested = -(self.output_size.h as f64 * 0.9);
            }
            CompositorAction::Copy => {
                tracing::debug!("copy to clipboard requested");
                self.pending_copy = true;
            }
            CompositorAction::Paste => {
                tracing::debug!("paste from clipboard requested");
                self.pending_paste = true;
            }
        }

        true
    }

    /// Handle global compositor bindings that work regardless of focused window type.
    /// These are: quit, focus switch, spawn terminal.
    /// Returns true if the binding was handled.
    fn handle_global_compositor_binding(
        &mut self,
        modifiers: &ModifiersState,
        keysym: Keysym,
        state: KeyState,
    ) -> bool {
        if state != KeyState::Pressed {
            return false;
        }

        let Some(action) = parse_compositor_keybinding(modifiers, keysym) else {
            return false;
        };

        match action {
            CompositorAction::Quit => {
                tracing::debug!("quit requested");
                self.running = false;
            }
            CompositorAction::SpawnTerminal => {
                tracing::debug!("spawn terminal binding triggered");
                self.spawn_terminal_requested = true;
            }
            CompositorAction::FocusNext => {
                tracing::debug!("focus next requested");
                self.focus_change_requested = 1;
            }
            CompositorAction::FocusPrev => {
                tracing::debug!("focus prev requested");
                self.focus_change_requested = -1;
            }
            // Other actions not used in global bindings
            _ => return false,
        }

        true
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

        // INVARIANT: Mouse events arrive in screen coordinates (Y=0 at top).
        // Must convert to render coordinates (Y=0 at bottom) before any layout operations.
        // The Y-flip formula is: render_y = screen_height - screen_y
        let screen_x = event.x_transformed(output_size.w);
        let screen_y = ScreenY::new(event.y_transformed(output_size.h));

        // Convert to render coordinates (Y=0 at bottom) for hit detection
        let render_y = screen_y.to_render(output_size.h).value();

        // Store pointer position for Shift+Scroll (scroll terminal under pointer)
        self.pointer_position = Point::from((screen_x, render_y));

        // Check if pointer is on a resize handle (for cursor change)
        // Do this before checking for active resize drag
        let on_resize_handle = self.find_resize_handle_at(screen_y.value() as i32, &self.cached_actual_heights).is_some();
        self.cursor_on_resize_handle = on_resize_handle || self.resizing.is_some();

        // Handle resize drag if active
        if let Some(drag) = &self.resizing {
            let window_index = drag.window_index;
            let delta = screen_y.value() as i32 - drag.start_screen_y;
            let new_height = (drag.start_height + delta).max(MIN_WINDOW_HEIGHT);

            tracing::trace!(
                window_index,
                screen_y = screen_y.value(),
                delta,
                new_height,
                "resize motion"
            );

            // Get the cell type to determine how to resize
            let window_type = self.layout_nodes.get(window_index).and_then(|node| {
                match &node.cell {
                    StackWindow::Terminal(id) => Some(*id),
                    StackWindow::External(_) => None,
                }
            });

            match window_type {
                Some(id) => {
                    // Terminal - snap to full rows during drag
                    let char_height = terminals.cell_height;
                    if let Some(terminal) = terminals.get_mut(id) {
                        let title_bar = if terminal.show_title_bar { TITLE_BAR_HEIGHT } else { 0 };

                        // Calculate content height and snap to full rows
                        let content_height = (new_height as u32).saturating_sub(title_bar);
                        let rows = (content_height / char_height).max(1);
                        let snapped_content = rows * char_height;
                        let snapped_total = (snapped_content + title_bar) as i32;

                        terminal.resize_to_height(snapped_content, char_height);

                        // Update cached height with snapped value
                        if let Some(node) = self.layout_nodes.get_mut(window_index) {
                            node.height = snapped_total;
                        }
                    }
                }
                None => {
                    // External window - Niri-style approach: no configures during drag
                    // Only send configure when drag ends to prevent overwhelming window
                    // But DO update layout positions for visual feedback

                    // Update drag target for final resize
                    if let Some(drag) = &mut self.resizing {
                        drag.target_height = new_height;
                    }

                    // Update cached height for layout positioning (visual feedback)
                    // Window will render at committed size but be positioned at target size
                    if let Some(node) = self.layout_nodes.get_mut(window_index) {
                        node.height = new_height;
                    }

                    tracing::trace!(
                        window_index,
                        new_height,
                        "drag motion - updating layout positions"
                    );
                }
            }
            // Don't call recalculate_layout() here - main loop handles it
            // Just update node.height and let render use it directly
            return;
        }

        // Update selection if we're in a drag operation
        if let Some((term_id, window_render_y, window_height)) = self.selecting {
            update_terminal_selection(terminals, term_id, window_render_y, window_height, screen_x, render_y);
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
        mut terminals: Option<&mut TerminalManager>,
    ) {
        let serial = SERIAL_COUNTER.next_serial();
        let button = event.button_code();
        let state = event.state();

        // Track button press/release for stale drag detection
        match state {
            ButtonState::Pressed => self.pointer_buttons_pressed = self.pointer_buttons_pressed.saturating_add(1),
            ButtonState::Released => self.pointer_buttons_pressed = self.pointer_buttons_pressed.saturating_sub(1),
        }

        let pointer = self.seat.get_pointer().unwrap();

        // Handle left mouse button release
        if button == BTN_LEFT && state == ButtonState::Released {
            // End resize drag
            if let Some(drag) = self.resizing.take() {
                let window_index = drag.window_index;
                let final_target = drag.target_height as u32;

                if let Some(node) = self.layout_nodes.get(window_index) {
                    match &node.cell {
                        StackWindow::Terminal(id) => {
                            // Terminal: mark dirty to trigger texture re-render at final size
                            if let Some(ref mut tm) = terminals {
                                if let Some(term) = tm.get_mut(*id) {
                                    term.mark_dirty();
                                }
                            }
                        }
                        StackWindow::External(_) => {
                            // External window: send final configure
                            if final_target > 0 {
                                self.request_resize(window_index, final_target, true);
                            }
                        }
                    }
                }
                return;
            }

            // End selection drag (selection remains for copying)
            if self.selecting.is_some() {
                self.selecting = None;
            }
        }

        // Focus window on click
        if state == ButtonState::Pressed {
            // Use self.pointer_position which is updated on every motion event
            // pointer.current_location() can be stale if no motion happened since last button press
            let screen_x = self.pointer_position.x;
            let render_y = self.pointer_position.y;

            // Convert render Y back to screen Y for resize handle detection
            let screen_y = RenderY::new(render_y).to_screen(self.output_size.h);
            let render_location: Point<f64, Logical> = Point::from((screen_x, render_y));

            // Check for resize handle before normal cell hit detection
            if button == BTN_LEFT {
                if let Some(window_index) = self.find_resize_handle_at(screen_y.value() as i32, &self.cached_actual_heights) {
                    // Start resize drag
                    let raw_height = self.get_window_height(window_index).unwrap_or(100);

                    // For terminals, snap start_height to row boundary to prevent jump
                    let start_height = if let Some(node) = self.layout_nodes.get(window_index) {
                        if let StackWindow::Terminal(id) = &node.cell {
                            if let Some(ref mut tm) = terminals {
                                let char_height = tm.cell_height;
                                if let Some(term) = tm.get_mut(*id) {
                                    let title_bar = if term.show_title_bar { TITLE_BAR_HEIGHT } else { 0 };
                                    let content = (raw_height as u32).saturating_sub(title_bar);
                                    let rows = (content / char_height).max(1);
                                    let snapped_content = rows * char_height;
                                    let snapped_total = (snapped_content + title_bar) as i32;

                                    // Apply snap to terminal immediately
                                    if snapped_total != raw_height {
                                        term.resize_to_height(snapped_content, char_height);
                                    }

                                    snapped_total
                                } else {
                                    raw_height
                                }
                            } else {
                                raw_height
                            }
                        } else {
                            raw_height
                        }
                    } else {
                        raw_height
                    };

                    self.resizing = Some(ResizeDrag {
                        window_index,
                        start_screen_y: screen_y.value() as i32,
                        start_height,
                        last_configure_time: std::time::Instant::now(),
                        target_height: start_height,
                        last_sent_height: None,
                    });

                    // Update node.height immediately if we snapped (for terminals)
                    if start_height != raw_height {
                        if let Some(node) = self.layout_nodes.get_mut(window_index) {
                            node.height = start_height;
                        }
                    }

                    // Clear any pending external_window_resized for this cell
                    // to prevent stale resize events from overwriting our drag updates
                    if self.external_window_resized.as_ref().map(|(idx, _)| *idx) == Some(window_index) {
                        self.external_window_resized = None;
                        tracing::debug!(window_index, "cleared pending external_window_resized on drag start");
                    }
                    return; // Don't process as normal click
                }
            }

            if let Some(index) = self.window_at(render_location) {
                // Clicked on a cell - focus it
                self.set_focus_by_index(index);

                // Calculate cell's render position for close button detection
                let window_render_top = {
                    let screen_height = self.output_size.h as f64;
                    let mut content_y = -self.scroll_offset;
                    for node in self.layout_nodes.iter().take(index) {
                        content_y += node.height as f64;
                    }
                    // render_y is bottom of cell, render_top is top
                    screen_height - content_y
                };

                // Extract cell info before doing mutable operations
                // For terminals, check if they have a title bar
                let terminal_has_title_bar = if let StackWindow::Terminal(id) = &self.layout_nodes[index].cell {
                    terminals.as_ref()
                        .and_then(|tm| tm.get(*id))
                        .map(|t| t.show_title_bar)
                        .unwrap_or(false)
                } else {
                    false
                };

                // Extract cell info for click handling
                enum CellClickInfo<'a> {
                    External {
                        surface: &'a SurfaceKind,
                        has_ssd: bool,
                    },
                    Terminal {
                        id: TerminalId,
                        has_ssd: bool,
                    },
                }

                let window_info = match &self.layout_nodes[index].cell {
                    StackWindow::External(entry) => {
                        CellClickInfo::External {
                            surface: &entry.surface,
                            has_ssd: !entry.uses_csd,
                        }
                    }
                    StackWindow::Terminal(id) => {
                        CellClickInfo::Terminal {
                            id: *id,
                            has_ssd: terminal_has_title_bar,
                        }
                    }
                };

                match window_info {
                    CellClickInfo::External { surface, has_ssd } => {
                        // Check if click is on close button in title bar
                        if is_click_on_close_button(
                            render_location.y,
                            window_render_top,
                            render_location.x,
                            self.output_size.w,
                            has_ssd,
                            button,
                        ) {
                            tracing::debug!(index, "close button clicked, sending close");
                            surface.send_close();
                            return; // Don't process further
                        }

                        // Update keyboard focus (handles both Wayland and X11)
                        self.update_keyboard_focus_for_focused_window();
                    }
                    CellClickInfo::Terminal { id, has_ssd } => {
                        // Check if click is on close button in title bar (for terminals with title bars)
                        if is_click_on_close_button(
                            render_location.y,
                            window_render_top,
                            render_location.x,
                            self.output_size.w,
                            has_ssd,
                            button,
                        ) {
                            tracing::debug!(index, terminal_id = ?id, "close button clicked on terminal, removing");
                            // Remove the terminal from the layout
                            self.layout_nodes.remove(index);
                            // Remove from terminal manager
                            if let Some(terminals) = terminals {
                                terminals.remove(id);
                            }
                            // Update focus if the focused cell was removed
                            self.update_focus_after_removal(index);
                            return; // Don't process further
                        }

                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, None, serial);
                        }
                        // Deactivate all external windows when focusing terminal
                        self.deactivate_all_toplevels();

                        // Start selection on left button press
                        if button == BTN_LEFT {
                            if let Some(terminals) = &mut terminals {
                                self.selecting = start_terminal_selection(
                                    self,
                                    terminals,
                                    id,
                                    index,
                                    render_location.x,
                                    render_location.y,
                                );
                            }
                        }
                    }
                }
            } else {
                // Click not on any cell - check if it's below all cells
                // If so, focus the last cell
                if !self.layout_nodes.is_empty() {
                    // Calculate the bottom edge of the last cell in render coords
                    let screen_height = self.output_size.h as f64;
                    let mut content_y = -self.scroll_offset;

                    for node in &self.layout_nodes {
                        content_y += node.height as f64;
                    }

                    // Last cell's bottom in render coords: screen_height - content_y
                    let last_window_bottom = screen_height - content_y;

                    // If click is below the last cell's bottom, focus the last cell
                    if render_location.y < last_window_bottom {
                        let last_index = self.layout_nodes.len() - 1;
                        self.set_focus_by_index(last_index);

                        tracing::debug!(
                            render_y = render_location.y,
                            last_window_bottom,
                            last_index,
                            "clicked below all cells, focusing last cell"
                        );
                    } else {
                        tracing::debug!(
                            render_y = render_location.y,
                            screen_y = screen_y.value(),
                            "handle_pointer_button: click not on terminal or window"
                        );
                    }
                } else {
                    tracing::debug!(
                        render_y = render_location.y,
                        screen_y = screen_y.value(),
                        "handle_pointer_button: click not on terminal or window"
                    );
                }
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

    fn handle_pointer_axis<I: InputBackend>(
        &mut self,
        event: impl PointerAxisEvent<I>,
        terminals: Option<&mut TerminalManager>,
    ) {
        let source = event.source();

        // Handle vertical scroll
        let vertical = event
            .amount(Axis::Vertical)
            .unwrap_or_else(|| event.amount_v120(Axis::Vertical).unwrap_or(0.0) / 120.0 * 3.0);

        if vertical != 0.0 {
            // Get current modifier state directly from keyboard
            let shift_held = self.seat.get_keyboard()
                .map(|kb| kb.modifier_state().shift)
                .unwrap_or(false);

            if shift_held {
                // Shift+Scroll: Terminal scrollback navigation
                // Scroll the terminal under the pointer, not the focused one
                if let Some(terminals) = terminals {
                    // Find which cell is under the pointer
                    if let Some(window_idx) = self.window_at(self.pointer_position) {
                        if let Some(StackWindow::Terminal(term_id)) = self.layout_nodes.get(window_idx).map(|n| &n.cell) {
                            if let Some(term) = terminals.get_mut(*term_id) {
                                // Convert scroll amount to lines
                                // Positive vertical = wheel down = scroll toward newer output
                                // We negate because scroll_display positive = scroll up (into history)
                                let lines = (vertical * 3.0).round() as i32;
                                term.terminal.scroll_display(-lines);
                                term.mark_dirty(); // Mark for re-render
                                tracing::debug!(
                                    vertical,
                                    lines,
                                    offset = term.terminal.display_offset(),
                                    "terminal scrollback (Shift+Scroll)"
                                );
                            }
                        }
                    }
                }
            } else {
                // Regular scroll: Compositor column navigation
                // Positive vertical = wheel down = scroll content down (increase offset)
                self.scroll_requested += vertical * SCROLL_WHEEL_MULTIPLIER;
                tracing::info!(
                    vertical,
                    scroll_requested = self.scroll_requested,
                    "handle_pointer_axis: compositor scroll"
                );
            }
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
        // First check all popups (they're on top of windows)
        // We need to check popups for ALL external windows, not just the one under the point
        for (idx, node) in self.layout_nodes.iter().enumerate() {
            if let crate::state::StackWindow::External(entry) = &node.cell {
                // Calculate window position
                let output_height = self.output_size.h as f64;
                let mut content_y = -self.scroll_offset;
                for (i, n) in self.layout_nodes.iter().enumerate() {
                    if i == idx {
                        break;
                    }
                    content_y += n.height as f64;
                }
                let window_render_top = output_height - content_y;

                // Check popups for this window
                let wl_surface = entry.surface.wl_surface();
                for (popup_kind, popup_offset) in smithay::desktop::PopupManager::popups_for_surface(wl_surface) {
                    let popup_surface = match &popup_kind {
                        smithay::desktop::PopupKind::Xdg(xdg_popup) => xdg_popup,
                        _ => continue,
                    };

                    let popup_geo = popup_surface.with_pending_state(|state| state.geometry);

                    // Calculate popup position in render coords
                    // Popup offset is relative to the client area
                    // For CSD apps, client area is the whole window
                    // For SSD apps, client area is below our title bar
                    let popup_render_x = (popup_offset.x + FOCUS_INDICATOR_WIDTH) as f64;
                    let title_bar_offset = if entry.uses_csd { 0.0 } else { TITLE_BAR_HEIGHT as f64 };
                    let client_area_top = window_render_top - title_bar_offset;
                    let popup_render_y = client_area_top - popup_offset.y as f64 - popup_geo.size.h as f64;
                    let popup_w = popup_geo.size.w as f64;
                    let popup_h = popup_geo.size.h as f64;

                    // Check if point is inside popup
                    if point.x >= popup_render_x && point.x < popup_render_x + popup_w
                        && point.y >= popup_render_y && point.y < popup_render_y + popup_h
                    {
                        // Point is on popup - return popup surface
                        let local_x = point.x - popup_render_x;
                        let local_y = (popup_render_y + popup_h) - point.y; // Flip Y for client coords

                        tracing::debug!(
                            popup_offset = ?popup_offset,
                            popup_geo = ?popup_geo,
                            local = ?(local_x, local_y),
                            "surface_under: hit popup"
                        );

                        let wl_surface = popup_surface.wl_surface().clone();
                        // Return popup position in screen coords
                        let screen_popup_y = output_height - (popup_render_y + popup_h);
                        return Some((wl_surface, Point::from((popup_render_x, screen_popup_y))));
                    }
                }
            }
        }

        // No popup hit, check main window
        let index = self.window_at(point)?;

        let crate::state::StackWindow::External(entry) = &self.layout_nodes[index].cell else {
            return None;
        };

        tracing::debug!(
            render_point = ?point,
            scroll_offset = self.scroll_offset,
            window_count = self.layout_nodes.len(),
            hit_index = index,
            "surface_under: checking point"
        );

        let output_height = self.output_size.h as f64;

        // Calculate the cell's content_y position (Y from top in content space)
        let mut content_y = -self.scroll_offset;
        for (i, node) in self.layout_nodes.iter().enumerate() {
            if i == index {
                break;
            }
            content_y += node.height as f64;
        }

        let window_height = self.get_window_height(index).unwrap_or(0) as f64;

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
            window_height,
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
