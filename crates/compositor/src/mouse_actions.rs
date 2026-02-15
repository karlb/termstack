//! Shared mouse click, drag, and scroll handling
//!
//! Both Linux and macOS backends need close-button detection, resize drag
//! management, text selection end, and scroll handling. This module provides
//! cross-platform implementations that each backend calls with its native
//! coordinate types converted to `ScreenY`.

use smithay::reexports::wayland_server::Resource;

use crate::coords::ScreenY;
use crate::state::{FocusedWindow, ResizeDrag, StackWindow, TermStack, MIN_WINDOW_HEIGHT};
use crate::terminal_manager::TerminalManager;

/// Result of processing a left mouse button press.
pub enum ClickResult {
    /// A resize handle was clicked; drag has been started.
    ResizeDragStarted,
    /// The close button was clicked on the window at this index.
    CloseButtonClicked { index: usize },
    /// A window was clicked and focused.
    WindowClicked { index: usize },
    /// Click was not on any window.
    NoHit,
}

/// Check if a click in screen coordinates is on a close button.
///
/// The close button occupies the right `close_button_width` pixels of the
/// title bar (top `title_bar_height` pixels of the window).
pub fn is_click_on_close_button(
    screen_x: f64,
    screen_y: f64,
    window_screen_top: i32,
    output_width: i32,
    title_bar_height: i32,
    close_button_width: i32,
    has_ssd: bool,
) -> bool {
    if !has_ssd {
        return false;
    }
    let click_in_title_bar = (screen_y as i32) >= window_screen_top
        && (screen_y as i32) < window_screen_top + title_bar_height;
    let click_in_close_zone = screen_x >= (output_width - close_button_width) as f64;
    click_in_title_bar && click_in_close_zone
}

/// Core left-click processing: check resize handles, close buttons, and set
/// focus. Both backends call this with screen-Y coordinates.
///
/// Does NOT start text selection or perform platform-specific focus management
/// (keyboard focus, toplevel activation) — callers handle those after matching
/// on the result.
pub fn process_left_click(
    compositor: &mut TermStack,
    terminal_manager: &TerminalManager,
    screen_x: f64,
    screen_y: ScreenY,
    title_bar_height: i32,
    close_button_width: i32,
) -> ClickResult {
    // 1. Check for resize handle
    if let Some(handle_idx) = compositor.find_resize_handle_at(screen_y) {
        let node = &compositor.layout_nodes[handle_idx];
        let identity = match &node.cell {
            StackWindow::Terminal(id) => FocusedWindow::Terminal(*id),
            StackWindow::External(e) => {
                FocusedWindow::External(e.surface.wl_surface().id())
            }
        };
        compositor.resizing = Some(ResizeDrag {
            window_index: handle_idx,
            window_identity: identity,
            start_screen_y: screen_y.value() as i32,
            start_height: node.height,
            target_height: node.height,
            last_configure_time: std::time::Instant::now(),
            last_sent_height: None,
        });
        return ClickResult::ResizeDragStarted;
    }

    // 2. Find window at click position
    let Some(index) = compositor.window_at_screen_y(screen_y) else {
        return ClickResult::NoHit;
    };

    // 3. Check for close button in title bar
    let window_screen_top: i32 = compositor.layout_nodes[..index]
        .iter()
        .map(|n| n.height)
        .sum::<i32>()
        - compositor.scroll_offset as i32;

    let has_ssd = match &compositor.layout_nodes[index].cell {
        StackWindow::Terminal(id) => {
            terminal_manager.get(*id).is_some_and(|t| t.show_title_bar)
        }
        StackWindow::External(entry) => !entry.uses_csd,
    };

    if is_click_on_close_button(
        screen_x,
        screen_y.value(),
        window_screen_top,
        compositor.output_size.w,
        title_bar_height,
        close_button_width,
        has_ssd,
    ) {
        return ClickResult::CloseButtonClicked { index };
    }

    // 4. Focus the window
    compositor.set_focus_by_index(index);

    ClickResult::WindowClicked { index }
}

/// Handle left button release: end resize drag, end text selection.
///
/// Returns the selected text (if any) for the caller to copy to clipboard.
/// Platform-specific clipboard handling (PRIMARY vs system) is done by callers.
pub fn process_left_release(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
) -> Option<String> {
    // End resize drag
    if let Some(drag) = compositor.resizing.take() {
        if let Some(node) = compositor.layout_nodes.get(drag.window_index) {
            match &node.cell {
                StackWindow::Terminal(id) => {
                    if let Some(term) = terminal_manager.get_mut(*id) {
                        term.mark_dirty();
                    }
                }
                StackWindow::External(_) => {
                    let final_target = drag.target_height as u32;
                    if final_target > 0 {
                        compositor.request_resize(drag.window_index, final_target);
                    }
                }
            }
        }
        return None;
    }

    // End cross-window text selection
    crate::selection::end_cross_selection(compositor, terminal_manager)
}

