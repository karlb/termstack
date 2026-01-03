# X11/XWayland Integration Notes

---

## Current: xwayland-satellite (2026-01-03+)

**As of 2026-01-03, we use [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) for X11 support.**

X11 windows are presented as normal Wayland `ToplevelSurface` windows - all the issues documented below are historical and no longer apply.

**Current Architecture:**
```
X11 App <-> XWayland <-> xwayland-satellite <-> Compositor
                         (acts as WM + Wayland client)
```

**Why we switched:**
- Smithay's X11Wm integration is fundamentally incomplete for compositor-initiated resizes (see Issue 8 below)
- X11 windows now "just work" through the Wayland protocol
- Removed ~750 lines of X11-specific workaround code
- Production-tested solution (used by niri and other compositors)

**Implementation:** See `crates/compositor/src/main.rs:initialize_xwayland()` for:
- XWayland spawn with on-disk socket
- xwayland-satellite lifecycle management
- Auto-restart on crash
- Soft dependency handling (warns if missing, continues in Wayland-only mode)

**Testing:** See `crates/test-harness/tests/x11_integration.rs` for integration tests.

---

## Historical: Smithay X11Wm Integration (Removed 2026-01-03)

**The sections below document our previous Smithay X11Wm integration and the issues we encountered.**

This document captures hard-won knowledge about X11 window integration using Smithay's X11Wm.

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

## Issue 3: Infinite Scroll Adjustment Loop (TITLE_BAR_HEIGHT Double-Add Bug)

### Symptoms
- When launching mupdf or other X11 apps, the compositor enters an infinite scroll adjustment loop
- Window grows by 24 pixels every frame
- Logs show continuous `Adjusting scroll by 24 pixels` messages

### Root Cause
`TITLE_BAR_HEIGHT` (24 pixels) was being added twice for X11 SSD (Server-Side Decoration) windows:
1. **First addition**: In `configure_notify()` (xwayland.rs), visual_height calculation:
   ```rust
   let visual_height = geometry.size.h + TITLE_BAR_HEIGHT as i32;
   node.height = visual_height;
   ```
2. **Second addition**: In `collect_cell_data()` (render.rs), when building render data:
   ```rust
   // BUG: Adding title bar height again!
   let actual_height = window_height + TITLE_BAR_HEIGHT as i32;
   ```

This caused the window to grow by 24 pixels every frame in the layout calculation.

### Solution
Make `collect_cell_data()` return `node.height` directly for X11 windows, since it already includes the title bar:

```rust
// In render.rs - collect_cell_data():
let actual_height = match &entry.surface {
    crate::state::SurfaceKind::X11(_) => {
        // X11: node.height is already the visual height (content + title bar for SSD)
        node.height
    }
    crate::state::SurfaceKind::Wayland(_) => {
        if entry.uses_csd {
            window_height
        } else {
            window_height + TITLE_BAR_HEIGHT as i32
        }
    }
};
```

Also ensure `add_x11_window()` sets initial height correctly:

```rust
// In xwayland.rs - add_x11_window():
let visual_height = if uses_csd {
    initial_height as i32
} else {
    initial_height as i32 + crate::title_bar::TITLE_BAR_HEIGHT as i32
};
self.layout_nodes.insert(insert_index, LayoutNode {
    cell: ColumnCell::External(Box::new(entry)),
    height: visual_height,  // Already includes title bar
});
```

## Issue 4: X11 Content Renders at Bottom of Cell

### Symptoms
- With `Transform::Flipped180` applied to fix upside-down rendering, X11 window content appears at the bottom of its cell
- Gap appears at the top where the content should be
- Title bar is in correct position, but content area is misaligned

### Root Cause
`Transform::Flipped180` flips the texture on the Y-axis. In OpenGL coordinates (Y=0 at bottom):
- The **top edge** of the flipped texture appears at `dest.y + dest.height`
- The **bottom edge** appears at `dest.y`

The old code positioned content using:
```rust
let dest_y = geo.loc.y + y;  // This puts top of texture at bottom of cell
```

For a cell at render position `y` with height `height`, and content with geometry height `geo.size.h`:
- Cell bottom is at `y`
- Cell top is at `y + height`
- Content should fill from `y` (bottom) to `y + height - TITLE_BAR_HEIGHT` (top of content area)

### Solution
Calculate `dest_y` to align the top of the flipped content with the top of the content area:

```rust
// In render.rs - render_external():
let dest_y = if is_x11 && !uses_csd {
    // X11 SSD: align content to top of content area (below our title bar)
    // Top of content area is at: y + height - TITLE_BAR_HEIGHT
    // For flipped texture, top appears at dest_y + geo.size.h
    // So: dest_y + geo.size.h = y + height - TITLE_BAR_HEIGHT
    // Therefore: dest_y = y + height - TITLE_BAR_HEIGHT - geo.size.h
    y + height - TITLE_BAR_HEIGHT as i32 - geo.size.h
} else if is_x11 {
    // X11 CSD: no title bar from us, align to cell top
    y + height - geo.size.h
} else {
    // Wayland: use element's natural position
    geo.loc.y + y
};
```

