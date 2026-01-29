//! Window icon and class for the compositor.
//!
//! Sets X11 window properties for proper desktop integration:
//! - `_NET_WM_ICON`: Window icon data for window managers
//! - `WM_CLASS`: Application class for .desktop file matching (GNOME, etc.)

use std::sync::Arc;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Application class name for WM_CLASS property.
/// Must match StartupWMClass in the .desktop file.
const WM_CLASS: &[u8] = b"termstack\0termstack\0";

/// Icon data at multiple sizes for different display contexts.
/// Raw BGRA format: on little-endian, bytes [B,G,R,A] read as u32 = 0xAARRGGBB.
const ICON_48: &[u8] = include_bytes!("../../../assets/termstack-icon-48.raw");
const ICON_64: &[u8] = include_bytes!("../../../assets/termstack-icon-64.raw");
const ICON_128: &[u8] = include_bytes!("../../../assets/termstack-icon-128.raw");

/// Append icon data for one size to the icon buffer.
fn append_icon(icon_data: &mut Vec<u32>, size: u32, raw_bytes: &[u8]) {
    icon_data.push(size);
    icon_data.push(size);
    for chunk in raw_bytes.chunks_exact(4) {
        icon_data.push(u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
}

/// Set the window icon using the _NET_WM_ICON property.
///
/// Includes multiple sizes (48, 64, 128) so the window manager can choose
/// the best size for different contexts (taskbar, alt-tab, etc.).
pub fn set_window_icon(
    connection: &Arc<RustConnection>,
    window_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let atom_cookie = connection.intern_atom(false, b"_NET_WM_ICON")?;
    let atom = atom_cookie.reply()?.atom;

    // Build icon data with multiple sizes: [w1, h1, pixels1..., w2, h2, pixels2..., ...]
    let total_pixels = 48 * 48 + 64 * 64 + 128 * 128;
    let mut icon_data: Vec<u32> = Vec::with_capacity(6 + total_pixels);

    append_icon(&mut icon_data, 48, ICON_48);
    append_icon(&mut icon_data, 64, ICON_64);
    append_icon(&mut icon_data, 128, ICON_128);

    connection.change_property32(
        PropMode::REPLACE,
        window_id,
        atom,
        AtomEnum::CARDINAL,
        &icon_data,
    )?;

    connection.flush()?;

    tracing::debug!("window icon set (48, 64, 128)");
    Ok(())
}

/// Set the WM_CLASS property for desktop file matching.
///
/// GNOME and other desktops use WM_CLASS to match windows to .desktop files,
/// which determines the application icon shown in the task switcher.
pub fn set_window_class(
    connection: &Arc<RustConnection>,
    window_id: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    connection.change_property8(
        PropMode::REPLACE,
        window_id,
        AtomEnum::WM_CLASS,
        AtomEnum::STRING,
        WM_CLASS,
    )?;

    connection.flush()?;

    tracing::debug!("window class set to 'termstack'");
    Ok(())
}
