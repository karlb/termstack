# TermStack Architecture

This document describes the architecture, coordinate systems, state machines, and design decisions for the TermStack Wayland compositor.

## Overview

TermStack is a Wayland compositor that arranges terminal windows in a scrollable vertical column. It's built with Smithay and uses explicit state machines to prevent bugs from implicit state changes.

**Design Philosophy:**
- **Pure functions** for layout calculation (no side effects)
- **Explicit state machines** for terminal sizing
- **Type-safe coordinate wrappers** to prevent mixing coordinate systems
- **DRY principle** for maintainability

## Coordinate Systems

TermStack uses three distinct coordinate systems that must never be mixed. Type-safe wrappers (`ScreenY`, `RenderY`, `ContentY`) enforce this at compile time.

### Coordinate System Diagram

```
┌─────────────────────────────────────────────────────────────┐
│ Mouse Event (Screen Coordinates)                            │
│ Y=0 at TOP of screen (Winit convention)                     │
│                                                              │
│ screen_y = 0    ┌──────────── Top of screen                │
│                  │                                           │
│ screen_y = 300   │  Middle                                  │
│                  │                                           │
│ screen_y = 600   └──────────── Bottom of screen             │
└─────────────────────────────────────────────────────────────┘
                          ↓
                 Y-Flip Transformation
            render_y = screen_height - screen_y
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ Render Coordinates (OpenGL/Smithay)                         │
│ Y=0 at BOTTOM of screen (OpenGL convention)                 │
│                                                              │
│ render_y = 600   ┌──────────── Top of screen                │
│                  │                                           │
│ render_y = 300   │  Middle                                  │
│                  │                                           │
│ render_y = 0     └──────────── Bottom of screen             │
└─────────────────────────────────────────────────────────────┘
                          ↓
                 Scroll Offset Applied
            content_y = render_y + scroll_offset
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ Content Coordinates (Absolute Position)                     │
│ Absolute position in scrollable content space               │
│                                                              │
│ content_y = 900  ┌──────────── Window above viewport        │
│                  │                                           │
│ ─────────────────┤ Viewport starts (scroll_offset = 300)    │
│ content_y = 600  │                                           │
│                  │  Visible content                          │
│ content_y = 300  │                                           │
│ ─────────────────┘ Viewport ends                            │
│ content_y = 0        Window below viewport                  │
└─────────────────────────────────────────────────────────────┘
```

### Key Formulas

```rust
// Y-flip: Screen to Render
render_y = screen_height - screen_y

// Scroll offset: Render to Content
content_y = render_y + scroll_offset

// Window positioning with Y-flip
render_y = screen_height - content_y - window_height

// Surface-local coordinates for Wayland windows
render_end = screen_height - content_y
surface_local_y = render_end - render_y
```

### INVARIANTS

**Input Processing (input.rs:522-524):**
```rust
/// INVARIANT: Mouse events arrive in screen coordinates (Y=0 at top).
/// Must convert to render coordinates (Y=0 at bottom) before any layout operations.
/// The Y-flip formula is: render_y = screen_height - screen_y
```

**Layout Nodes (state/mod.rs:153-154):**
```rust
/// INVARIANT: layout_nodes[0] renders at highest Y (top of screen after Y-flip).
/// After any mutation (insert/remove), must call recalculate_layout() to update positions.
```

## Terminal Sizing State Machine

The terminal sizing state machine prevents the content-counting bugs from v1 by tracking resize state explicitly and only incrementing content during the Stable state.

### State Diagram

```
                    ┌─────────────────┐
                    │                 │
             ┌──────┤     Stable      │◄─────┐
             │      │                 │      │
             │      └────────┬────────┘      │
             │               │               │
             │  on_new_line()│               │
             │  (content_rows│               │ on_resize_complete()
             │   increments) │               │ (restore scrollback)
             │               │               │
             │      content_rows >           │
             │      configured_rows?         │
             │               │               │
             │               ▼               │
             │      ┌─────────────────┐      │
             │      │ GrowthRequested │      │
             │      └────────┬────────┘      │
             │               │               │
             │               │ request_growth(new_rows)
             │               │               │
             │               ▼               │
             │      ┌─────────────────┐      │
             │      │                 │      │
             └─────►│    Resizing     │──────┘
       on_new_line()│                 │
       (accumulate  │  (pending_      │
        in pending_ │   scrollback    │
        scrollback) │   tracks new    │
                    │   lines during  │
                    │   resize)       │
                    └─────────────────┘
                             │
                             │ on_configure(rows)
                             │ (compositor acks resize)
                             │
                             ▼
                    (back to Resizing,
                     waiting for resize_complete)
```

### State Transitions

