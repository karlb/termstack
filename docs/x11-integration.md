# X11/XWayland Integration Notes

This document captures hard-won knowledge about X11 window integration in this Smithay-based Wayland compositor.

## Architecture Overview

X11 applications connect to our compositor via XWayland, which acts as a bridge:

```
X11 App <-> XWayland <-> Smithay Compositor <-> Wayland Protocol
```

- XWayland creates a Wayland surface for each X11 window
- Smithay's `X11Wm` handles X11 window management via the `XwmHandler` trait
- X11 windows appear as `SurfaceKind::X11(X11Surface)` in our cell list

## Issue 1: X11 Windows Don't Resize Properly

### Symptoms
- Dragging resize handle moves cell boundary, but X11 window content stays at original size
- Gap appears between window content and cell boundary
- Simple apps (xeyes, xclock) affected; some complex apps (mupdf) partially work

### Root Cause
X11 apps only redraw when they receive **Expose events**. Unlike Wayland's configure/ack protocol, X11 apps passively wait to be told to redraw.

When we call `x11_surface.configure()`:
1. ConfigureNotify event is sent to the app (window geometry changed)
2. App updates its internal size knowledge
3. But app does NOT redraw unless it receives an Expose event

Smithay does NOT implement `_NET_WM_SYNC_REQUEST` protocol, which would synchronize resizes. Smithay also doesn't expose the X11 connection for sending custom events.

### Solution: Send Expose Events Manually

We create our own x11rb connection to XWayland and send Expose events after configure:

```rust
// In state.rs - when XWayland is ready:
let display = format!(":{}", display_number);
match x11rb::connect(Some(&display)) {
    Ok((conn, _screen)) => {
        compositor.x11_conn = Some(Arc::new(conn));
    }
    // ...
}

// In request_resize() after x11.configure():
if let Some(ref conn) = self.x11_conn {
    let expose_event = ExposeEvent {
        response_type: EXPOSE_EVENT,
        window: x11.window_id(),
        x: 0, y: 0,
        width: width as u16,
        height: clamped_height as u16,
        count: 0,
    };
    let serialized = expose_event.serialize();
    let mut event_bytes = [0u8; 32];  // X11 events must be 32 bytes
    event_bytes[..serialized.len()].copy_from_slice(&serialized);
    conn.send_event(false, window_id, EventMask::EXPOSURE, event_bytes)?;
    conn.flush()?;
}
```

### Throttling

X11 apps can be overwhelmed by rapid configure requests during drag. We throttle to ~30fps:

```rust
const MIN_CONFIGURE_INTERVAL: Duration = Duration::from_millis(33);

if let Some(last) = self.last_x11_configure {
    if last.elapsed() < MIN_CONFIGURE_INTERVAL {
        return;  // Skip this configure
    }
}
self.last_x11_configure = Some(Instant::now());
```

## Issue 2: X11 Windows Render Upside Down

### Symptoms
- X11 window content appears vertically flipped
- Text and images are inverted
- Wayland windows render correctly

### Root Cause
Coordinate system mismatch:
- OpenGL (Smithay renderer): Y=0 at **bottom** of screen
- X11/XWayland buffers: Y=0 at **top** of buffer

`WaylandSurfaceRenderElement::draw()` passes the element's `buffer_transform` to `render_texture_from_to()`. For X11 surfaces, this is typically `Transform::Normal`, but we need `Transform::Flipped180` to flip the Y axis.

### Failed Approach: Negative Source Rectangle

Initial attempt tried flipping via negative height in source rectangle:
```rust
// THIS CAUSES A PANIC - Size cannot have negative dimensions
let flipped_src = Rectangle::new(
    Point::from((src.loc.x, src.loc.y + src.size.h)),
    Size::from((src.size.w, -src.size.h)),  // PANIC!
);
```

### Working Solution: Access Texture Directly

For X11 surfaces, bypass `element.draw()` and render the texture directly with the correct transform:

```rust
// In render.rs - render_external():
if is_x11 {
    match element.texture() {
        WaylandSurfaceTexture::Texture(texture) => {
            frame.render_texture_from_to(
                texture,
                src_rect,
                dest,
                &[damage],
                &[],
                Transform::Flipped180,  // Apply Y-flip
                1.0,
                None,
                &[],
            ).ok();
        }
        WaylandSurfaceTexture::SolidColor(color) => {
            frame.draw_solid(dest, &[damage], *color).ok();
        }
    }
} else {
    // Wayland surfaces use element.draw() which respects buffer_transform
    element.draw(frame, src, dest, &[damage], &[]).ok();
}
```

Key insight: `WaylandSurfaceRenderElement::texture()` is a public method that returns `&WaylandSurfaceTexture<R>`, allowing direct access to the underlying texture.

## Key Types and Traits

### SurfaceKind (state.rs)
```rust
pub enum SurfaceKind {
    Wayland(ToplevelSurface),
    X11(X11Surface),
}

impl SurfaceKind {
    pub fn is_x11(&self) -> bool { ... }
    pub fn wl_surface(&self) -> Option<WlSurface> { ... }
}
```

### X11Surface (from Smithay)
- `window_id()` - X11 window ID for sending events
- `configure(Rectangle)` - Request window geometry change
- `wl_surface()` - Get underlying Wayland surface (if mapped)

### WaylandSurfaceTexture (from Smithay)
```rust
pub enum WaylandSurfaceTexture<R: Renderer> {
    Texture(R::TextureId),
    SolidColor(Color32F),
}
```

## Smithay Limitations

1. **No `_NET_WM_SYNC_REQUEST`**: Can't synchronize resize with X11 app
2. **No X11 connection access**: Must create our own x11rb connection
3. **No transform control in element.draw()**: Must access texture directly for custom transforms

## Dependencies

```toml
x11rb = { version = "0.13", features = ["allow-unsafe-code"] }
```

Required imports for X11 event handling:
```rust
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, ExposeEvent, EventMask, EXPOSE_EVENT};
use x11rb::rust_connection::RustConnection;
use x11rb::x11_utils::Serialize;
```

## Testing X11 Integration

Good test applications:
- `xeyes` - Simple, redraws on Expose, good for resize testing
- `xclock` - Similar to xeyes
- `mupdf` - More complex, tests document rendering orientation
- `pqiv` - Image viewer, tests image orientation

Test script example:
```bash
WINIT_UNIX_BACKEND=x11 RUST_LOG=column_compositor=info \
    ./target/release/column-compositor &
sleep 3
DISPLAY=:2 xeyes &
DISPLAY=:2 mupdf ~/some.pdf &
```

## Future Improvements

1. Implement frame synchronization protocol for smoother resizes
2. Consider using `_XWAYLAND_ALLOW_COMMITS` to gate buffer updates
3. Investigate why some X11 apps show blank initially (timing issue?)
