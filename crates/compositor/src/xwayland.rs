//! XWayland support - X11 window management via XWayland
//!
//! This module implements the XwmHandler trait to manage X11 windows
//! within the column compositor layout.

use smithay::desktop::Window;
use smithay::utils::{Logical, Rectangle};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XwmHandler};

use crate::state::{ColumnCell, ColumnCompositor, LayoutNode, SurfaceKind, WindowEntry, WindowState};

impl XwmHandler for ColumnCompositor {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.x11_wm.as_mut().expect("X11Wm should be set when XwmHandler is called")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Just track the window - don't add to layout until map_window_request
        tracing::info!(
            class = %window.class(),
            instance = %window.instance(),
            "new X11 window created"
        );
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Override-redirect windows are menus, tooltips, etc.
        // They're handled separately from normal windows
        tracing::info!(
            class = %window.class(),
            "new override-redirect X11 window"
        );
    }

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // This is where we add normal windows to the column layout
        // Check reparent status BEFORE and AFTER mapping
        // mapped_window_id() returns Some(frame_id) if reparented, None if not
        let frame_before = window.mapped_window_id();

        tracing::info!(
            class = %window.class(),
            instance = %window.instance(),
            ?frame_before,
            "X11 window map request"
        );

        // Configure window to compositor width, initial height
        let width = self.output_size.w;
        let initial_height = 200i32;
        let geo = Rectangle::new((0, 0).into(), (width, initial_height).into());
        if let Err(e) = window.configure(geo) {
            tracing::warn!(?e, "failed to configure X11 window");
        }

        // Mark as mapped
        if let Err(e) = window.set_mapped(true) {
            tracing::warn!(?e, "failed to set X11 window as mapped");
        }

        // Check if reparenting happened
        let frame_after = window.mapped_window_id();
        tracing::info!(
            ?frame_after,
            "X11 window after set_mapped"
        );

        // Add to layout with the configured height (not surface.geometry() which may lag)
        self.add_x11_window(window, initial_height as u32);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        // Add to overlay list for rendering on top
        tracing::info!(
            class = %window.class(),
            "override-redirect X11 window mapped"
        );
        self.override_redirect_windows.push(window);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        tracing::info!(class = %window.class(), "X11 window unmapped");
        self.remove_x11_window(&window);
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        tracing::info!(class = %window.class(), "X11 window destroyed");
        self.remove_x11_window(&window);
        self.override_redirect_windows.retain(|w| w != &window);
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        _x: Option<i32>,
        _y: Option<i32>,
        _w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        // Column layout: ignore X position, force full width
        // Accept height requests if reasonable
        let width = self.output_size.w;
        let current_geo = window.geometry();
        let height = h.map(|h| h as i32).unwrap_or(current_geo.size.h);

        let geo = Rectangle::new((0, 0).into(), (width, height).into());
        if let Err(e) = window.configure(geo) {
            tracing::warn!(?e, "failed to configure X11 window on configure_request");
        }
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
        // Get window's current geometry to see if it matches what we configured
        let window_geo = window.geometry();

        tracing::info!(
            geometry_w = geometry.size.w,
            geometry_h = geometry.size.h,
            window_geo_w = window_geo.size.w,
            window_geo_h = window_geo.size.h,
            "configure_notify: reported geometry vs window.geometry()"
        );

        // Update stored height for layout and window state
        if let Some((idx, _)) = self.find_x11_window(&window) {
            if let Some(node) = self.layout_nodes.get_mut(idx) {
                let notified_height = geometry.size.h as u32;

                // Check if we're in the middle of a resize
                let should_update = if let ColumnCell::External(entry) = &node.cell {
                    match &entry.state {
                        WindowState::PendingResize { requested_height, .. } => {
                            // During resize, only accept configure_notify that matches our request
                            // This prevents stale configure_notify from overwriting the new height
                            if notified_height == *requested_height {
                                tracing::info!(
                                    index = idx,
                                    requested_height,
                                    notified_height,
                                    "configure_notify matches pending resize, accepting"
                                );
                                true
                            } else {
                                tracing::debug!(
                                    index = idx,
                                    requested_height,
                                    notified_height,
                                    "configure_notify doesn't match pending resize, ignoring stale event"
                                );
                                false
                            }
                        }
                        _ => {
                            // Not resizing, accept any configure_notify
                            true
                        }
                    }
                } else {
                    true
                };

                if should_update {
                    // For SSD windows, store visual height (content + title bar)
                    // For CSD windows, store the height as-is
                    let visual_height = if let ColumnCell::External(entry) = &node.cell {
                        if entry.uses_csd {
                            geometry.size.h
                        } else {
                            geometry.size.h + crate::title_bar::TITLE_BAR_HEIGHT as i32
                        }
                    } else {
                        geometry.size.h
                    };
                    node.height = visual_height;

                    // Update window state to Active with new height
                    // X11 doesn't use configure_ack, configure_notify means the resize is done
                    if let ColumnCell::External(entry) = &mut node.cell {
                        entry.state = WindowState::Active { height: notified_height };
                        tracing::info!(
                            index = idx,
                            notified_height,
                            visual_height,
                            "X11 window resize completed via configure_notify"
                        );
                    }
                    self.recalculate_layout();
                }
            }
        } else {
            tracing::warn!("configure_notify: window not found in layout");
        }
    }

    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _resize_edge: ResizeEdge,
    ) {
        // Handle interactive resize - for column layout, we might want to
        // only allow vertical resizing. For now, ignore resize requests.
        tracing::debug!("X11 resize request ignored (column layout manages positions)");
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {
        // Ignore move requests - column layout manages position
        tracing::debug!("X11 move request ignored (column layout manages positions)");
    }
}

