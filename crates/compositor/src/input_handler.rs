//! Input event processing and handling
//!
//! Handles key repeat for terminal input and processes focus change
//! requests from the input handler.

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

/// Handle focus change requests from input handlers.
///
/// This processes the `focus_change_requested` field set by the input handler,
/// applying focus changes to compositor state.
///
/// Note: Scroll is applied immediately in input handlers to avoid backlog.
pub fn handle_focus_change_requests(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    if compositor.focus_change_requested == 0 {
        return;
    }

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