This also required adding `uses_csd` to `CellRenderData::External` enum to distinguish between SSD and CSD X11 windows.

## Issue 5: Crash When Focusing X11 Window Too Early

### Symptoms
- Panic when launching mupdf as foreground GUI: `panic: external window should have wl_surface when focused`
- Crash occurs in `set_focus_by_index()` at state.rs:1342
- Happens because X11 windows don't have `wl_surface()` immediately after `map_window_request`

### Root Cause
In `add_x11_window()`, for foreground GUI windows, we call `set_focus_by_index()` immediately. But X11 windows may not have their Wayland surface ready yet:

```rust
// In add_x11_window():
if is_foreground_gui {
    self.set_focus_by_index(insert_index);  // May crash if wl_surface not ready
}
```

The panic was:
```rust
ColumnCell::External(entry) => {
    let surface = entry.surface.wl_surface()
        .expect("external window should have wl_surface when focused");
    // ...
}
```

### Solution
Handle missing `wl_surface` gracefully by skipping focus and logging a warning:

```rust
// In state.rs - set_focus_by_index():
ColumnCell::External(entry) => {
    // External windows should have a wl_surface by the time we focus them.
    // For X11 windows, the surface might not be ready immediately after map_window_request,
    // so we skip focusing if it's not ready yet. Focus will be set on the next interaction.
    let Some(surface) = entry.surface.wl_surface() else {
        tracing::warn!(
            index,
            "external window doesn't have wl_surface yet, skipping focus"
        );
        return;
    };
    FocusedCell::External(surface.id())
}
```

Focus will be properly set when the user next interacts with the window.

## Issue 6: Configure Notification Height Mismatch

### Symptoms
- Logs show: `configure_notify doesn't match pending resize, ignoring stale event requested_height=225 notified_height=201`
- X11 app responds to configure but we reject it as stale
- Window stays at old size

### Root Cause
Visual height vs content height confusion. For X11 SSD windows:
- **Visual height** = content height + TITLE_BAR_HEIGHT (what we store in `node.height`)
- **Content height** = what we send to the X11 app via `configure()`

The bug was storing visual height in `requested_height`:
```rust
// BUG: Storing visual height (225 = 201 + 24)
entry.state = WindowState::PendingResize {
    requested_height: new_height,  // new_height is visual height
    // ...
};

// But we sent content height to X11:
let clamped_height = new_height.saturating_sub(TITLE_BAR_HEIGHT);
x11.configure(Rectangle::from_loc_and_size((0, 0), (width, clamped_height)))?;
```

When mupdf responded with `configure_notify` height=201, we compared `201 == 225` and rejected it.

### Solution
Store the **actual height we sent to X11** in `requested_height`:

```rust
// In state.rs - request_resize():
// For X11 SSD windows, we send content_height (without title bar) to the X11 window.
// We must store the ACTUAL height we sent, not the visual height, so configure_notify
// matching works correctly.
let actual_requested_height = if matches!(&entry.surface, SurfaceKind::X11(_)) && !entry.uses_csd {
    // X11 SSD: we sent content_height = new_height - TITLE_BAR_HEIGHT
    new_height.saturating_sub(crate::title_bar::TITLE_BAR_HEIGHT)
} else {
    // Wayland or X11 CSD: we sent new_height as-is
    new_height
};

entry.state = WindowState::PendingResize {
    current_height: current,
    requested_height: actual_requested_height,  // Store what we actually sent
    request_serial: serial,
    requested_at: Instant::now(),
};
```

Now when mupdf responds with height=201, we compare `201 == 201` and accept it.

## Issue 7: Height Comparison Bug Causing Redundant Resizes

### Symptoms
- Logs show continuous resize requests even though window hasn't moved:
  ```
  current_height=285 new_height=312
  current_height=288 new_height=315
  ```
- Heights never match, causing resize requests in a loop
- X11 protocol works correctly (configure_notify matches), but we keep sending more

### Root Cause
Early-return check comparing different height types. `WindowState::PendingResize` and `WindowState::Active` store **content height** for X11 SSD windows, but `new_height` parameter is **visual height**:

```rust
// BUG: Comparing content height (288) vs visual height (312)
match &entry.state {
    WindowState::Active { height: current }
    | WindowState::PendingResize { current_height: current, .. } => {
        if *current == new_height {  // Never matches!
            return;
        }
    }
}
```

### Solution
Convert `new_height` to content height before comparison:

```rust
// In state.rs - request_resize():
// For X11 SSD windows, entry.state stores content height, but new_height is visual height.
// Convert new_height to content height for comparison.
let new_height_for_comparison = if matches!(&entry.surface, SurfaceKind::X11(_)) && !entry.uses_csd {
    new_height.saturating_sub(crate::title_bar::TITLE_BAR_HEIGHT)
} else {
    new_height
};

match &entry.state {
    WindowState::Active { height: current }
    | WindowState::PendingResize { current_height: current, .. } => {
        if *current == new_height_for_comparison {
            tracing::trace!("request_resize: height unchanged ({})", current);
            return;
        }
    }
}
```

