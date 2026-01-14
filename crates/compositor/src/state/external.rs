//! External window helpers for TermStack
//!
//! Handles external window-specific operations: CSD detection, activation, and hit testing.

use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState;
use super::{StackWindow, TermStack};

impl TermStack {
    /// Check if an app uses client-side decorations based on app_id pattern matching
    ///
    /// Patterns can use '*' as a suffix for prefix matching.
    /// Example: "org.gnome.*" matches "org.gnome.Maps", "org.gnome.Files", etc.
    pub fn is_csd_app(&self, app_id: &str) -> bool {
        self.csd_apps.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix('*') {
                app_id.starts_with(prefix)
            } else {
                app_id == pattern
            }
        })
    }

    /// Set the activated state on a toplevel window at the given index.
    /// Also clears the activated state from all other toplevels.
    /// This is required for GTK apps to run animations and handle input properly.
    pub fn activate_toplevel(&mut self, index: usize) {
        for (i, node) in self.layout_nodes.iter().enumerate() {
            if let StackWindow::External(entry) = &node.cell {
                let should_activate = i == index;
                entry.surface.with_pending_state(|state| {
                    if should_activate {
                        state.states.set(ToplevelState::Activated);
                    } else {
                        state.states.unset(ToplevelState::Activated);
                    }
                });
                entry.surface.send_pending_configure();
            }
        }
    }

    /// Deactivate all toplevel windows (e.g., when focusing a terminal)
    pub fn deactivate_all_toplevels(&mut self) {
        for node in &self.layout_nodes {
            if let StackWindow::External(entry) = &node.cell {
                entry.surface.with_pending_state(|state| {
                    state.states.unset(ToplevelState::Activated);
                });
                entry.surface.send_pending_configure();
            }
        }
    }
}