**Stable → GrowthRequested:**
- Trigger: `content_rows > configured_rows` (terminal content exceeds PTY size)
- Action: Return `SizingAction::RequestGrowth { new_rows }`

**GrowthRequested → Resizing:**
- Trigger: `request_growth(new_rows)` called by compositor
- Action: Transition to Resizing state, wait for configure

**Resizing → Resizing:**
- Trigger: `on_configure(rows)` (compositor acknowledges resize)
- Action: Update `configured_rows`, still waiting for resize_complete

**Resizing → Stable:**
- Trigger: `on_resize_complete()` (resize operation finished)
- Action: Return `SizingAction::RestoreScrollback { lines }` if any lines accumulated

### INVARIANT (terminal_manager/mod.rs:113-116)

```rust
/// INVARIANT: Terminal grid is always 1000 rows (internal alacritty storage),
/// PTY size changes independently via resize operations.
/// Never confuse terminal.grid_rows() with terminal.dimensions().rows.
/// Programs query PTY size via tcgetwinsize, not grid size.
```

The grid stays large (1000 rows) to hold all content. Only PTY size changes on resize. TUI apps query PTY size via `tcgetwinsize`, not the grid size.

## Component Interaction Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│ User Input (Keyboard/Mouse/Scroll)                              │
└──────────────────────┬──────────────────────────────────────────┘
                       │
                       ▼
         ┌─────────────────────────┐
         │   Input Module          │
         │   (input.rs)            │
         │                         │
         │  - Event processing     │
         │  - Coordinate conversion│  ◄─── INVARIANT: Y-flip here
         │  - Window hit testing   │
         │  - Resize drag tracking │
         └────────┬────────────────┘
                  │
                  │ Calls state methods
                  ▼
         ┌─────────────────────────┐
         │   State Module          │
         │   (state/)              │
         │                         │
         │  state/core.rs:         │
         │   - add_window()        │
         │   - remove_window()     │
         │                         │
         │  state/focus.rs:        │
         │   - focus_next/prev()   │
         │   - set_focus_by_index()│
         │                         │
         │  state/resize.rs:       │
         │   - request_resize()    │
         │   - handle_commit()     │
         │                         │
         │  state/external.rs:     │
         │   - activate_toplevel() │
         │   - is_csd_app()        │
         └────────┬────────────────┘
                  │
                  │ State changes trigger
                  │ recalculate_layout()
                  ▼
         ┌─────────────────────────┐
         │   Layout Module         │
         │   (layout.rs)           │
         │                         │
         │  Pure functions:        │  ◄─── No side effects
         │  - window positions     │
         │  - visibility detection │
         │  - scroll calculations  │
         └────────┬────────────────┘
                  │
                  │ Layout positions
                  ▼
         ┌─────────────────────────┐
         │   Render Module         │
         │   (render.rs)           │
         │                         │
         │  - Terminal textures    │
         │  - Title bars           │
         │  - Focus indicators     │
         │  - External windows     │
         │  - Damage tracking      │
         └────────┬────────────────┘
                  │
                  ▼
         ┌─────────────────────────┐
         │   Display (X11/Wayland) │
         └─────────────────────────┘
```

### Data Flow Example: Window Insertion

```
1. User spawns external window (e.g., pqiv image.png)
   └─> XDG shell creates ToplevelSurface

2. Compositor receives xdg_shell::new_toplevel event
   └─> Creates WindowEntry, stores in pending_window state

3. Window commits initial size
   └─> state/core.rs::add_window()
       ├─> Inserts at focused position or output terminal position
       ├─> Stores initial height (200px fallback)
       └─> Calls recalculate_layout()

4. layout.rs::recalculate_layout() (pure function)
   ├─> Calculates content_y for each window
   ├─> Determines visibility in viewport
   └─> Returns LayoutResult with positions

5. render.rs::collect_window_data()
   ├─> Gets actual texture/geometry heights
   ├─> Caches heights for next frame (click detection)
   └─> Prepares render elements

6. render.rs::render_window()
   └─> Uses layout positions + Y-flip to render at correct screen location
```

## Design Decisions

### 1. Pure Layout Functions

**Decision:** Layout calculation is performed by pure functions with no side effects.

**Rationale:**
- Deterministic: same inputs always produce same outputs
- Testable: property-based tests verify invariants
- Debuggable: no hidden state mutations

**Example:**
```rust
pub fn recalculate_layout(
    cells: &[LayoutNode],
    scroll_offset: f64,
    output_height: i32,
) -> LayoutResult {
    // Pure function - no state mutation
    // Returns new positions, doesn't modify input
}
```

### 2. Height Consistency (Click = Render)

**Decision:** Click detection and rendering must use identical heights, cached from the previous frame.

**Rationale:**
- Previous bug: Click detection used `bbox()` height, rendering used actual texture height
- When these differed, clicks would miss windows
- Solution: Cache actual rendered heights (`node.height`), use for both click and render

**Implementation:**
```rust
// render.rs: Cache actual heights from geometry
let actual_height = elements.iter()
    .filter_map(|e| e.geometry(scale))
    .map(|geo| geo.loc.y + geo.size.h)
    .max()
    .unwrap_or(node.height);