Now heights match correctly and we only send resize when the size actually changes.

## Height Consistency: Visual vs Content

For X11 windows with Server-Side Decorations (SSD), we maintain two height concepts:

1. **Visual height** (`node.height`): Content + title bar, used for layout and rendering
2. **Content height** (sent to X11): Excludes title bar, what the X11 app actually renders

Key points:
- `LayoutNode.height` always stores visual height
- `WindowState::Active.height` and `WindowState::PendingResize.requested_height` store content height for X11 SSD
- When converting between them: `content_height = visual_height - TITLE_BAR_HEIGHT`
- Always compare heights of the same type (both visual or both content)
- CSD windows (both X11 and Wayland) don't have this distinction - height is height

## Issue 8: Compositor-Initiated Resize Doesn't Work (Fundamental Smithay Limitation)

### Symptoms
- When compositor calls `x11.configure()` to resize an X11 window, no `configure_notify` response is received
- X11 window content never updates to the new size
- The issue persists even with only ONE configure request (no throttling/flooding)
- Cell boundary moves but window content stays at original size
- Works fine in GNOME/Mutter but fails in our Smithay-based compositor

### Root Cause
**Smithay's X11Wm integration is fundamentally incomplete for tiling window managers.**

We discovered this by investigating how other Smithay-based compositors handle X11:
- **Niri** (production Smithay compositor) deliberately **avoids** using Smithay's X11Wm
- Instead, niri uses [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite)
- Reason: "X11 is very cursed" - xwayland-satellite handles X11 peculiarities

### What We Tried (All Failed)
1. **Throttling configure requests** - limited to ~30fps (33ms interval) - still no response
2. **Single configure at drag end** - only ONE request when button released - still no response
3. **Using ClearArea with exposures=true** - instead of manual Expose events - still no response
4. **Direct x11rb ConfigureWindow** - bypassing Smithay's configure() - still no response
5. **Both Smithay and x11rb** - trying both methods together - still no response

### Diagnostic Findings
When resizing mupdf:
- Configure requests ARE sent: `sending X11 configure for resize current_height=201 requested_visual_height=234`
- Window is properly reparented: `frame_id=Some(2097158)`
- No size constraints blocking resize: `min_h=None max_h=None`
- Window has Wayland surface: `has_wl_surface=true is_mapped=true`
- **But NO configure_notify responses ever received**

This indicates Smithay's `X11Surface::configure()` is non-functional for compositor-initiated resizes of reparented windows.

### The Solution: xwayland-satellite

[xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) is a bridge that converts X11 windows into normal Wayland windows:

**Architecture:**
```
Without satellite:  X11 App <-> XWayland <-> Smithay X11Wm <-> Compositor
                                             ^^^^^^^^^^^^^^^^
                                             (incomplete, broken)

With satellite:     X11 App <-> XWayland <-> xwayland-satellite <-> Compositor
                                             (acts as WM + Wayland client)
```

**How it works:**
1. xwayland-satellite acts as the X11 window manager for XWayland
2. It handles all X11 protocol complexity (configure requests, events, etc.)
3. It presents X11 windows as normal Wayland `ToplevelSurface` windows to the compositor
4. The compositor treats them like any other Wayland window (resize just works)

**Benefits:**
- X11 windows appear as regular Wayland windows (our existing resize code works)
- No need to handle X11 quirks (height conversions, reparenting, etc.)
- Production-tested (used by niri and other compositors)
- Maintained separately from Smithay

**Integration:**
- Requires compositor to implement `xdg_wm_base` and `viewporter` protocols (we already do)
- Launches as a separate process after compositor starts
- Compositor creates X11 socket and exports `$DISPLAY`
- xwayland-satellite spawns on-demand when X11 apps launch

### References
- [niri Xwayland integration docs](https://yalter.github.io/niri/Xwayland.html) - Production compositor using satellite
- [xwayland-satellite README](https://github.com/Supreeeme/xwayland-satellite) - Bridge implementation
- [Smithay XwmHandler docs](https://smithay.github.io/smithay/smithay/xwayland/xwm/trait.XwmHandler.html) - Current (incomplete) approach
- [Smithay X11Wm PR](https://github.com/Smithay/smithay/pull/570) - Original implementation (noted as basic, works for "simple clients")

## Historical Future Improvements

These were planned improvements for Smithay X11Wm integration. **We've now switched to xwayland-satellite**, which eliminates the need for most of these workarounds:

1. ~~**Switch to xwayland-satellite** for proper X11 support (recommended)~~ **✅ IMPLEMENTED (2026-01-03)**
2. ~~Implement frame synchronization protocol for smoother resizes~~ **No longer needed** - xwayland-satellite handles this
3. ~~Consider using `_XWAYLAND_ALLOW_COMMITS` to gate buffer updates~~ **No longer needed**
4. ~~Investigate why some X11 apps show blank initially~~ **No longer needed** - Wayland protocol handles timing
5. ~~Consider unifying height storage to always use one type~~ **No longer needed** - unified to visual height only
