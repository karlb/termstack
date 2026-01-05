//! Input event processing and handling
//!
//! Handles key repeat for terminal input and processes focus change and
//! scroll requests from the input handler.

use crate::state::TermStack;
use crate::terminal_manager::TerminalManager;

/// Handle key repeat for terminal input.
///
/// When a key is held down, this sends repeat events at regular intervals.
pub fn handle_key_repeat(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let Some((ref bytes, next_repeat)) = compositor.key_repeat else {
        return;
    };

    let now = std::time::Instant::now();
    if now < next_repeat {
        return;
    }

    // Time to send a repeat event
    let bytes_to_send = bytes.clone();

    if let Some(terminal) = terminal_manager.get_focused_mut(compositor.focused_window.as_ref()) {
        if let Err(e) = terminal.write(&bytes_to_send) {
            tracing::error!(?e, "failed to write repeat to terminal");
            compositor.key_repeat = None;
            return;
        }
    } else {
        // No focused terminal, stop repeating
        compositor.key_repeat = None;
        return;
    }

    // Schedule next repeat
    let next = now + std::time::Duration::from_millis(compositor.repeat_interval_ms);
    compositor.key_repeat = Some((bytes_to_send, next));
}

/// Handle focus change and scroll requests from input handlers.
///
/// This processes the `focus_change_requested` and `scroll_requested` fields
/// set by the input handler, applying the changes to compositor state.
pub fn handle_focus_and_scroll_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    // Handle focus change requests
    if compositor.focus_change_requested != 0 {
        // Create visibility checker closure
        let is_terminal_visible = |id| terminal_manager.is_terminal_visible(id);

        if compositor.focus_change_requested > 0 {
            compositor.focus_next(is_terminal_visible);
        } else {
            compositor.focus_prev(is_terminal_visible);
        }
        compositor.focus_change_requested = 0;

        // Update keyboard focus to match the newly focused cell
        compositor.update_keyboard_focus_for_focused_window();

        // Scroll to show focused cell
        if let Some(focused_idx) = compositor.focused_index() {
            compositor.scroll_to_show_window_bottom(focused_idx);
        }
    }

    // Handle scroll requests
    if compositor.scroll_requested != 0.0 {
        let delta = compositor.scroll_requested;
        compositor.scroll_requested = 0.0;
        compositor.scroll(delta);
        tracing::debug!(
            new_offset = compositor.scroll_offset,
            "scroll processed"
        );
    }
}