/// Update resize drag during pointer motion.
///
/// Includes row-snapping for terminals so the resize visually aligns to
/// character cell boundaries.
pub fn update_resize_drag(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    screen_y: i32,
    title_bar_height: i32,
) {
    let Some(ref mut drag) = compositor.resizing else {
        return;
    };
    let delta = screen_y - drag.start_screen_y;
    let new_height = (drag.start_height + delta).max(MIN_WINDOW_HEIGHT);
    drag.target_height = new_height;

    let window_index = drag.window_index;
    if let Some(node) = compositor.layout_nodes.get_mut(window_index) {
        node.height = new_height;

        // Resize terminal with row-snapping
        if let StackWindow::Terminal(tid) = node.cell {
            let cell_height = terminal_manager.cell_height;
            if let Some(term) = terminal_manager.get_mut(tid) {
                let tb_h = if term.show_title_bar { title_bar_height as u32 } else { 0 };
                let content_height = (new_height as u32).saturating_sub(tb_h);
                let rows = (content_height / cell_height).max(1);
                let snapped_content = rows * cell_height;
                let snapped_total = (snapped_content + tb_h) as i32;

                term.resize_to_height(snapped_content, cell_height);
                node.height = snapped_total;
            }
        }
    }
}

/// Handle scroll input (compositor column scroll or terminal scrollback).
///
/// `pixel_delta`: scroll amount in pixels (positive = content moves down).
/// `scrollback_lines`: if Some, scroll terminal scrollback by this many lines
///   instead of compositor scroll.
pub fn handle_scroll(
    compositor: &mut TermStack,
    terminal_manager: &mut TerminalManager,
    pixel_delta: f64,
    shift_held: bool,
    pointer_screen_y: ScreenY,
    scrollback_lines: Option<i32>,
) {
    if shift_held {
        // Shift+Scroll: terminal scrollback navigation
        let lines = scrollback_lines.unwrap_or(0);
        if lines == 0 {
            return;
        }
        if let Some(index) = compositor.window_at_screen_y(pointer_screen_y) {
            if let StackWindow::Terminal(tid) = compositor.layout_nodes[index].cell {
                if let Some(term) = terminal_manager.get_mut(tid) {
                    term.terminal.scroll_display(lines);
                    term.mark_dirty();
                }
            }
        }
    } else {
        // Regular scroll: compositor column navigation
        if pixel_delta == 0.0 {
            return;
        }
        compositor.pending_scroll_delta += pixel_delta;

        // Clamp so scroll debt doesn't accumulate at boundaries
        let max_scroll = compositor.max_scroll();
        let projected = compositor.scroll_offset + compositor.pending_scroll_delta;
        if projected < 0.0 {
            compositor.pending_scroll_delta = -compositor.scroll_offset;
        } else if projected > max_scroll {
            compositor.pending_scroll_delta = max_scroll - compositor.scroll_offset;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_button_click_inside_title_bar_on_button() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        let window_screen_top = 0; // First window starts at top

        // Click in close button area within title bar
        let close_button_x = (output_width - close_button_width) as f64 + 5.0;
        let title_bar_y = 5.0; // Inside title bar

        assert!(is_click_on_close_button(
            close_button_x,
            title_bar_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));
    }

    #[test]
    fn close_button_click_outside_title_bar() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        let window_screen_top = 0;

        // Click below title bar (in content area)
        let close_button_x = (output_width - close_button_width) as f64 + 5.0;
        let below_title_bar = (title_bar_height + 10) as f64;

        assert!(!is_click_on_close_button(
            close_button_x,
            below_title_bar,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));
    }

    #[test]
    fn close_button_click_outside_button_x_range() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        let window_screen_top = 0;

        // Click in title bar but not on close button
        let left_of_button = 100.0;
        let title_bar_y = 5.0;

        assert!(!is_click_on_close_button(
            left_of_button,
            title_bar_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));
    }

    #[test]
    fn close_button_not_triggered_without_ssd() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        let window_screen_top = 0;

        let close_button_x = (output_width - close_button_width) as f64 + 5.0;
        let title_bar_y = 5.0;

        assert!(!is_click_on_close_button(
            close_button_x,
            title_bar_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            false,
        ));
    }

    #[test]
    fn close_button_with_scrolled_window() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        // Window scrolled so its top is at y=200
        let window_screen_top = 200;

        let close_button_x = (output_width - close_button_width) as f64 + 5.0;
        let click_y = 210.0; // Inside title bar (200..224)

        assert!(is_click_on_close_button(
            close_button_x,
            click_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));

        // Click above the window should NOT hit close button
        let above_window = 195.0;
        assert!(!is_click_on_close_button(
            close_button_x,
            above_window,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));
    }

    #[test]
    fn close_button_edge_at_boundary() {
        let output_width = 800;
        let title_bar_height = 24;
        let close_button_width = 30;
        let window_screen_top = 0;
        let title_bar_y = 5.0;

        // Exactly at left edge of close button
        let close_button_left_edge = (output_width - close_button_width) as f64;
        assert!(is_click_on_close_button(
            close_button_left_edge,
            title_bar_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));

        // One pixel left of close button
        assert!(!is_click_on_close_button(
            close_button_left_edge - 1.0,
            title_bar_y,
            window_screen_top,
            output_width,
            title_bar_height,
            close_button_width,
            true,
        ));
    }
}
