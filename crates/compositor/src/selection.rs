//! Cross-window text selection
//!
//! Enables selecting text across multiple terminals and their title bars,
//! allowing users to copy a continuous range of commands and outputs as if
//! the stack were a single traditional terminal.

use std::time::Instant;

use crate::coords::RenderY;
use crate::layout::FOCUS_INDICATOR_WIDTH;
use crate::state::{CrossSelection, StackWindow, TermStack, WindowPosition};
use crate::terminal_manager::TerminalManager;
use crate::title_bar::{TITLE_BAR_HEIGHT, TITLE_BAR_PADDING};

/// Maximum number of windows a selection can span
const MAX_SELECTION_WINDOWS: usize = 50;

/// Determine which window and what position within it a click landed on
///
/// Returns (window_index, position) where position indicates whether the click
/// was on the title bar or content area, with appropriate coordinates.
pub fn position_at_point(
    compositor: &TermStack,
    terminals: &TerminalManager,
    render_x: f64,
    render_y: RenderY,
) -> Option<(usize, WindowPosition)> {
    let window_index = compositor.window_at(render_y)?;
    let node = compositor.layout_nodes.get(window_index)?;

    // Calculate window's render position
    let (window_render_y, window_height) = compositor.get_window_render_position(window_index);
    let window_render_top = window_render_y.value() + window_height as f64;

    // Determine if this window has a title bar
    let has_title_bar = match &node.cell {
        StackWindow::Terminal(id) => terminals
            .get(*id)
            .map(|t| t.show_title_bar)
            .unwrap_or(false),
        StackWindow::External(entry) => !entry.uses_csd,
    };

    if has_title_bar {
        let title_bar_bottom = window_render_top - TITLE_BAR_HEIGHT as f64;

        // Check if click is in title bar region (top of window)
        if render_y.value() >= title_bar_bottom {
            // Click is in title bar
            // Calculate character position from X coordinate
            let x_in_title = (render_x - FOCUS_INDICATOR_WIDTH as f64 - TITLE_BAR_PADDING as f64)
                .max(0.0) as f32;

            // Try to get character info from cache
            let char_index = compositor
                .title_bar_char_info
                .get(&window_index)
                .and_then(|info| info.char_index_at_x(x_in_title))
                .unwrap_or(0);

            return Some((window_index, WindowPosition::TitleBar { char_index }));
        }
    }

    // Click is in content area
    match &node.cell {
        StackWindow::Terminal(id) => {
            let managed = terminals.get(*id)?;
            let (char_width, char_height) = managed.terminal.cell_size();

            // Calculate position within terminal content
            let title_bar_offset = if has_title_bar {
                TITLE_BAR_HEIGHT as f64
            } else {
                0.0
            };
            let content_top = window_render_top - title_bar_offset;
            let local_y = (content_top - render_y.value()).max(0.0);
            let local_x = (render_x - FOCUS_INDICATOR_WIDTH as f64).max(0.0);

            let col = (local_x / char_width as f64) as usize;
            let row = (local_y / char_height as f64) as usize;
            let grid_rows = managed.terminal.grid_rows();

            tracing::debug!(
                window_index,
                terminal_id = id.0,
                col,
                row,
                grid_rows,
                local_y,
                char_height,
                "position_at_point content"
            );

            Some((window_index, WindowPosition::Content { col, row }))
        }
        StackWindow::External(_) => {
            // External windows don't have selectable text content
            // Return title bar position at start if window has title bar
            if has_title_bar {
                Some((window_index, WindowPosition::TitleBar { char_index: 0 }))
            } else {
                None
            }
        }
    }
}

/// Start a cross-window selection at the given render coordinates
pub fn start_cross_selection(
    compositor: &mut TermStack,
    terminals: &mut TerminalManager,
    render_x: f64,
    render_y: RenderY,
) -> bool {
    let Some((window_index, position)) = position_at_point(compositor, terminals, render_x, render_y) else {
        return false;
    };

    // Clear any existing selections in terminals
    for node in &compositor.layout_nodes {
        if let StackWindow::Terminal(id) = &node.cell {
            if let Some(term) = terminals.get_mut(*id) {
                if term.terminal.has_selection() {
                    term.terminal.clear_selection();
                    term.mark_selection_dirty();
                }
            }
        }
    }

    // If starting in a terminal's content, also start the terminal's internal selection
    if let WindowPosition::Content { col, row } = &position {
        if let Some(node) = compositor.layout_nodes.get(window_index) {
            if let StackWindow::Terminal(id) = &node.cell {
                if let Some(term) = terminals.get_mut(*id) {
                    term.terminal.start_selection(*col, *row);
                    term.mark_dirty();
                }
            }
        }
    }

    compositor.cross_selection = Some(CrossSelection::new(window_index, position));
    true
}

