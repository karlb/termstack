//! X11 cursor management for the compositor.
//!
//! This module handles loading cursor themes and changing the cursor icon
//! when hovering over resize handles.

use std::sync::Arc;
use x11rb::connection::Connection;
use x11rb::cursor::Handle as CursorHandle;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, Cursor};
use x11rb::resource_manager::new_from_default;
use x11rb::rust_connection::RustConnection;

/// Manages X11 cursor state for the compositor window.
pub struct CursorManager {
    /// The underlying X11 connection
    connection: Arc<RustConnection>,
    /// The X11 window ID
    window_id: u32,
    /// Default cursor (arrow)
    default_cursor: Cursor,
    /// Resize cursor (vertical resize)
    resize_cursor: Cursor,
    /// Current cursor state
    current_is_resize: bool,
}

impl CursorManager {
    /// Create a new cursor manager.
    ///
    /// Loads the default cursor and resize cursor from the current theme.
    pub fn new(
        connection: Arc<RustConnection>,
        screen: usize,
        window_id: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        // Load resource database for cursor theme settings
        let database = new_from_default(&*connection)?;

        // Create cursor handle for loading cursors
        let cursor_handle = CursorHandle::new(&connection, screen, &database)?.reply()?;

        // Load default cursor (left_ptr/arrow)
        let default_cursor = cursor_handle.load_cursor(&connection, "left_ptr")?;
        tracing::info!(cursor = default_cursor, "loaded default cursor");

        // Load resize cursor - try several names that cursor themes typically provide
        // Priority: row-resize (standard), sb_v_double_arrow (X11), ns-resize (CSS-style)
        let resize_cursor = cursor_handle
            .load_cursor(&connection, "row-resize")
            .or_else(|_| cursor_handle.load_cursor(&connection, "sb_v_double_arrow"))
            .or_else(|_| cursor_handle.load_cursor(&connection, "ns-resize"))
            .or_else(|_| cursor_handle.load_cursor(&connection, "size_ver"))
            .unwrap_or(default_cursor);
        tracing::info!(cursor = resize_cursor, "loaded resize cursor");

        Ok(Self {
            connection,
            window_id,
            default_cursor,
            resize_cursor,
            current_is_resize: false,
        })
    }

    /// Update the cursor to show the resize cursor or default cursor.
    ///
    /// Only sends X11 requests if the cursor state actually changes.
    pub fn set_resize_cursor(&mut self, on_resize_handle: bool) {
        if self.current_is_resize == on_resize_handle {
            return; // No change needed
        }

        let cursor = if on_resize_handle {
            self.resize_cursor
        } else {
            self.default_cursor
        };

        // Update window cursor attribute
        let values = ChangeWindowAttributesAux::new().cursor(cursor);
        if let Err(e) = self.connection.change_window_attributes(self.window_id, &values) {
            tracing::warn!(?e, "failed to change cursor");
            return;
        }

        // Flush to ensure the cursor change is visible immediately
        if let Err(e) = self.connection.flush() {
            tracing::warn!(?e, "failed to flush connection after cursor change");
            return;
        }

        self.current_is_resize = on_resize_handle;
        tracing::debug!(on_resize_handle, cursor, "cursor updated");
    }
}