// state.rs: Use cached heights for click detection
let height = node.height; // Cached from previous frame
```

### 3. Explicit Sizing State Machine

**Decision:** Terminal sizing uses an explicit state machine (Stable/GrowthRequested/Resizing).

**Rationale:**
- Version 1 bug: Implicit state tracking led to incorrect content_rows counting
- Content could increment during resize, causing mismatch
- Explicit states prevent confusion about when content should increment

**Benefit:** Only increment content_rows in Stable state, track pending scrollback separately during resize.

### 4. Type-Safe Coordinates

**Decision:** Use NewType wrappers (`ScreenY`, `RenderY`, `ContentY`) instead of raw `f64`.

**Rationale:**
- Prevents mixing coordinate systems at compile time
- Self-documenting code (type signature shows coordinate space)
- Centralized conversion logic in wrapper methods

**Example:**
```rust
// Compile error: can't mix coordinate systems
let screen_y = ScreenY::new(100.0);
let render_y: RenderY = screen_y; // ERROR: type mismatch

// Must use explicit conversion
let render_y = screen_y.to_render(output_height);
```

### 5. Centralized Height Calculations

**Decision:** `calculate_window_heights()` centralizes height logic with proper fallbacks.

**Rationale:**
- Single source of truth for height calculation
- Consistent fallback values (`DEFAULT_TERMINAL_HEIGHT`, `node.height`)
- Handles both terminals and external windows uniformly

**Implementation:** See `compositor_main.rs:999-1042`

### 6. Window and Terminal Unification

**Decision:** Terminals and external windows are unified in a single `Vec<LayoutNode>`.

**Rationale:**
- Simplifies layout calculation (one loop for all windows)
- Natural stack ordering (index 0 = top of screen)
- Easy to insert external windows at any position

**Type:**
```rust
pub enum StackWindow {
    Terminal(TerminalId),
    External(WindowEntry),
}

pub struct LayoutNode {
    pub cell: StackWindow,
    pub height: i32,  // Cached from previous frame
}
```

### 7. Focus Persistence Across Mutations

**Decision:** Focus is identity-based (TerminalId or WlSurface), not index-based.

**Rationale:**
- Window can change position (insertions/removals) but focus stays on same window
- Prevents accidental focus jumps when windows are added/removed
- `focused_index()` recalculates index from identity each time

### 8. Module Responsibilities

**Decision:** Split large modules (state.rs 2427→~800 lines each) by responsibility.

**Modules:**
- `state/core.rs` - Window lifecycle (add/remove)
- `state/focus.rs` - Focus management and navigation
- `state/resize.rs` - Resize protocol and state tracking
- `state/external.rs` - External window helpers (CSD detection, activation)

**Rationale:**
- Single Responsibility Principle
- Easier to test individual components
- Clear boundaries prevent scope creep

## File Structure

```
crates/
├── compositor/          # Smithay-based compositor library
│   ├── main.rs         # Entry point, event loop
│   ├── state/          # State machine (split module)
│   │   ├── mod.rs      # TermStack struct, re-exports
│   │   ├── core.rs     # Window lifecycle
│   │   ├── focus.rs    # Focus management
│   │   ├── resize.rs   # Resize handling
│   │   └── external.rs # External window helpers
│   ├── input.rs        # Keyboard/pointer event handling
│   ├── coords.rs       # Type-safe coordinate wrappers
│   ├── layout.rs       # Pure layout calculation
│   ├── render.rs       # Rendering and damage tracking
│   ├── terminal_manager/ # Multiple terminal instances
│   ├── cursor.rs       # Cursor rendering
│   ├── title_bar.rs    # Title bar rendering (fontdue)
│   ├── ipc.rs          # IPC protocol for CLI
│   └── config.rs       # Configuration file handling
│
├── terminal/           # alacritty_terminal wrapper
│   ├── state.rs        # Terminal state + alacritty integration
│   ├── sizing.rs       # TerminalSizingState machine
│   ├── render.rs       # Software renderer (fontdue)
│   └── pty.rs          # PTY management (rustix)
│
├── termstack/          # Unified binary (smart mode detection)
│   ├── main.rs         # Entry point (compositor or CLI)
│   └── cli.rs          # CLI tool for spawning terminals
│
└── test-harness/       # Testing infrastructure
    ├── headless.rs     # TestCompositor mock
    ├── assertions.rs   # Test assertion helpers
    ├── fixtures.rs     # Reusable test scenarios
    ├── live.rs         # Live testing utilities
    └── tests/          # Integration tests
        ├── input_events.rs
        ├── window_positioning.rs
        ├── terminal_state.rs
        └── ...
