# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Installation

```bash
# Install dependencies (including xwayland-satellite for X11 support)
./install-deps.sh

# Or manually install from GitHub:
cargo install --git https://github.com/Supreeeme/xwayland-satellite.git xwayland-satellite
```

## Development Commands

```bash
cargo b                              # Build (alias for: cargo build -Zchecksum-freshness)
cargo t                              # Test (alias for: cargo nextest run -Zchecksum-freshness)
cargo l                              # Lint (alias for: cargo clippy -Zchecksum-freshness)
cargo r                              # Run compositor (alias for: cargo run -Zchecksum-freshness --bin termstack)
RUST_LOG=termstack_compositor=debug cargo r    # Run with debug logging
```

**Note:** The `termstack` binary uses smart mode detection. When run directly (`cargo r`), it starts the compositor (no TERMSTACK_SOCKET set). When run inside a termstack session, it acts as the CLI tool.

Configuration is in `.cargo/config.toml` and `rust-toolchain.toml`. The nightly toolchain and `-Zchecksum-freshness` flag (to avoid mtime race conditions) are applied automatically.

## Development approach

- Before fixing bugs, reproduce them inside the test suite.
- Prefer improving the test suite to writing one-off tests.
- When done, check for linting errors

## Architecture

This is a Wayland compositor built with Smithay that arranges windows in a scrollable vertical column.

### Crate Structure

- **compositor**: Smithay-based Wayland compositor library
  - `main.rs`: Compositor entry point (`run_compositor()`, `setup_logging()`)
  - `lib.rs`: Public library API, re-exports main functions
  - `state.rs`: `TermStack` state machine, `StackWindow` enum (Terminal/External)
  - `input.rs`: Keyboard/pointer event handling, coordinate conversion
  - `coords.rs`: Type-safe coordinate wrappers (ScreenY, RenderY, ContentY)
  - `layout.rs`: Pure function layout calculation
  - `render.rs`: Rendering logic and damage tracking
  - `terminal_manager/`: Manages multiple terminal instances
  - `cursor.rs`: Cursor rendering and management
  - `title_bar.rs`: Title bar rendering using fontdue
  - `ipc.rs`: IPC protocol for termstack CLI communication
  - `config.rs`: Configuration file handling

- **termstack**: Unified binary with smart mode detection
  - `main.rs`: Smart detection entry point (compositor or CLI mode)
  - `cli.rs`: CLI tool for spawning terminals
  - Shell integration for automatic command routing
  - IPC client for communicating with compositor
  - TUI app detection and automatic resizing
  - GUI app output terminal spawning

- **terminal**: alacritty_terminal wrapper with explicit sizing state machine
  - `state.rs`: Terminal state and alacritty integration
  - `sizing.rs`: `TerminalSizingState` (Stable/GrowthRequested/Resizing)
  - `render.rs`: Software renderer using fontdue
  - `pty.rs`: PTY management with rustix

- **test-harness**: Testing infrastructure
  - `headless.rs`: `TestCompositor` mock for unit testing
  - `assertions.rs`: Test assertion helpers
  - `fixtures.rs`: Test fixtures and data
  - `live.rs`: Live testing utilities
  - Tests in `tests/` directory

### Coordinate Systems (Critical!)

The compositor uses three coordinate systems that must not be mixed:

1. **Screen coords**: Y=0 at TOP (from Winit input events)
2. **Render coords**: Y=0 at BOTTOM (OpenGL/Smithay convention)
3. **Content coords**: Absolute position in scrollable content

The Y-flip formula: `render_y = screen_height - screen_y`

Window positioning with Y-flip: `render_y = screen_height - content_y - window_height`

### Windows Model

Windows and terminals are unified in a single `layout_nodes: Vec<LayoutNode>` list:
- `StackWindow::Terminal(TerminalId)` - internal terminal
- `StackWindow::External(WindowEntry)` - Wayland client window

Window index 0 appears at TOP of screen (highest render Y).

### Height Consistency

**Important**: Click detection and rendering must use identical heights. Heights are cached from the previous frame's actual rendered heights (element geometry), NOT from `bbox()` which can differ.

### Terminal Grid vs PTY Size

The terminal has two row counts that must not be confused:
- **Grid rows** (`grid_rows()`): Always 1000, internal alacritty storage for content
- **PTY rows** (`dimensions()`): Actual size reported to programs, changes on resize

The grid stays large to hold all content without scrolling. Only PTY size changes when the terminal is resized. TUI apps query PTY size via `tcgetwinsize`.

## XWayland Integration (via xwayland-satellite)

X11 application support uses [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite), which acts as an X11 window manager and presents X11 windows as normal Wayland `ToplevelSurface` windows.

**Requirements:**
- xwayland-satellite >= 0.7 (optional soft dependency)
- Install: `cargo install xwayland-satellite` or via package manager

**Architecture:**
```
X11 App <-> XWayland <-> xwayland-satellite <-> Compositor
                         (acts as WM + Wayland client)
```

X11 windows appear as regular `ToplevelSurface` windows - no special handling needed. The compositor automatically spawns xwayland-satellite when XWayland becomes ready. If xwayland-satellite crashes, it will auto-restart up to 3 times (with 10-second backoff window) before giving up for the session.

If xwayland-satellite is not installed, the compositor continues in Wayland-only mode with a warning.

**Implementation:** See `crates/compositor/src/main.rs:initialize_xwayland()` for lifecycle management and auto-restart logic.

**Testing:**
```bash
# Integration tests (requires xwayland-satellite installed)
cargo test -p test-harness --features gui-tests --test x11_integration

# Manual testing with X11 apps
DISPLAY=:2 xeyes &
DISPLAY=:2 xclock &
```

## Testing Patterns

The test harness uses a mock `TestCompositor` that simulates the real compositor's coordinate calculations. Tests should:

1. Use `simulate_click(screen_x, screen_y)` with screen coordinates
2. Use `render_positions()` to get where windows actually render
3. Verify click detection matches render positions

## Additional Documentation

- **[Glossary](docs/glossary.md)**: Terminology used throughout TermStack (stack, window, launcher terminal, etc.)
- **[X11/XWayland Integration](docs/x11-integration.md)**: Historical documentation of Smithay X11Wm integration (pre-xwayland-satellite migration). Includes detailed notes on resize issues, coordinate transforms, and protocol workarounds. Now using xwayland-satellite instead (see XWayland Integration section above).