/// Update an ongoing cross-window selection
///
/// Returns true if the selection was updated
pub fn update_cross_selection(
    compositor: &mut TermStack,
    terminals: &mut TerminalManager,
    render_x: f64,
    render_y: RenderY,
) -> bool {
    let Some(cross_sel) = &compositor.cross_selection else {
        return false;
    };

    // Don't update completed selections (only update while actively dragging)
    if !cross_sel.active {
        return false;
    }

    // Throttle updates to avoid overwhelming the system
    let now = Instant::now();
    if now.duration_since(cross_sel.last_update) < std::time::Duration::from_millis(16) {
        return false;
    }

    let Some((end_window, end_position)) =
        position_at_point(compositor, terminals, render_x, render_y)
    else {
        return false;
    };

    // Get start anchor info (clone to avoid borrow issues)
    let start = compositor.cross_selection.as_ref().unwrap().start.clone();

    // Clamp selection span to MAX_SELECTION_WINDOWS
    let clamped_end_window = clamp_selection_window(start.window_index, end_window);

    // Update end position
    if let Some(sel) = &mut compositor.cross_selection {
        sel.end.window_index = clamped_end_window;
        sel.end.position = end_position.clone();
        sel.last_update = now;
    }
    let end_window = clamped_end_window;

    // Update terminal internal selections based on new range
    update_terminal_selections_for_range(compositor, terminals, &start, end_window, &end_position);

    true
}

/// Update terminal internal selections based on the cross-selection range
fn update_terminal_selections_for_range(
    compositor: &TermStack,
    terminals: &mut TerminalManager,
    start: &crate::state::SelectionAnchor,
    end_window: usize,
    end_position: &WindowPosition,
) {
    let (first_window, last_window) = if start.window_index <= end_window {
        (start.window_index, end_window)
    } else {
        (end_window, start.window_index)
    };

    // Clear all terminal selections first
    for node in &compositor.layout_nodes {
        if let StackWindow::Terminal(id) = &node.cell {
            if let Some(term) = terminals.get_mut(*id) {
                term.terminal.clear_selection();
            }
        }
    }

    // Update selections for terminals in range
    for (idx, node) in compositor.layout_nodes.iter().enumerate() {
        if idx < first_window || idx > last_window {
            continue;
        }

        let StackWindow::Terminal(id) = &node.cell else {
            continue;
        };

        let Some(term) = terminals.get_mut(*id) else {
            continue;
        };

        let content_rows = term.content_rows() as usize;
        let grid_rows = term.terminal.grid_rows() as usize;
        let (cols, _) = term.terminal.dimensions();
        let max_col = (cols as usize).saturating_sub(1);
        // Use content_rows for "select to bottom" to avoid empty trailing rows
        // But clamp to grid_rows for safety
        let last_content_row = content_rows.saturating_sub(1).min(grid_rows.saturating_sub(1));
        let max_row = grid_rows.saturating_sub(1);

        // Determine selection range for this terminal
        // Note: row values from position_at_point may exceed grid_rows if the window
        // is taller than the PTY size, so we clamp all values to valid bounds.
        let (start_col, start_row, end_col, end_row) = if first_window == last_window {
            // Single window selection
            match (&start.position, end_position) {
                (
                    WindowPosition::Content {
                        col: sc,
                        row: sr,
                    },
                    WindowPosition::Content {
                        col: ec,
                        row: er,
                    },
                ) => (
                    (*sc).min(max_col),
                    (*sr).min(max_row),
                    (*ec).min(max_col),
                    (*er).min(max_row),
                ),
                _ => continue, // Title bar only selection in single window
            }
        } else if idx == first_window {
            // First window (topmost) in multi-window selection
            // The selection exits this window going DOWN to the next window.
            // So we always select from the anchor point DOWN to the bottom.
            // - If dragging down: anchor is start_pos (where user clicked)
            // - If dragging up: anchor is end_pos (where mouse currently is)
            let anchor_pos = if start.window_index == first_window {
                &start.position // dragging down, user started here
            } else {
                end_position // dragging up, mouse is here
            };

            match anchor_pos {
                WindowPosition::Content { col, row } => (
                    (*col).min(max_col),
                    (*row).min(max_row),
                    max_col,
                    last_content_row,
                ),
                WindowPosition::TitleBar { .. } => (0, 0, max_col, last_content_row),
            }
        } else if idx == last_window {
            // Last window (bottommost) in multi-window selection
            // The selection enters this window from ABOVE (from the previous window).
            // In both directions, we select from the top of terminal to the anchor point.
            let anchor_pos = if start.window_index == last_window {
                &start.position // user started here (dragging up)
            } else {
                end_position // user ended here (dragging down)
            };

            match anchor_pos {
                WindowPosition::Content { col, row } => (
                    0,
                    0,
                    (*col).min(max_col),
                    (*row).min(max_row),
                ),
                WindowPosition::TitleBar { .. } => {
                    // Selection anchor is in title bar, don't select terminal content
                    continue;
                }
            }
        } else {
            // Middle window - select all content
            (0, 0, max_col, last_content_row)
        };

        tracing::debug!(
            window_idx = idx,
            terminal_id = id.0,
            start_col,
            start_row,
            end_col,
            end_row,
            content_rows,
            grid_rows,
            "setting terminal selection"
        );
        term.terminal.start_selection(start_col, start_row);
        term.terminal.update_selection(start_col, start_row, end_col, end_row);
        term.mark_selection_dirty();
    }
}

