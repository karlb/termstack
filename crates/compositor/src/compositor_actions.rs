//! Compositor keybinding actions and dispatch
//!
//! Defines the set of compositor-level actions (quit, spawn, scroll, copy, etc.)
//! and a single dispatch function. Both Linux and macOS backends parse their
//! native key events into `CompositorAction` and call `apply_compositor_action`.

use crate::state::TermStack;

/// Scroll amount per key press (pixels)
pub const SCROLL_STEP: f64 = 50.0;

/// Compositor keybinding action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositorAction {
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
    FontSizeUp,
    FontSizeDown,
}

/// Apply a compositor action to the compositor state.
///
/// This is the single source of truth for what each action does.
pub fn apply_compositor_action(compositor: &mut TermStack, action: CompositorAction) {
    match action {
        CompositorAction::Quit => {
            tracing::info!("quit requested");
            compositor.running = false;
        }
        CompositorAction::SpawnTerminal => {
            tracing::debug!("spawn terminal binding triggered");
            compositor.spawn_terminal_requested = true;
        }
        CompositorAction::FocusNext => {
            tracing::debug!("focus next requested");
            compositor.focus_change_requested = 1;
        }
        CompositorAction::FocusPrev => {
            tracing::debug!("focus prev requested");
            compositor.focus_change_requested = -1;
        }
        CompositorAction::ScrollDown => {
            compositor.pending_scroll_delta += SCROLL_STEP;
        }
        CompositorAction::ScrollUp => {
            compositor.pending_scroll_delta += -SCROLL_STEP;
        }
        CompositorAction::ScrollToTop => {
            compositor.scroll_to_top();
        }
        CompositorAction::ScrollToBottom => {
            compositor.scroll_to_bottom();
        }
        CompositorAction::PageDown => {
            compositor.pending_scroll_delta += compositor.output_size.h as f64 * 0.9;
        }
        CompositorAction::PageUp => {
            compositor.pending_scroll_delta += -(compositor.output_size.h as f64 * 0.9);
        }
        CompositorAction::Copy => {
            tracing::debug!("copy to clipboard requested");
            compositor.pending_copy = true;
        }
        CompositorAction::Paste => {
            tracing::debug!("paste from clipboard requested");
            compositor.pending_paste = true;
        }
        CompositorAction::FontSizeUp => {
            tracing::debug!("font size increase requested");
            compositor.pending_font_size_delta += 1.0;
        }
        CompositorAction::FontSizeDown => {
            tracing::debug!("font size decrease requested");
            compositor.pending_font_size_delta -= 1.0;
        }
    }
}
