# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

```bash
cargo build --release           # Build for release
cargo test                      # Run all tests
cargo test --package compositor # Run compositor crate tests only
cargo test --package test-harness --test coordinate_accuracy  # Run specific test file
cargo test some_test_name       # Run tests matching name
cargo clippy                    # linting
```

## Running the Compositor

```bash
cargo run --release --bin column-compositor   # Winit backend (development)
WINIT_UNIX_BACKEND=x11 cargo run --release    # Force X11 backend
RUST_LOG=column_compositor=debug cargo run    # With debug logging
```

## Development approach

- Before fixing bugs, reproduce them inside the test suite.
- Prefer improving the test suite to writing one-off tests.

## Architecture

This is a Wayland compositor built with Smithay that arranges windows in a scrollable vertical column.

### Crate Structure

- **compositor**: Smithay-based Wayland compositor with winit backend
  - `main.rs`: Event loop, rendering, frame handling
  - `state.rs`: `ColumnCompositor` state machine, `ColumnCell` enum (Terminal/External)
  - `input.rs`: Keyboard/pointer event handling, coordinate conversion
  - `coords.rs`: Type-safe coordinate wrappers (ScreenY, RenderY, ContentY)
  - `layout.rs`: Pure function layout calculation
  - `terminal_manager.rs`: Manages multiple terminal instances

- **terminal**: alacritty_terminal wrapper with explicit sizing state machine
  - `sizing.rs`: `TerminalSizingState` (Stable/GrowthRequested/Resizing)
  - `render.rs`: Software renderer using fontdue
  - `pty.rs`: PTY management with rustix

- **test-harness**: Testing infrastructure
  - `headless.rs`: `TestCompositor` mock for unit testing
  - `assertions.rs`: Test assertion helpers
  - Tests in `tests/` directory

### Coordinate Systems (Critical!)

The compositor uses three coordinate systems that must not be mixed:

1. **Screen coords**: Y=0 at TOP (from Winit input events)
2. **Render coords**: Y=0 at BOTTOM (OpenGL/Smithay convention)
3. **Content coords**: Absolute position in scrollable content

The Y-flip formula: `render_y = screen_height - screen_y`

Cell positioning with Y-flip: `render_y = screen_height - content_y - cell_height`

### Cells Model

Windows and terminals are unified in a single `cells: Vec<ColumnCell>` list:
- `ColumnCell::Terminal(TerminalId)` - internal terminal
- `ColumnCell::External(WindowEntry)` - Wayland client window

Cell index 0 appears at TOP of screen (highest render Y).

### Height Consistency

**Important**: Click detection and rendering must use identical heights. Heights are cached from the previous frame's actual rendered heights (element geometry), NOT from `bbox()` which can differ.

### Terminal Grid vs PTY Size

The terminal has two row counts that must not be confused:
- **Grid rows** (`grid_rows()`): Always 1000, internal alacritty storage for content
- **PTY rows** (`dimensions()`): Actual size reported to programs, changes on resize

The grid stays large to hold all content without scrolling. Only PTY size changes when the terminal is resized. TUI apps query PTY size via `tcgetwinsize`.

### Key Bindings

- Super+T or Ctrl+Shift+T: Spawn terminal
- Super+J/K or Ctrl+Shift+J/K: Focus next/prev
- Super+Q or Ctrl+Shift+Q: Quit
- Page Up/Down: Scroll

## Testing Patterns

The test harness uses a mock `TestCompositor` that simulates the real compositor's coordinate calculations. Tests should:

1. Use `simulate_click(screen_x, screen_y)` with screen coordinates
2. Use `render_positions()` to get where windows actually render
3. Verify click detection matches render positions