/// End the cross-window selection and extract selected text
///
/// Returns the combined text from all selected windows, or None if no selection.
/// The selection remains visible until the next selection starts (traditional terminal behavior).
pub fn end_cross_selection(
    compositor: &mut TermStack,
    terminals: &TerminalManager,
) -> Option<String> {
    // Mark selection as completed (not active) so it persists
    if let Some(sel) = &mut compositor.cross_selection {
        sel.active = false;
    }
    let cross_sel = compositor.cross_selection.as_ref()?;
    extract_cross_selection_text(compositor, terminals, cross_sel)
}

/// Extract text from the cross-selection range
fn extract_cross_selection_text(
    compositor: &TermStack,
    terminals: &TerminalManager,
    selection: &CrossSelection,
) -> Option<String> {
    let (first_window, last_window) = selection.window_range();

    // Determine the "top" and "bottom" anchors based on window order
    let (top_anchor, bottom_anchor) = if selection.start.window_index <= selection.end.window_index
    {
        (&selection.start, &selection.end)
    } else {
        (&selection.end, &selection.start)
    };

    let mut result = String::new();

    for idx in first_window..=last_window {
        let Some(node) = compositor.layout_nodes.get(idx) else {
            continue;
        };

        // Determine if this window has a title bar and get title text
        let (has_title_bar, title_text) = match &node.cell {
            StackWindow::Terminal(id) => {
                let term = terminals.get(*id);
                let has_tb = term.map(|t| t.show_title_bar).unwrap_or(false);
                let title = term.map(|t| t.title.clone()).unwrap_or_default();
                (has_tb, title)
            }
            StackWindow::External(entry) => (!entry.uses_csd, entry.command.clone()),
        };

        let is_first = idx == first_window;
        let is_last = idx == last_window;

        // Add separator between windows
        if idx > first_window && !result.is_empty() {
            result.push('\n');
        }

        // Handle title bar text
        if has_title_bar {
            let title_start = if is_first {
                match &top_anchor.position {
                    WindowPosition::TitleBar { char_index } => Some(*char_index),
                    WindowPosition::Content { .. } => None, // Selection starts in content
                }
            } else {
                Some(0) // Middle/last windows include full title
            };

            let title_end = if is_last {
                match &bottom_anchor.position {
                    WindowPosition::TitleBar { char_index } => Some(*char_index),
                    WindowPosition::Content { .. } => {
                        // Selection ends in content, include full title
                        Some(title_text.chars().count().saturating_sub(1))
                    }
                }
            } else {
                Some(title_text.chars().count().saturating_sub(1))
            };

            if let (Some(start), Some(end)) = (title_start, title_end) {
                if start <= end {
                    let title_slice: String =
                        title_text.chars().skip(start).take(end - start + 1).collect();
                    if !title_slice.is_empty() {
                        result.push_str(&title_slice);
                        result.push('\n');
                    }
                }
            }
        }

        // Handle terminal content
        if let StackWindow::Terminal(id) = &node.cell {
            let Some(term) = terminals.get(*id) else {
                continue;
            };

            // Get selected text from terminal
            let has_sel = term.terminal.has_selection();
            let selected = term.terminal.selection_text();
            tracing::debug!(
                window_idx = idx,
                terminal_id = id.0,
                has_selection = has_sel,
                selected_len = selected.as_ref().map(|s| s.len()),
                selected_preview = selected.as_ref().map(|s| if s.len() > 50 { format!("{}...", &s[..50]) } else { s.clone() }),
                "extracting terminal content"
            );
            if let Some(selected) = selected {
                if !selected.is_empty() {
                    result.push_str(&selected);
                }
            }
        }
        // Note: External windows don't have accessible text content
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Clamp the end window index so the selection spans at most MAX_SELECTION_WINDOWS
fn clamp_selection_window(start_window: usize, end_window: usize) -> usize {
    let span = if end_window >= start_window {
        end_window - start_window + 1
    } else {
        start_window - end_window + 1
    };

    if span <= MAX_SELECTION_WINDOWS {
        return end_window;
    }

    let clamped = if end_window >= start_window {
        start_window + MAX_SELECTION_WINDOWS - 1
    } else {
        start_window.saturating_sub(MAX_SELECTION_WINDOWS - 1)
    };

    tracing::warn!(
        start_window,
        end_window,
        clamped,
        max = MAX_SELECTION_WINDOWS,
        "selection span exceeds limit, clamping"
    );
    clamped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_selection_window_range() {
        // Selection from window 1 to window 3
        let sel = CrossSelection::new(1, WindowPosition::Content { col: 0, row: 0 });
        let mut sel = sel;
        sel.end.window_index = 3;

        assert_eq!(sel.window_range(), (1, 3));
        assert!(sel.is_multi_window());
        assert!(sel.contains_window(1));
        assert!(sel.contains_window(2));
        assert!(sel.contains_window(3));
        assert!(!sel.contains_window(0));
        assert!(!sel.contains_window(4));
    }

    #[test]
    fn cross_selection_reverse_direction() {
        // Selection from window 3 to window 1 (dragging up)
        let sel = CrossSelection::new(3, WindowPosition::Content { col: 0, row: 0 });
        let mut sel = sel;
        sel.end.window_index = 1;

        // window_range should still return (1, 3)
        assert_eq!(sel.window_range(), (1, 3));
        assert!(sel.is_multi_window());
    }

    #[test]
    fn cross_selection_single_window() {
        let sel = CrossSelection::new(2, WindowPosition::Content { col: 5, row: 10 });
        assert!(!sel.is_multi_window());
        assert_eq!(sel.window_range(), (2, 2));
    }

    #[test]
    fn selection_window_clamping() {
        // Within limit - no clamping
        assert_eq!(clamp_selection_window(0, 10), 10);
        assert_eq!(clamp_selection_window(10, 0), 0);

        // At limit - no clamping
        assert_eq!(clamp_selection_window(0, MAX_SELECTION_WINDOWS - 1), MAX_SELECTION_WINDOWS - 1);

        // Exceeds limit - clamped (dragging down)
        assert_eq!(clamp_selection_window(0, 100), MAX_SELECTION_WINDOWS - 1);

        // Exceeds limit - clamped (dragging up)
        assert_eq!(clamp_selection_window(100, 0), 100 - MAX_SELECTION_WINDOWS + 1);
    }

    #[test]
    fn fully_selected_middle_windows() {
        let sel = CrossSelection::new(0, WindowPosition::Content { col: 0, row: 0 });
        let mut sel = sel;
        sel.end.window_index = 4;

        // Middle windows (1, 2, 3) should be fully selected
        assert!(!sel.is_window_fully_selected(0)); // first
        assert!(sel.is_window_fully_selected(1));
        assert!(sel.is_window_fully_selected(2));
        assert!(sel.is_window_fully_selected(3));
        assert!(!sel.is_window_fully_selected(4)); // last
    }
}
