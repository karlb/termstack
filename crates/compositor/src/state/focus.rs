//! Focus management for TermStack
//!
//! Handles focus tracking, focus changes, and keyboard focus updates.

use smithay::utils::SERIAL_COUNTER;
use smithay::reexports::wayland_server::Resource;
use crate::terminal_manager::TerminalId;
use super::{FocusedWindow, StackWindow, TermStack};

impl TermStack {
    /// Update focus after a cell is removed.
    ///
    /// With identity-based focus, this only needs to handle the case where the
    /// focused cell itself was removed. If so, focus the previous cell (or next
    /// if at the start).
    pub fn update_focus_after_removal(&mut self, removed_index: usize) {
        if self.layout_nodes.is_empty() {
            self.clear_focus();
            return;
        }

        // If the focused cell still exists, nothing to do
        if self.focused_index().is_some() {
            return;
        }

        // The focused cell was removed - focus adjacent cell
        // Prefer the cell that was after the removed one (now at removed_index)
        // Fall back to the one before if we removed the last cell
        let new_index = if removed_index < self.layout_nodes.len() {
            removed_index
        } else {
            self.layout_nodes.len().saturating_sub(1)
        };
        self.set_focus_by_index(new_index);
    }

    /// Focus previous visible cell, skipping hidden terminals
    ///
    /// # Arguments
    /// * `is_terminal_visible` - Closure that returns true if a terminal ID is visible
    pub fn focus_prev<F: Fn(TerminalId) -> bool>(&mut self, is_terminal_visible: F) {
        if let Some(current) = self.focused_index() {
            if current > 0 {
                // Search backward from previous index
                for i in (0..current).rev() {
                    let cell = &self.layout_nodes[i].cell;
                    let is_visible = match cell {
                        StackWindow::Terminal(id) => is_terminal_visible(*id),
                        StackWindow::External(_) => true, // External windows always visible
                    };

                    if is_visible {
                        self.set_focus_by_index(i);
                        self.ensure_focused_visible();
                        return;
                    }
                }
                // No visible cell found before current index
                // Boundary behavior preserved: don't wrap to bottom
            }
        }
    }

    /// Focus next visible cell, skipping hidden terminals
    ///
    /// # Arguments
    /// * `is_terminal_visible` - Closure that returns true if a terminal ID is visible
    pub fn focus_next<F: Fn(TerminalId) -> bool>(&mut self, is_terminal_visible: F) {
        if let Some(current) = self.focused_index() {
            // Search forward from next index
            for i in (current + 1)..self.layout_nodes.len() {
                let cell = &self.layout_nodes[i].cell;
                let is_visible = match cell {
                    StackWindow::Terminal(id) => is_terminal_visible(*id),
                    StackWindow::External(_) => true, // External windows always visible
                };

                if is_visible {
                    self.set_focus_by_index(i);
                    self.ensure_focused_visible();
                    return;
                }
            }
            // No visible cell found after current index
            // Boundary behavior preserved: don't wrap to top
        }
    }

    /// Update keyboard focus based on the currently focused cell.
    /// If focused cell is an external window, set keyboard focus to it.
    /// If focused cell is a terminal, clear keyboard focus from external windows.
    pub fn update_keyboard_focus_for_focused_window(&mut self) {
        let Some(focused_idx) = self.focused_index() else { return };
        let Some(node) = self.layout_nodes.get(focused_idx) else { return };

        let serial = SERIAL_COUNTER.next_serial();

        // Extract what we need before mutable operations
        let (wl_surface, is_terminal) = match &node.cell {
            StackWindow::External(entry) => (Some(entry.surface.wl_surface()), false),
            StackWindow::Terminal(_) => (None, true),
        };

        // Clone seat to avoid borrow conflicts when calling KeyboardTarget::enter
        let seat = self.seat.clone();
        let Some(keyboard) = seat.get_keyboard() else { return };

        if is_terminal {
            // Clear keyboard focus from external windows
            keyboard.set_focus(self, None, serial);
            self.deactivate_all_toplevels();
        } else {
            // Set keyboard focus on the wl_surface
            if let Some(surface) = wl_surface {
                keyboard.set_focus(self, Some(surface.clone()), serial);
                self.activate_toplevel(focused_idx);
            }
        }
    }

    /// Ensure the focused cell is visible
    fn ensure_focused_visible(&mut self) {
        if let Some(index) = self.focused_index() {
            self.scroll_to_show_window_bottom(index);
        }
    }

    /// Get the index of the focused cell by finding it in layout_nodes.
    ///
    /// Returns None if no cell is focused or if the focused cell no longer exists.
    /// Uses caching to avoid O(n) lookup on repeated calls within the same frame.
    pub fn focused_index(&self) -> Option<usize> {
        // Check cache first
        if let Some(cached) = self.cached_focused_index.get() {
            return cached;
        }

        // Compute and cache the result
        let result = self.compute_focused_index();
        self.cached_focused_index.set(Some(result));
        result
    }

    /// Compute focused index without caching (internal helper)
    fn compute_focused_index(&self) -> Option<usize> {
        let focused = self.focused_window.as_ref()?;
        self.layout_nodes.iter().position(|node| match (&node.cell, focused) {
            (StackWindow::Terminal(tid), FocusedWindow::Terminal(focused_tid)) => tid == focused_tid,
            (StackWindow::External(entry), FocusedWindow::External(focused_id)) => {
                entry.surface.wl_surface().id() == *focused_id
            }
            _ => false,
        })
    }

    /// Invalidate the cached focused index.
    ///
    /// Call this after any mutation to `layout_nodes` (insert/remove) or `focused_window`.
    pub fn invalidate_focused_index_cache(&self) {
        self.cached_focused_index.set(None);
    }

    /// Get the focused index or fallback to the end of the list.
    ///
    /// This is a convenience method for the common pattern of inserting new windows
    /// at the focused position, or at the end if nothing is focused.
    pub(super) fn focused_or_last(&self) -> usize {
        self.focused_index().unwrap_or(self.layout_nodes.len())
    }

    /// Set focus to the cell at the given index.
    ///
    /// Extracts the cell's identity and stores it in focused_window.
    pub fn set_focus_by_index(&mut self, index: usize) {
        if let Some(node) = self.layout_nodes.get(index) {
            self.focused_window = Some(match &node.cell {
                StackWindow::Terminal(id) => FocusedWindow::Terminal(*id),
                StackWindow::External(entry) => {
                    // All external windows are now Wayland toplevels (via xwayland-satellite)
                    let surface = entry.surface.wl_surface();
                    FocusedWindow::External(surface.id())
                }
            });
            // The cache now needs to resolve to `index`, but it's correct by construction
            // since we just set focused_window to point to layout_nodes[index].
            // We could set the cache directly here, but invalidating is safer.
            self.invalidate_focused_index_cache();
        }
    }

    /// Clear focus (no cell focused).
    pub fn clear_focus(&mut self) {
        self.focused_window = None;
        self.invalidate_focused_index_cache();
    }

    /// Check if the focused cell is a terminal
    pub fn is_terminal_focused(&self) -> bool {
        matches!(self.focused_window, Some(FocusedWindow::Terminal(_)))
    }

    /// Check if the focused cell is an external window
    pub fn is_external_focused(&self) -> bool {
        matches!(self.focused_window, Some(FocusedWindow::External(_)))
    }

    /// Get the focused terminal ID, if any
    pub fn focused_terminal(&self) -> Option<TerminalId> {
        match &self.focused_window {
            Some(FocusedWindow::Terminal(id)) => Some(*id),
            _ => None,
        }
    }
}
