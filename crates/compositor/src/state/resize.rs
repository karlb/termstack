//! Resize handling for TermStack
//!
//! Handles window resize requests, resize completion tracking, and resize handle detection.

use std::time::Instant;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Size, SERIAL_COUNTER};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use super::{StackWindow, TermStack, WindowState};
use crate::coords::ScreenY;

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

        // Get the committed size
        let committed_size: Option<Size<i32, smithay::utils::Logical>> = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok())
                .and_then(|data| data.current.size)
        });

        if let Some(size) = committed_size {
            let committed_surface_height = size.h as u32;

            // For SSD windows, the total cell height includes the title bar
            // For CSD windows, surface height = cell height
            let committed_window_height = if entry.uses_csd {
                committed_surface_height
            } else {
                committed_surface_height + crate::title_bar::TITLE_BAR_HEIGHT
            };

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
}