impl ColumnCompositor {
    /// Add an X11 window to the column layout
    ///
    /// `configured_height` should be the height we configured the window to,
    /// not the surface geometry (which may lag behind due to X11 async nature).
    pub fn add_x11_window(&mut self, surface: X11Surface, configured_height: u32) {
        let window = Window::new_x11_window(surface.clone());

        // Use WM_CLASS as app_id for CSD detection
        let class = surface.class();
        let uses_csd = self.is_csd_app(&class) || surface.is_decorated();

        let initial_height = configured_height;

        // Consume pending output terminal (if any)
        let output_terminal = self.pending_window_output_terminal.take();

        // Consume pending command for title bar (if any)
        let command = self.pending_window_command.take().unwrap_or_else(|| class.clone());

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

        let entry = WindowEntry {
            surface: SurfaceKind::X11(surface),
            window,
            state: WindowState::Active { height: initial_height },
            output_terminal,
            command,
            uses_csd,
            is_foreground_gui,
        };

        // For GUI spawns with an output terminal, insert at the output terminal's position
        // (pushing it down). This gives the order: GUI, Output, Launcher.
        // For regular windows, insert at focused position.
        let insert_index = if let Some(term_id) = output_terminal {
            // Find the output terminal's position and insert there
            let output_pos = self.layout_nodes.iter().position(|node| {
                matches!(node.cell, ColumnCell::Terminal(id) if id == term_id)
            });
            if let Some(pos) = output_pos {
                tracing::info!(
                    terminal_id = term_id.0,
                    output_pos = pos,
                    "inserting X11 GUI window at output terminal position"
                );
                pos
            } else {
                tracing::warn!(
                    terminal_id = term_id.0,
                    "output terminal not found in layout, using focused index"
                );
                self.focused_index().unwrap_or(self.layout_nodes.len())
            }
        } else {
            self.focused_index().unwrap_or(self.layout_nodes.len())
        };

        // Visual height includes title bar for SSD windows (consistent with configure_notify)
        let visual_height = if uses_csd {
            initial_height as i32
        } else {
            initial_height as i32 + crate::title_bar::TITLE_BAR_HEIGHT as i32
        };
        self.layout_nodes.insert(insert_index, LayoutNode {
            cell: ColumnCell::External(Box::new(entry)),
            height: visual_height,
        });

        // For foreground GUI windows, focus the new window
        // For other windows (background GUI or regular), focus stays on existing cell
        // (with identity-based focus, no adjustment needed for insertion)
        if is_foreground_gui {
            self.set_focus_by_index(insert_index);
            tracing::info!(insert_index, "focused foreground X11 GUI window");
        }
        // Note: with identity-based focus, we don't need to adjust for insertion

        // Signal main loop to scroll to show this new window
        self.new_external_window_index = Some(insert_index);
        self.new_window_needs_keyboard_focus = is_foreground_gui;

        self.recalculate_layout();

        // Activate the new window
        self.activate_toplevel(insert_index);

        tracing::info!(
            cell_count = self.layout_nodes.len(),
            focused = ?self.focused_cell,
            insert_index,
            class,
            "X11 window added to layout"
        );
    }

    /// Find X11 window in layout
    pub fn find_x11_window(&self, surface: &X11Surface) -> Option<(usize, &WindowEntry)> {
        self.layout_nodes.iter().enumerate().find_map(|(idx, node)| {
            if let ColumnCell::External(entry) = &node.cell {
                if let SurfaceKind::X11(ref s) = entry.surface {
                    if s == surface {
                        return Some((idx, entry.as_ref()));
                    }
                }
            }
            None
        })
    }

    /// Remove X11 window from layout
    pub fn remove_x11_window(&mut self, surface: &X11Surface) {
        if let Some((idx, _)) = self.find_x11_window(surface) {
            let (output_terminal, is_foreground_gui) = if let ColumnCell::External(entry) = &self.layout_nodes[idx].cell {
                self.space.unmap_elem(&entry.window);
                (entry.output_terminal, entry.is_foreground_gui)
            } else {
                (None, false)
            };

            self.layout_nodes.remove(idx);

            // Queue output terminal for cleanup in main loop
            if let Some(term_id) = output_terminal {
                tracing::info!(
                    terminal_id = term_id.0,
                    is_foreground_gui,
                    "X11 window closed, queuing output terminal for cleanup"
                );
                self.pending_output_terminal_cleanup.push(term_id);
            }

            self.update_focus_after_removal(idx);
            self.recalculate_layout();

            tracing::info!(
                cell_count = self.layout_nodes.len(),
                focused = ?self.focused_cell,
                has_output_terminal = output_terminal.is_some(),
                is_foreground_gui,
                "X11 window removed from layout"
            );
            // Foreground GUI restoration is handled by handle_output_terminal_cleanup
            // which checks foreground_gui_sessions when processing pending_output_terminal_cleanup
        }
    }
}
