//! Core window management for TermStack
//!
//! Handles adding and removing windows (both terminals and external windows) to/from the layout.

use smithay::desktop::Window;
use smithay::wayland::shell::xdg::ToplevelSurface;
use crate::terminal_manager::TerminalId;
use super::{FocusedWindow, LayoutNode, StackWindow, TermStack, WindowEntry, WindowState};

impl TermStack {
    /// Add a new external window at the focused position
    pub fn add_window(&mut self, toplevel: ToplevelSurface) {
        let window = Window::new_wayland_window(toplevel.clone());

        // Consume pending output terminal (if any)
        let output_terminal = self.pending_window_output_terminal.take();

        // Consume pending command for title bar (if any)
        let command = self.pending_window_command.take().unwrap_or_default();

        // Consume pending foreground GUI flag
        let is_foreground_gui = std::mem::take(&mut self.pending_gui_foreground);

        // Mark that this output terminal is now linked to a window
        // (for foreground GUI fallback trigger - we only restore launcher on process exit
        // if no window was ever linked)
        if let Some(term_id) = output_terminal {
            if let Some((_, window_linked)) = self.foreground_gui_sessions.get_mut(&term_id) {
                *window_linked = true;
            }
        }

        // Default initial height (will be resized based on content)
        let initial_height = 200u32;

        let entry = WindowEntry {
            surface: toplevel,
            window: window.clone(),
            state: WindowState::Active {
                height: initial_height,
            },
            output_terminal,
            command: command.clone(),
            uses_csd: false, // Will be set by XdgDecorationHandler if client requests CSD
            is_foreground_gui,
        };

        // Keep the output terminal in the layout - its title bar shows the command
        // that launched this window, which is useful context for the user.
        // (Previously we removed it and only promoted back if it had output,
        // but now that we have title bars, the terminal is valuable even without output.)

        // For GUI spawns with an output terminal, insert at the output terminal's position
        // (pushing it down). This gives the order: GUI, Output, Launcher.
        // For regular windows, insert at focused position.
        let insert_index = if let Some(term_id) = output_terminal {
            // Find the output terminal's position and insert there
            let output_pos = self.layout_nodes.iter().position(|node| {
                matches!(node.cell, StackWindow::Terminal(id) if id == term_id)
            });
            if let Some(pos) = output_pos {
                tracing::info!(
                    terminal_id = term_id.0,
                    output_pos = pos,
                    "inserting GUI window at output terminal position"
                );
                pos
            } else {
                tracing::warn!(
                    terminal_id = term_id.0,
                    "output terminal not found in layout, using focused index"
                );
                self.focused_or_last()
            }
        } else {
            self.focused_or_last()
        };

        self.layout_nodes.insert(insert_index, LayoutNode {
            cell: StackWindow::External(Box::new(entry)),
            height: initial_height as i32,
        });

        // For foreground GUI windows, focus the new window
        // For other windows (background GUI or regular), focus stays on existing cell
        // (with identity-based focus, no adjustment needed for insertion)
        if is_foreground_gui {
            self.set_focus_by_index(insert_index);
            tracing::info!(insert_index, "focused foreground GUI window");
        }
        // Note: with identity-based focus, we don't need to adjust for insertion

        // Signal main loop to scroll to show this new window and set keyboard focus if needed
        self.new_external_window_index = Some(insert_index);
        self.new_window_needs_keyboard_focus = is_foreground_gui;

        self.recalculate_layout();

        // Activate the new window (required for GTK animations to work)
        self.activate_toplevel(insert_index);

        tracing::info!(
            window_count = self.layout_nodes.len(),
            focused = ?self.focused_window,
            insert_index,
            has_output_terminal = output_terminal.is_some(),
            command = %command,
            "external window added"
        );
    }

    /// Add a new terminal above the focused position
    pub fn add_terminal(&mut self, id: TerminalId) {
        // Insert at focused index to appear ABOVE the focused cell
        // (lower index = higher on screen after Y-flip)
        let insert_index = self.focused_or_last();

        // Insert with placeholder height 0, will be updated in next frame
        self.layout_nodes.insert(insert_index, LayoutNode {
            cell: StackWindow::Terminal(id),
            height: 0,
        });

        // With identity-based focus, the previously focused cell's identity is unchanged
        // If nothing was focused, focus the new cell
        if self.focused_window.is_none() {
            self.focused_window = Some(FocusedWindow::Terminal(id));
        }

        self.recalculate_layout();

        tracing::info!(
            terminal_id = id.0,
            insert_index,
            window_count = self.layout_nodes.len(),
            "terminal added"
        );
    }

    /// Remove an external window by its surface
    /// If the window had an output terminal, it's added to pending_output_terminal_cleanup
    pub fn remove_window(&mut self, surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface) -> Option<TerminalId> {
        if let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(&node.cell, StackWindow::External(entry) if entry.surface.wl_surface() == surface)
        }) {
            let (output_terminal, is_foreground_gui) = if let StackWindow::External(entry) = &self.layout_nodes.remove(index).cell {
                self.space.unmap_elem(&entry.window);
                (entry.output_terminal, entry.is_foreground_gui)
            } else {
                (None, false)
            };

            // Queue output terminal for cleanup in main loop
            if let Some(term_id) = output_terminal {
                tracing::info!(
                    terminal_id = term_id.0,
                    is_foreground_gui,
                    "window closed, queuing output terminal for cleanup"
                );
                self.pending_output_terminal_cleanup.push(term_id);
            }

            self.update_focus_after_removal(index);

            self.recalculate_layout();

            tracing::info!(
                window_count = self.layout_nodes.len(),
                focused = ?self.focused_window,
                has_output_terminal = output_terminal.is_some(),
                is_foreground_gui,
                "external window removed"
            );

            // If this was a foreground GUI, return the output terminal ID
            // so the caller can restore the launching terminal
            if is_foreground_gui {
                return output_terminal;
            }
        }
        None
    }

    /// Remove a terminal by its ID
    pub fn remove_terminal(&mut self, id: TerminalId) {
        if let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(node.cell, StackWindow::Terminal(tid) if tid == id)
        }) {
            self.layout_nodes.remove(index);
            self.update_focus_after_removal(index);
            self.recalculate_layout();

            tracing::info!(
                window_count = self.layout_nodes.len(),
                focused = ?self.focused_window,
                terminal_id = ?id,
                "terminal removed"
            );
        }
    }
}
