//! Terminal output processing and resizing
//!
//! Handles PTY output processing, automatic terminal growth, alternate screen
//! detection, manual resize requests, and output terminal promotion.

use crate::render::calculate_terminal_render_height;
use crate::state::{FocusedWindow, StackWindow, TermStack};
use crate::terminal_manager::{TerminalId, TerminalManager};

/// Minimum number of rows for a terminal
const MIN_TERMINAL_ROWS: u16 = 1;

/// Process terminal PTY output and handle sizing actions.
///
/// Processes all terminal output, handles growth requests, and auto-resizes
/// terminals that enter alternate screen mode. Also flushes any pending
/// write buffers from paste operations.
pub fn process_terminal_output(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    // Flush any pending write buffers (from paste operations)
    terminal_manager.flush_pending_writes();

    // Process PTY output and get sizing actions
    let sizing_actions = terminal_manager.process_all();

    // Handle sizing actions
    for (id, action) in sizing_actions {
        if let terminal::sizing::SizingAction::RequestGrowth { target_rows } = action {
            // Skip auto-growth if terminal was manually resized
            if terminal_manager.get(id).map(|t| t.manually_sized).unwrap_or(false) {
                tracing::debug!(id = id.0, "skipping growth request - terminal was manually resized");
                continue;
            }

            tracing::info!(id = id.0, target_rows, "processing growth request");
            terminal_manager.grow_terminal(id, target_rows);

            // If focused terminal grew, update cache and scroll (if bottom was visible)
            let is_focused = matches!(compositor.focused_window.as_ref(), Some(FocusedWindow::Terminal(fid)) if *fid == id);
            if is_focused {
                if let Some(idx) = find_terminal_window_index(compositor, id) {
                    // Check if bottom was visible before resize
                    let was_bottom_visible = is_window_bottom_visible(compositor, idx);

                    if let Some(term) = terminal_manager.get(id) {
                        if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                            node.height = calculate_terminal_render_height(
                                term.height as i32,
                                term.show_title_bar,
                                true, // visible since we're handling growth
                            );
                        }
                    }

                    // Only autoscroll if bottom was already visible
                    // This allows users to scroll up while content flows in
                    if was_bottom_visible {
                        compositor.scroll_to_show_window_bottom(idx);
                        tracing::debug!(
                            id = id.0,
                            idx,
                            "autoscrolled after terminal growth (bottom was visible)"
                        );
                    } else {
                        tracing::debug!(
                            id = id.0,
                            idx,
                            "skipped autoscroll after terminal growth (bottom not visible)"
                        );
                    }
                }
            }
        }
    }

    // Fix up heights for terminals that just became visible
    // (e.g., command terminals that received output this frame).
    // Without this, a terminal transitioning from WaitingForOutput to HasOutput
    // would keep height 0, causing validate_state() to panic.
    for node in &mut compositor.layout_nodes {
        if let StackWindow::Terminal(tid) = node.cell {
            if node.height == 0 && terminal_manager.is_terminal_visible(tid) {
                if let Some(term) = terminal_manager.get(tid) {
                    node.height = calculate_terminal_render_height(
                        term.height as i32,
                        term.show_title_bar,
                        true,
                    );
                }
            }
        }
    }

    // Auto-resize terminals entering alternate screen mode
    auto_resize_alt_screen_terminals(compositor, terminal_manager);
}

/// Auto-resize terminals that entered alternate screen mode.
fn auto_resize_alt_screen_terminals(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let max_height = terminal_manager.max_rows as u32 * terminal_manager.cell_height;
    // Check ALL terminals, not just visible ones - TUI apps like fzf enter
    // alternate screen before producing content_rows, so they'd be hidden
    let all_ids = terminal_manager.ids();

    let mut ids_to_resize = Vec::new();
    for id in all_ids {
        if let Some(term) = terminal_manager.get_mut(id) {
            if term.check_alt_screen_resize_needed(max_height) {
                ids_to_resize.push(id);
            }
        }
    }

    let max_rows = terminal_manager.max_rows;
    let char_height = terminal_manager.cell_height;

    for id in ids_to_resize {
        if let Some(term) = terminal_manager.get_mut(id) {
            let old_height = term.height;
            term.resize(max_rows, char_height);
            let new_height = term.height;

            tracing::info!(
                id = id.0,
                old_height,
                new_height,
                "auto-resized terminal for alternate screen"
            );

            // Update cached height (note: new_height is content-only, need to add title bar)
            if let Some(idx) = find_terminal_window_index(compositor, id) {
                if let Some(node) = compositor.layout_nodes.get_mut(idx) {
                    node.height = calculate_terminal_render_height(
                        new_height as i32,
                        term.show_title_bar,
                        true,
                    );
                }
            }
        }
    }
}

/// Find the cell index for a terminal ID.
pub fn find_terminal_window_index(compositor: &TermStack, id: TerminalId) -> Option<usize> {
    compositor.layout_nodes.iter().enumerate().find_map(|(i, node)| {
        if let StackWindow::Terminal(tid) = node.cell {
            if tid == id {
                return Some(i);
            }
        }
        None
    })
}

