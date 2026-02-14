//! Resize handling for TermStack
//!
//! Handles window resize requests, resize completion tracking, and resize handle detection.

use std::time::Instant;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as ToplevelState;
use smithay::utils::{Size, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::reexports::wayland_server::Resource;
use super::{FocusedWindow, StackWindow, TermStack, WindowState};
use crate::coords::ScreenY;
use crate::terminal_manager::TerminalId;

// Constants
const RESIZE_TIMEOUT_MS: u128 = 5000;
const MIN_CONFIGURE_INTERVAL_MS: u64 = 100;
const RESIZE_HANDLE_SIZE: i32 = 8;

impl TermStack {
    /// Request a resize on an external window
    pub fn request_resize(&mut self, index: usize, new_height: u32) {
        let Some(node) = self.layout_nodes.get_mut(index) else {
            tracing::warn!("request_resize: node not found at index {}", index);
            return;
        };
        let StackWindow::External(entry) = &mut node.cell else {
            tracing::debug!("request_resize: cell at index {} is not External", index);
            return;
        };

        let current = entry.state.current_height();

        if current == new_height {
            tracing::trace!("request_resize: height unchanged ({})", current);
            return;
        }

        tracing::info!(
            index,
            current_height = current,
            new_height,
            "request_resize called"
        );

        // Get output width
        let width = self.output_size.w as u32;

        // For SSD windows, subtract title bar height from the total cell height
        // to get the actual surface content height
        let surface_height = if entry.uses_csd {
            new_height
        } else {
            new_height.saturating_sub(crate::title_bar::TITLE_BAR_HEIGHT)
        };

        // Request the resize (all external windows are Wayland toplevels via xwayland-satellite)
        entry.surface.with_pending_state(|state| {
            state.size = Some(Size::from((width as i32, surface_height as i32)));
        });
        entry.surface.send_configure();

        tracing::debug!(
            index,
            total_height = new_height,
            surface_height,
            uses_csd = entry.uses_csd,
            "sending configure with surface height"
        );

        let serial = SERIAL_COUNTER.next_serial().into();

        entry.state = WindowState::PendingResize {
            current_height: current,
            requested_height: new_height,
            request_serial: serial,
            requested_at: Instant::now(),
        };

        tracing::debug!(
            index,
            current_height = current,
            requested_height = new_height,
            "resize requested"
        );
    }

    /// Resize all external windows to new width (called when compositor is resized)
    pub fn resize_all_external_windows(&mut self, new_width: i32) {
        for node in &mut self.layout_nodes {
            if let StackWindow::External(entry) = &mut node.cell {
                let current_height = entry.state.current_height();
                entry.surface.with_pending_state(|state| {
                    state.size = Some(Size::from((new_width, current_height as i32)));
                });
                entry.surface.send_configure();
            }
        }

        tracing::info!(
            new_width,
            "resized all external windows to new width"
        );
    }

    /// Handle window commit - check for resize completion
    pub fn handle_commit(&mut self, surface: &WlSurface) {
        let Some(index) = self.layout_nodes.iter().position(|node| {
            matches!(&node.cell, StackWindow::External(entry) if entry.surface.wl_surface() == surface)
        }) else {
            // This commit is not for an external window we're tracking
            return;
        };

        // Skip processing commits if we're actively resizing this window
        // (commits during drag have the old size and would overwrite our visual updates)
        if self.resizing.as_ref().map(|d| d.window_index) == Some(index) {
            tracing::debug!(index, "skipping commit during active resize drag");
            return;
        }

        // Check if this is a CSD app (before getting mutable borrow)
        let should_mark_csd = {
            let Some(node) = self.layout_nodes.get(index) else {
                return;
            };
            let StackWindow::External(entry) = &node.cell else {
                return;
            };

            if !entry.uses_csd {
                let app_id: Option<String> = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .and_then(|data| data.lock().ok())
                        .and_then(|attrs| attrs.app_id.clone())
                });

                if let Some(ref id) = app_id {
                    self.is_csd_app(id)
                } else {
                    false
                }
            } else {
                false
            }
        };

        let Some(node) = self.layout_nodes.get_mut(index) else {
            return;
        };
        let StackWindow::External(entry) = &mut node.cell else {
            return;
        };

        if should_mark_csd {
            entry.uses_csd = true;
            tracing::debug!(command = %entry.command, "marked window as CSD from config");
        }

        // Refresh the Window's internal geometry cache from the newly committed surface state.
        // This must be called before window.geometry() to get accurate values.
        entry.window.on_commit();

        // Get the committed size. We check two sources:
        // 1. XdgToplevelSurfaceData.current.size - the size from configure/ack cycle
        // 2. window.geometry() - the actual rendered geometry from set_window_geometry
        //
        // We prefer geometry() when XdgToplevelSurfaceData has height=0, because that means
        // we sent configure(width, 0) telling the client to choose their own height.
        let configure_size: Option<Size<i32, smithay::utils::Logical>> = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .and_then(|data| data.current.size)
        });

        let geo = entry.window.geometry();

        // Determine the actual committed size:
        // - If configure_size has height > 0, use it (compositor requested specific size)
        // - Otherwise use geometry if available (client chose their own size)
        let committed_size = match configure_size {
            Some(size) if size.h > 0 => Some(size),
            _ if geo.size.w > 0 && geo.size.h > 0 => {
                tracing::info!(
                    index,
                    width = geo.size.w,
                    height = geo.size.h,
                    "using window.geometry() for commit size (client chose height)"
                );
                Some(geo.size)
            }
            other => other, // Use configure_size even if 0, or None
        };

        // Track if we need to enforce width constraint after processing height
        let mut width_resize_info: Option<(i32, i32)> = None; // (expected_width, surface_height)

        if committed_size.is_none() {
            tracing::warn!(
                index,
                command = %entry.command,
                "external window commit with NO size data"
            );
        }

        if let Some(size) = committed_size {
            let committed_surface_width = size.w;
            let committed_surface_height = size.h as u32;

            tracing::debug!(
                index,
                committed_surface_width,
                committed_surface_height,
                command = %entry.command,
                "external window commit received"
            );

            // For SSD windows, the total cell height includes the title bar
            // For CSD windows, surface height = cell height
            let committed_window_height = if entry.uses_csd {
                committed_surface_height
            } else {
                committed_surface_height + crate::title_bar::TITLE_BAR_HEIGHT
            };

            // Check if width needs to be enforced (app used wrong width)
            let expected_width = self.output_size.w;
            if committed_surface_width != expected_width {
                width_resize_info = Some((expected_width, committed_surface_height as i32));
            }

            match &entry.state {
                WindowState::PendingResize { requested_height, .. }
                    if committed_window_height == *requested_height =>
                {
                    entry.state = WindowState::Active { height: committed_window_height };
                    tracing::info!(
                        index,
                        window_height = committed_window_height,
                        surface_height = committed_surface_height,
                        "resize completed"
                    );
                    self.external_window_resized = Some((index, committed_window_height as i32));
                    self.recalculate_layout();

                    // Reset throttle for catch-up (don't send immediately - give window breathing room)
                    if let Some(drag) = &self.resizing {
                        if drag.window_index == index {
                            let current_target = drag.target_height as u32;
                            if current_target != committed_window_height {
                                // User continued dragging while commit was pending
                                // Reset throttle timer so next configure will be sent after standard delay
                                // This gives the window breathing room after commit instead of immediately
                                // sending another configure
                                tracing::debug!(
                                    index,
                                    committed = committed_window_height,
                                    target = current_target,
                                    "drag moved during commit - will catch up on next motion event"
                                );

                                if let Some(drag_mut) = self.resizing.as_mut() {
                                    // Clear last_sent_height so next motion event sees height changed
                                    drag_mut.last_sent_height = None;
                                    // Set time far in past so throttle allows immediate send on next motion
                                    drag_mut.last_configure_time =
                                        Instant::now() - std::time::Duration::from_millis(MIN_CONFIGURE_INTERVAL_MS);
                                }
                            }
                        }
                    }
                }
                WindowState::PendingResize { requested_height, current_height, .. } => {
                    // Resize pending but committed height doesn't match - log mismatch
                    tracing::warn!(
                        index,
                        committed_window_height,
                        committed_surface_height,
                        requested_height,
                        current_height,
                        uses_csd = entry.uses_csd,
                        "resize mismatch - committed height != requested"
                    );
                }
                WindowState::AwaitingCommit { target_height, .. }
                    if committed_window_height == *target_height =>
                {
                    entry.state = WindowState::Active { height: committed_window_height };
                    tracing::info!(
                        index,
                        window_height = committed_window_height,
                        surface_height = committed_surface_height,
                        "resize completed"
                    );
                    self.external_window_resized = Some((index, committed_window_height as i32));
                    self.recalculate_layout();
                }
                WindowState::Active { height } if committed_window_height != *height => {
                    let old_height = *height;
                    entry.state = WindowState::Active { height: committed_window_height };
                    tracing::info!(
                        index,
                        window_height = committed_window_height,
                        surface_height = committed_surface_height,
                        old_height,
                        "external window size changed"
                    );
                    self.external_window_resized = Some((index, committed_window_height as i32));
                    self.recalculate_layout();
                }
                _ => {}
            }
        }

        // Enforce width constraint: if app committed with wrong width, send configure
        // to resize to our width while keeping the app's chosen height.
        // Also set tiled states now that we're enforcing constraints.
        // (done after match to avoid borrow conflicts)
        if let Some((expected_width, surface_height)) = width_resize_info {
            let Some(node) = self.layout_nodes.get_mut(index) else {
                return;
            };
            let StackWindow::External(entry) = &mut node.cell else {
                return;
            };
            tracing::info!(
                index,
                expected_width,
                surface_height,
                "enforcing width constraint on external window"
            );
            entry.surface.with_pending_state(|state| {
                state.size = Some(Size::from((expected_width, surface_height)));
                // Now set tiled states so app knows it's width-constrained
                state.states.set(ToplevelState::TiledLeft);
                state.states.set(ToplevelState::TiledRight);
            });
            entry.surface.send_configure();
        }
    }

    /// Cancel pending resizes that have timed out
    pub fn cancel_stale_pending_resizes(&mut self) {
        let now = Instant::now();

        for node in &mut self.layout_nodes {
            if let StackWindow::External(entry) = &mut node.cell {
                if let WindowState::PendingResize { current_height, requested_at, .. } = &entry.state {
                    let elapsed = now.duration_since(*requested_at).as_millis();
                    if elapsed > RESIZE_TIMEOUT_MS {
                        tracing::warn!(
                            elapsed_ms = elapsed,
                            current_height,
                            "cancelling stale pending resize - client did not respond"
                        );
                        entry.state = WindowState::Active { height: *current_height };
                    }
                }
            }
        }
    }

    /// Find resize handle at screen Y coordinate, returns window index above the handle
    pub fn find_resize_handle_at(&self, screen_y: ScreenY) -> Option<usize> {
        let screen_y_value = screen_y.value() as i32;

        // Don't allow resizing the last cell (no border below it)
        if self.layout_nodes.len() < 2 {
            tracing::debug!(
                screen_y = screen_y_value,
                cells = self.layout_nodes.len(),
                "find_resize_handle_at: too few cells"
            );
            return None;
        }

        let mut content_y = -(self.scroll_offset as i32);
        let half_handle = RESIZE_HANDLE_SIZE / 2;

        tracing::debug!(
            screen_y = screen_y_value,
            scroll_offset = self.scroll_offset,
            initial_content_y = content_y,
            half_handle,
            "find_resize_handle_at: starting search"
        );

        for i in 0..self.layout_nodes.len() {
            // Use layout_nodes height which includes title bar for terminals
            let height = self.layout_nodes[i].height;
            let bottom_y = content_y + height;

            tracing::debug!(
                i,
                height,
                content_y,
                bottom_y,
                handle_min = bottom_y - half_handle,
                handle_max = bottom_y + half_handle,
                screen_y = screen_y_value,
                in_range = (screen_y_value >= bottom_y - half_handle && screen_y_value <= bottom_y + half_handle),
                "find_resize_handle_at: checking cell"
            );

            // Check if screen_y is in the handle zone around this cell's bottom edge
            // But not for the last cell (nothing below to resize into)
            if i < self.layout_nodes.len() - 1
                && screen_y_value >= bottom_y - half_handle
                && screen_y_value <= bottom_y + half_handle
            {
                tracing::debug!(
                    i,
                    screen_y = screen_y_value,
                    bottom_y,
                    "resize handle found at cell index"
                );
                return Some(i);
            }

            content_y = bottom_y;
        }

        tracing::debug!(screen_y = screen_y_value, "find_resize_handle_at: no handle found");
        None
    }

    /// Find which window is at a given screen Y coordinate (Y=0 at top).
    ///
    /// Uses the same layout walk as `find_resize_handle_at` but returns the
    /// window index whose vertical extent contains the point.
    pub fn window_at_screen_y(&self, screen_y: ScreenY) -> Option<usize> {
        let screen_y_value = screen_y.value() as i32;
        let mut content_y = -(self.scroll_offset as i32);

        for (i, node) in self.layout_nodes.iter().enumerate() {
            let bottom_y = content_y + node.height;

            if screen_y_value >= content_y && screen_y_value < bottom_y {
                return Some(i);
            }

            content_y = bottom_y;
        }

        None
    }

    /// Clear resize drag if the dragged window no longer exists or has moved.
    ///
    /// Call this after removing windows from layout_nodes. The identity check
    /// ensures we don't accidentally resize the wrong window if indices shifted.
    pub fn clear_stale_resize_drag(&mut self) {
        let Some(drag) = &self.resizing else { return };

        let is_valid = if let Some(node) = self.layout_nodes.get(drag.window_index) {
            // Verify the identity at the stored index still matches
            match (&node.cell, &drag.window_identity) {
                (StackWindow::Terminal(id), FocusedWindow::Terminal(drag_id)) => id == drag_id,
                (StackWindow::External(entry), FocusedWindow::External(drag_id)) => {
                    entry.surface.wl_surface().id() == *drag_id
                }
                _ => false,
            }
        } else {
            false
        };

        if !is_valid {
            tracing::info!(
                window_index = drag.window_index,
                "clearing stale resize drag (window removed or shifted)"
            );
            self.resizing = None;
        }
    }

    /// Clear resize drag if it targets the given terminal ID
    pub fn clear_resize_drag_for_terminal(&mut self, id: TerminalId) {
        if let Some(drag) = &self.resizing {
            if drag.window_identity == FocusedWindow::Terminal(id) {
                tracing::info!(
                    terminal_id = id.0,
                    "clearing resize drag for removed terminal"
                );
                self.resizing = None;
            }
        }
    }
}