```

## Testing Strategy

### Property-Based Testing

Use `proptest` to verify invariants hold for arbitrary inputs:

```rust
proptest! {
    #[test]
    fn windows_never_overlap(heights in prop::collection::vec(1u32..500, 1..20)) {
        let layout = calculate_layout(&heights, scroll_offset, output_height);

        // Verify: window_i.end <= window_{i+1}.start for all i
        prop_assert!(layout.check_invariants().is_ok());
    }
}
```

### Edge Case Tests

Test boundary conditions to prevent regressions:

- **Boundary tests:** Zero content, no windows, screen boundaries
- **Edge cases:** 1px windows, windows taller than screen, all windows offscreen
- **State machine:** Rapid resizes, nested resizes, many lines during resize

See: `test-harness/tests/input_events.rs`, `window_positioning.rs`, `terminal_state.rs`

### Test Fixtures

Reusable test scenarios reduce duplication:

```rust
// fixtures.rs
pub fn compositor_with_scrollable_content() -> TestCompositor {
    let mut tc = TestCompositor::new_headless(1280, 720);
    tc.add_external_window(400);
    tc.add_external_window(400);
    tc.add_external_window(400);
    // Total: 1200px, viewport: 720px
    tc
}
```

## Performance Considerations

### Height Caching

**Why:** Avoid recalculating window heights every frame.

**How:**
- `calculate_window_heights()` runs once per layout change
- Results cached in `LayoutNode.height`
- Both rendering and click detection use cached values

### Damage Tracking

**Why:** Only re-render changed regions.

**How:**
- Track dirty regions per window
- Skip rendering windows with no damage
- Accumulate damage across multiple changes

### Pure Layout Functions

**Why:** Enables optimizations (memoization, parallelization).

**How:**
- No side effects = safe to cache results
- Deterministic = can skip recalculation if inputs unchanged
- Future: Could memoize layout for common scroll offsets

## Common Pitfalls

### ❌ Mixing Coordinate Systems

```rust
// WRONG: Mixing screen and render coordinates
let click_y = screen_y; // screen coords (Y=0 top)
if click_y > window_render_y { ... } // render coords (Y=0 bottom)
```

```rust
// CORRECT: Convert to same coordinate system
let render_y = ScreenY::new(screen_y).to_render(output_height);
if render_y > window_render_y { ... }
```

### ❌ Using Different Heights for Click vs Render

```rust
// WRONG: Click uses bbox(), render uses actual geometry
let click_height = window.bbox().height(); // Different value!
let render_height = window.geometry().height();
```

```rust
// CORRECT: Both use cached height from previous frame
let height = node.height; // Same value for both
```

### ❌ Confusing Grid Rows vs PTY Rows

```rust
// WRONG: Using grid size for resize
terminal.resize(terminal.grid_rows(), cols); // Always 1000!
```

```rust
// CORRECT: Use configured PTY size
terminal.resize(terminal.dimensions().rows, cols);
```

### ❌ Incrementing Content During Resize

```rust
// WRONG: Content changes during resize
if state == Resizing {
    content_rows += 1; // Causes mismatch!
}
```

```rust
// CORRECT: Only increment in Stable, track pending during resize
match state {
    Stable => content_rows += 1,
    Resizing => pending_scrollback += 1,
}
```

## Future Improvements

### Potential Optimizations

1. **Layout Memoization:** Cache layout results for common scroll offsets
2. **Partial Layout:** Only recalculate affected windows on insertion
3. **Async PTY Processing:** Move terminal I/O to separate thread
4. **GPU Acceleration:** Use shaders for terminal rendering instead of CPU

### Architecture Evolution

1. **Plugin System:** Allow external windows to register custom renderers
2. **Multi-Column Layout:** Extend from single column to grid layout
3. **Window Decorations:** Customizable title bars and borders
4. **Tiling Modes:** Horizontal splits, floating windows

## References

- **Smithay Documentation:** https://smithay.github.io/smithay/
- **Wayland Protocol:** https://wayland.freedesktop.org/docs/html/
- **XDG Shell Protocol:** https://wayland.app/protocols/xdg-shell
- **alacritty_terminal:** https://docs.rs/alacritty_terminal/

---

**Document Version:** 1.0
**Last Updated:** 2026-01-04
**Maintainer:** TermStack Team