/// Check if a window's bottom edge is visible in the viewport.
pub fn is_window_bottom_visible(compositor: &TermStack, window_idx: usize) -> bool {
    let cell_top_y: i32 = compositor
        .layout_nodes
        .iter()
        .take(window_idx)
        .map(|n| n.height)
        .sum();
    let window_height = compositor
        .layout_nodes
        .get(window_idx)
        .map(|n| n.height)
        .unwrap_or(0);
    let cell_bottom_y = cell_top_y + window_height;
    let viewport_height = compositor.output_size.h;

    // Calculate minimum scroll needed to show cell bottom
    let min_scroll_for_bottom = (cell_bottom_y - viewport_height).max(0) as f64;

    // Cell bottom is visible if current scroll >= minimum needed
    // (allowing small epsilon for floating point comparison)
    compositor.scroll_offset >= (min_scroll_for_bottom - 1.0)
}

/// Handle IPC resize request from termstack --resize.
///
/// Resizes the focused terminal to full or content-based height and
/// sends ACK to unblock the termstack process.
pub fn handle_ipc_resize_request(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) {
    let Some((resize_mode, ack_stream)) = compositor.pending_resize_request.take() else {
        return;
    };

    let focused_id = match compositor.focused_window.as_ref() {
        Some(FocusedWindow::Terminal(id)) => *id,
        _ => {
            tracing::warn!("resize request but no focused terminal");
            if let Err(e) = crate::ipc::send_ack(ack_stream) {
                tracing::warn!(error = ?e, "Failed to send ACK for failed resize request");
            }
            return;
        }
    };

    let char_height = terminal_manager.cell_height;
    let new_rows = match resize_mode {
        crate::ipc::ResizeMode::Full => {
            tracing::info!(id = focused_id.0, max_rows = terminal_manager.max_rows, "resize to full");
            terminal_manager.max_rows
        }
        crate::ipc::ResizeMode::Content => {
            // Process pending PTY output first
            if let Some(term) = terminal_manager.get_mut(focused_id) {
                term.process();
            }

            // Calculate content rows from last non-empty line
            if let Some(term) = terminal_manager.get(focused_id) {
                let last_line = term.terminal.last_content_line();
                // last_line is 0-indexed, so +1 converts to row count
                let content_rows = (last_line + 1).max(MIN_TERMINAL_ROWS);
                tracing::info!(id = focused_id.0, last_line, content_rows, "resize to content");
                content_rows
            } else {
                MIN_TERMINAL_ROWS
            }
        }
    };

    if let Some(term) = terminal_manager.get_mut(focused_id) {
        tracing::info!(id = focused_id.0, ?resize_mode, new_rows, "resizing terminal via IPC");
        term.resize(new_rows, char_height);

        // Update cached height using centralized calculation
        let total_height = calculate_terminal_render_height(
            term.height as i32,
            term.show_title_bar,
            true,
        );
        for node in compositor.layout_nodes.iter_mut() {
            if let StackWindow::Terminal(tid) = node.cell {
                if tid == focused_id {
                    node.height = total_height;
                    break;
                }
            }
        }

        // Scroll to keep terminal visible
        if let Some(idx) = compositor.focused_index() {
            compositor.scroll_to_show_window_bottom(idx);
        }
    }

    if let Err(e) = crate::ipc::send_ack(ack_stream) {
        tracing::warn!(error = ?e, "Failed to send ACK for resize completion");
    }
}

/// Promote output terminals that have content to standalone cells.
///
/// Checks each external window's output terminal. If it has output and isn't
/// already a cell, inserts it as a cell right after the window.
pub fn promote_output_terminals(
    compositor: &mut TermStack,
    terminal_manager: &TerminalManager,
) {
    // Collect (window_idx, term_id) pairs for terminals to promote
    let mut to_promote: Vec<(usize, TerminalId)> = Vec::new();

    for (window_idx, node) in compositor.layout_nodes.iter().enumerate() {
        if let StackWindow::External(entry) = &node.cell {
            if let Some(term_id) = entry.output_terminal {
                // Check if terminal already in cells
                let already_cell = compositor.layout_nodes.iter().any(|n| {
                    matches!(n.cell, StackWindow::Terminal(id) if id == term_id)
                });

                if !already_cell {
                    if let Some(term) = terminal_manager.get(term_id) {
                        // Promote if terminal has meaningful content (not just newlines)
                        if term.terminal.has_meaningful_content() {
                            to_promote.push((window_idx, term_id));
                        }
                    }
                }
            }
        }
    }

    // Promote terminals (one at a time to avoid index issues)
    // Insert in reverse order so earlier insertions don't affect later indices
    for (window_idx, term_id) in to_promote.into_iter().rev() {
        // Insert terminal cell right after this window
        // (window_idx + 1 puts it below the window in the column)
        let insert_idx = window_idx + 1;

        let height = terminal_manager.get(term_id)
            .map(|t| calculate_terminal_render_height(t.height as i32, t.show_title_bar, true))
            .unwrap_or(0);

        compositor.layout_nodes.insert(insert_idx, crate::state::LayoutNode {
            cell: StackWindow::Terminal(term_id),
            height,
        });
        // Invalidate cache since layout_nodes changed
        compositor.invalidate_focused_index_cache();

        tracing::info!(
            terminal_id = term_id.0,
            window_idx,
            insert_idx,
            "promoted output terminal to standalone cell"
        );
    }
}
