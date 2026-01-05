//! Window and terminal lifecycle management
//!
//! Handles window creation, cleanup of dead terminals and windows,
//! output terminal management, and focus synchronization.

use crate::state::{StackWindow, TermStack};
use crate::terminal_manager::{TerminalId, TerminalManager};
use crate::terminal_output::{find_terminal_window_index, is_window_bottom_visible};
use crate::title_bar::TITLE_BAR_HEIGHT;

/// Handle new external windows and window resize events.
///
/// Processes new external window additions (with keyboard focus if needed)
/// and handles window resize events with autoscroll logic.
pub fn handle_external_window_events(
    compositor: &mut TermStack,
    _terminal_manager: &mut TerminalManager,
) {
    // Handle new external window - heights are already managed in add_window,
    // just need to scroll and set keyboard focus if needed
    if let Some(window_idx) = compositor.new_external_window_index.take() {
        let needs_keyboard_focus = std::mem::take(&mut compositor.new_window_needs_keyboard_focus);

        tracing::info!(
            window_idx,
            cells_count = compositor.layout_nodes.len(),
            focused_index = ?compositor.focused_index(),
            needs_keyboard_focus,
            "handling new external window"
        );

        // If this is a foreground GUI window, give it keyboard focus
        if needs_keyboard_focus {
            compositor.update_keyboard_focus_for_focused_window();
            tracing::info!(window_idx, "set keyboard focus to foreground GUI window");
        }

        // Scroll to show the focused cell
        if let Some(focused_idx) = compositor.focused_index() {
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                tracing::info!(
                    window_idx,
                    focused_idx,
                    new_scroll,
                    "scrolled to show focused cell after external window added"
                );
            }
        }
    }

    // Handle external window resize
    if let Some((resized_idx, new_height)) = compositor.external_window_resized.take() {
        // Skip processing if this cell is currently being resized by the user
        // (don't let stale commits overwrite the drag updates)
        let is_resizing = compositor.resizing.as_ref().map(|d| d.window_index);
        if is_resizing == Some(resized_idx) {
            tracing::info!(
                resized_idx,
                new_height,
                is_resizing = ?is_resizing,
                "SKIPPING external_window_resized processing during active resize drag"
            );
        } else {
            // Check if focused cell bottom is visible before resize
            let should_autoscroll = if let Some(focused_idx) = compositor.focused_index() {
                focused_idx >= resized_idx && is_window_bottom_visible(compositor, focused_idx)
            } else {
                false
            };

            if let Some(node) = compositor.layout_nodes.get_mut(resized_idx) {
                node.height = new_height;
            }

            // Only autoscroll if focused cell is at/below resized window AND bottom was visible
            if should_autoscroll {
                if let Some(focused_idx) = compositor.focused_index() {
                    compositor.scroll_to_show_window_bottom(focused_idx);
                }
            }
        }
    }
}

