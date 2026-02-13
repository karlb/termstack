//! Window height calculation and change handling
//!
//! Manages height calculations for all windows (terminals and external),
//! detects significant height changes, and handles auto-scroll adjustments.

use smithay::utils::{Physical, Size};

use crate::layout::{calculate_terminal_render_height, heights_changed_significantly};
use crate::state::{StackWindow, TermStack};
use crate::terminal_manager::TerminalManager;
use crate::terminal_output::is_window_bottom_visible;
use crate::title_bar::TITLE_BAR_HEIGHT;

/// Default terminal height in pixels (fallback when terminal doesn't exist)
const DEFAULT_TERMINAL_HEIGHT: i32 = 200;

/// Calculate cell heights for layout.
///
/// All cells store visual height in node.height (including title bar for SSD windows).
/// This is set by configure_notify (X11), configure_ack (Wayland), or terminal resize.
///
/// For terminals: hidden terminals always get 0 height; otherwise uses cached height
/// if available, falls back to terminal.height for new cells (already includes title bar).
///
/// For external windows: uses cached visual height if available, otherwise computes
/// from window state (which stores content height, so we add title bar for SSD).
pub fn calculate_window_heights(
    compositor: &TermStack,
    terminal_manager: &TerminalManager,
) -> Vec<i32> {
    compositor.layout_nodes.iter().map(|node| {
        match &node.cell {
            StackWindow::Terminal(tid) => {
                // Hidden terminals always get 0 height
                if !terminal_manager.is_terminal_visible(*tid) {
                    return 0;
                }
                // Use cached visual height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells: use centralized height calculation
                terminal_manager.get(*tid)
                    .map(|t| calculate_terminal_render_height(t.height as i32, t.show_title_bar, true))
                    .unwrap_or(DEFAULT_TERMINAL_HEIGHT)
            }
            StackWindow::External(entry) => {
                // Use cached visual height if available
                if node.height > 0 {
                    return node.height;
                }
                // Fallback for new cells: try window state first, then window.geometry()
                let mut content_height = entry.state.current_height() as i32;

                // If state hasn't been updated yet, try to get size from window geometry
                // This handles the initial commit case where the client has drawn
                // but handle_commit hasn't processed the size yet
                if content_height == 0 {
                    let geo = entry.window.geometry();
                    if geo.size.h > 0 {
                        content_height = geo.size.h;
                    }
                }

                if entry.uses_csd {
                    content_height
                } else {
                    // Add title bar for SSD windows to get visual height
                    content_height + TITLE_BAR_HEIGHT as i32
                }
            }
        }
    }).collect()
}

/// Check if heights changed significantly and auto-scroll if needed.
///
/// This updates the layout heights cache and adjusts scroll to keep the focused
/// cell visible when content changes size. Skips height updates entirely during
/// manual resize to avoid overwriting the user's drag updates.
pub fn check_and_handle_height_changes(
    compositor: &mut TermStack,
    actual_heights: Vec<i32>,
) {
    let is_resizing = compositor.resizing.is_some();

    // During resize: update layout positions to show target state for visual feedback
    // - Terminals: instant resize (drag-updated height)
    // - External windows being resized: use TARGET height for layout (shows final positions)
    // - External windows NOT being resized: use committed height
    // The resizing window will render at committed size but be positioned at target size,
    // giving visual feedback without flickering
    let heights_to_apply: Vec<i32> = compositor.layout_nodes.iter().enumerate().map(|(i, node)| {
        match &node.cell {
            StackWindow::Terminal(_) => {
                // Check if this terminal is being resized
                let is_terminal_resizing = compositor.resizing
                    .as_ref()
                    .map(|drag| drag.window_index == i)
                    .unwrap_or(false);

                if is_terminal_resizing {
                    // Being resized: use cached node.height for instant visual feedback
                    node.height
                } else {
                    // Not resizing: use actual_heights which correctly reflects visibility
                    // and texture size changes. This ensures click detection matches rendering.
                    actual_heights.get(i).copied().unwrap_or(node.height)
                }
            }
            StackWindow::External(_) => {
                // Check if this is the window being resized
                if let Some(drag) = &compositor.resizing {
                    if i == drag.window_index {
                        // Resizing window: use TARGET height for layout positioning
                        // (content still renders at committed size, but positioned at target)
                        return drag.target_height;
                    }
                }
                // Non-resizing external windows: use actual_heights from element geometry
                // This handles both new windows (before first commit) and post-commit
                // windows correctly, ensuring click detection matches rendering.
                actual_heights.get(i).copied().unwrap_or(node.height)
            }
        }
    }).collect();

    let current_heights: Vec<i32> = compositor
        .layout_nodes
        .iter()
        .map(|n| n.height)
        .collect();

    let heights_changed = heights_changed_significantly(
        &current_heights,
        &heights_to_apply,
        compositor.focused_index(),
    );

    // Skip autoscroll during resize to avoid disrupting drag
    let should_autoscroll = if heights_changed && !is_resizing {
        if let Some(focused_idx) = compositor.focused_index() {
            is_window_bottom_visible(compositor, focused_idx)
        } else {
            false
        }
    } else {
        false
    };

    compositor.update_layout_heights(heights_to_apply);

    // Adjust scroll if heights changed AND focused cell bottom was visible
    // This allows users to scroll up while content continues to flow in
    if should_autoscroll {
        if let Some(focused_idx) = compositor.focused_index() {
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                tracing::info!(
                    focused_idx,
                    new_scroll,
                    "scroll adjusted due to actual height change (bottom was visible)"
                );
            }
        }
    }
}

/// Handle compositor window resize.
///
/// Updates all terminals and external windows to match the new size,
/// and recalculates the layout.
pub fn handle_compositor_resize(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    new_size: Size<i32, Physical>,
) {
    compositor.output_size = new_size;

    // Update terminal manager dimensions
    terminal_manager.update_output_size(new_size.w as u32, new_size.h as u32);

    // Resize all existing terminals to new width
    terminal_manager.resize_all_terminals(new_size.w as u32);

    // Resize all external windows to new width
    compositor.resize_all_external_windows(new_size.w);

    compositor.recalculate_layout();
}