/// Handle terminal spawn requests from input events.
///
/// Spawns a new terminal when requested via keyboard shortcut,
/// updates layout heights, and scrolls to show the new terminal.
pub fn handle_terminal_spawn(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    calculate_window_heights: impl Fn(&TermStack, &TerminalManager) -> Vec<i32>,
) {
    if !compositor.spawn_terminal_requested {
        return;
    }
    compositor.spawn_terminal_requested = false;

    match terminal_manager.spawn() {
        Ok(id) => {
            compositor.add_terminal(id);

            // Update cell heights
            let new_heights = calculate_window_heights(compositor, terminal_manager);
            compositor.update_layout_heights(new_heights);

            // Scroll to show the new terminal
            if let Some(focused_idx) = compositor.focused_index() {
                if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(focused_idx) {
                    tracing::info!(
                        id = id.0,
                        window_count = compositor.layout_nodes.len(),
                        focused_idx,
                        new_scroll,
                        "spawned terminal, scrolling to show"
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!(error = ?e, "failed to spawn terminal");
        }
    }
}

/// Handle cleanup of output terminals from closed windows.
///
/// Terminals that have had output stay visible. Terminals that never had output are removed.
/// For foreground GUI sessions, restores the launching terminal's visibility.
pub fn handle_output_terminal_cleanup(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let cleanup_ids = std::mem::take(&mut compositor.pending_output_terminal_cleanup);

    for term_id in cleanup_ids {
        let has_had_output = terminal_manager.get(term_id)
            .map(|t| t.has_had_output())
            .unwrap_or(false);

        // Check if this was a foreground GUI session and restore the launcher
        if let Some((launcher_id, _window_was_linked)) = compositor.foreground_gui_sessions.remove(&term_id) {
            if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                launcher.visibility.on_gui_exit();
                tracing::info!(
                    launcher_id = launcher_id.0,
                    output_terminal_id = term_id.0,
                    "restored launching terminal visibility after foreground GUI closed"
                );
            }

            // Focus the restored launcher
            if let Some(idx) = find_terminal_window_index(compositor, launcher_id) {
                compositor.set_focus_by_index(idx);
                tracing::info!(
                    launcher_id = launcher_id.0,
                    index = idx,
                    "focused restored launcher after foreground GUI closed"
                );
            }
        }

        if has_had_output {
            // Terminal has had output - keep it visible
            tracing::info!(
                terminal_id = term_id.0,
                "output terminal has had output, keeping visible after window close"
            );
        } else {
            // Never had output - remove from layout and TerminalManager
            compositor.layout_nodes.retain(|n| {
                !matches!(n.cell, StackWindow::Terminal(id) if id == term_id)
            });
            terminal_manager.remove(term_id);
            tracing::info!(
                terminal_id = term_id.0,
                "removed output terminal (never had output) after window close"
            );
        }
    }
}

/// Cleanup dead terminals, sync focus, and handle shutdown.
///
/// Returns `true` if the compositor should shut down (all cells removed).
pub fn cleanup_and_sync_focus(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) -> bool {
    let (dead, focus_changed_to) = terminal_manager.cleanup();

    // Remove dead terminals from compositor
    for dead_id in &dead {
        // Fallback trigger: If this was an output terminal for a foreground GUI
        // that never opened a window, restore the launcher
        if let Some((launcher_id, window_was_linked)) = compositor.foreground_gui_sessions.remove(dead_id) {
            if !window_was_linked {
                // No window was ever linked - this is the fallback case
                // (e.g., GUI command failed before opening a window)
                if let Some(launcher) = terminal_manager.get_mut(launcher_id) {
                    launcher.visibility.on_gui_exit();
                    tracing::info!(
                        launcher_id = launcher_id.0,
                        output_terminal_id = dead_id.0,
                        "fallback: restored launcher after output terminal exited without window"
                    );
                }

                // Focus the restored launcher
                if let Some(idx) = find_terminal_window_index(compositor, launcher_id) {
                    compositor.set_focus_by_index(idx);
                    tracing::info!(
                        launcher_id = launcher_id.0,
                        index = idx,
                        "focused restored launcher after fallback"
                    );
                }
            }
        }

        compositor.remove_terminal(*dead_id);
        tracing::info!(id = dead_id.0, "removed dead terminal from cells");
    }

    // Sync compositor focus if a command terminal exited
    if let Some(new_focus_id) = focus_changed_to {
        if let Some(idx) = find_terminal_window_index(compositor, new_focus_id) {
            compositor.set_focus_by_index(idx);
            tracing::info!(id = new_focus_id.0, index = idx, "synced compositor focus to parent terminal");

            // Update cached height for unhidden terminal (was 0 when hidden)
            if let Some(term) = terminal_manager.get(new_focus_id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    let content = term.height as i32;
                    node.height = if term.show_title_bar {
                        content + TITLE_BAR_HEIGHT as i32
                    } else {
                        content
                    };
                }
            }

            // Scroll to show the unhidden parent terminal
            if let Some(new_scroll) = compositor.scroll_to_show_window_bottom(idx) {
                tracing::info!(
                    id = new_focus_id.0,
                    new_scroll,
                    "scrolled to show unhidden parent terminal"
                );
            }
        }
    }

    // Check if all cells are gone
    if !dead.is_empty() && compositor.layout_nodes.is_empty() {
        tracing::info!("all cells removed, shutting down");
        return true;
    }

    false
}
