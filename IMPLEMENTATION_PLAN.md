# Implementation Plan: Content-Aware Terminal Compositor v2

A ground-up reimplementation using Smithay (Rust compositor library) and alacritty_terminal (terminal emulation), incorporating lessons learned from the foot fork approach.

---

## Executive Summary

**Goal**: Build a Wayland compositor with content-aware, dynamically-sizing terminal windows arranged in a scrollable vertical column.

**Tech Stack**:
- Compositor: Smithay (Rust)
- Terminal emulation: alacritty_terminal crate
- Rendering: softbuffer or wgpu (simpler than OpenGL)
- Testing: Rust's built-in test framework + custom integration harness

**Key Architectural Changes from v1**:
1. Single language (Rust) eliminates FFI complexity
2. Type-safe state machine prevents content-counting bugs
3. alacritty_terminal provides content tracking out of the box
4. Async/await for clean Wayland protocol handling
5. Property-based testing for invariant verification

---

## Phase 1: Foundation (Compositor Shell)

### 1.1 Project Setup

```
column-compositor/
├── Cargo.toml              # Workspace root
├── crates/
│   ├── compositor/         # Smithay-based compositor
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── state.rs    # Compositor state machine
│   │   │   ├── layout.rs   # Column layout algorithm
│   │   │   ├── input.rs    # Keyboard/scroll handling
│   │   │   └── config.rs   # Runtime configuration
│   │   └── Cargo.toml
│   ├── terminal/           # Terminal window implementation
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── pty.rs      # PTY management
│   │   │   ├── render.rs   # Grid to buffer rendering
│   │   │   ├── sizing.rs   # Dynamic size calculation
│   │   │   └── state.rs    # Terminal state machine
│   │   └── Cargo.toml
│   └── test-harness/       # Integration test infrastructure
│       ├── src/
│       │   ├── lib.rs
│       │   ├── headless.rs # Headless compositor wrapper
│       │   ├── assertions.rs
│       │   └── fixtures.rs
│       └── Cargo.toml
├── tests/
│   ├── integration/
│   └── properties/
└── README.md
```

**Dependencies** (compositor/Cargo.toml):
```toml
[dependencies]
smithay = { version = "0.3", default-features = false, features = [
    "backend_drm",
    "backend_libinput",
    "backend_udev",
    "backend_winit",      # For development/testing
    "backend_headless",   # For CI
    "renderer_glow",
    "xwayland",
    "wayland_frontend",
] }
tracing = "0.1"
tracing-subscriber = "0.3"

[dev-dependencies]
proptest = "1.0"          # Property-based testing
```

### 1.2 Minimal Compositor (anvil-derived)

Start from Smithay's anvil example, strip to essentials:

```rust
// compositor/src/state.rs

use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::CompositorState;
use smithay::wayland::shell::xdg::XdgShellState;

pub struct ColumnCompositor {
    // Wayland state
    pub display: Display<Self>,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,

    // Our state
    pub windows: Vec<WindowEntry>,
    pub scroll_offset: f64,
    pub focused_index: Option<usize>,

    // Layout cache (recalculated on change)
    pub layout: ColumnLayout,
}

pub struct WindowEntry {
    pub toplevel: ToplevelSurface,
    pub state: WindowState,
}

/// Explicit state machine - LEARNING: implicit state caused bugs
#[derive(Debug, Clone)]
pub enum WindowState {
    /// Window is stable, accepting input
    Active { height: u32 },

    /// Resize requested, waiting for client ack
    PendingResize {
        current_height: u32,
        requested_height: u32,
        request_serial: u32,
    },

    /// Client acked, waiting for
    /// commit with new size
    AwaitingCommit {
        current_height: u32,
        target_height: u32,
    },
}
```

**Key Lesson Applied**: Window state is an explicit enum. No boolean flags, no implicit states. The type system enforces valid transitions.

### 1.3 Column Layout Algorithm

```rust
// compositor/src/layout.rs

pub struct ColumnLayout {
    pub window_positions: Vec<WindowPosition>,
    pub total_height: u32,
    pub visible_range: Range<u32>,
}

pub struct WindowPosition {
    pub y: i32,           // Can be negative (scrolled off top)
    pub height: u32,
    pub visible: bool,    // Any part on screen?
}

impl ColumnLayout {
    /// Pure function: inputs -> layout
    /// LEARNING: Keep layout calculation pure, no side effects
    pub fn calculate(
        windows: &[WindowEntry],
        output_height: u32,
        scroll_offset: f64,
    ) -> Self {
        let mut y_accumulator: i32 = 0;
        let mut positions = Vec::with_capacity(windows.len());

        for window in windows {
            let height = window.state.current_height();
            let y = y_accumulator - scroll_offset as i32;

            positions.push(WindowPosition {
                y,
                height,
                visible: y < output_height as i32 && y + height as i32 > 0,
            });

            y_accumulator += height as i32;
        }

        Self {
            window_positions: positions,
            total_height: y_accumulator as u32,
            visible_range: scroll_offset as u32..scroll_offset as u32 + output_height,
        }
    }

    /// LEARNING: Auto-scroll logic extracted and testable
    pub fn scroll_to_show_bottom(
        &self,
        window_index: usize,
        output_height: u32,
    ) -> Option<f64> {
        let pos = &self.window_positions[window_index];
        let window_bottom = pos.y + pos.height as i32;

        if window_bottom > output_height as i32 {
            // Window extends below viewport - scroll to show bottom
            Some((window_bottom - output_height as i32) as f64)
        } else {
            None
        }
    }
}
```

### 1.4 Deliverables - Phase 1

- [ ] Compositor starts and creates Wayland socket
- [ ] Can run in headless mode (WLR_BACKENDS equivalent)
- [ ] Accepts XDG shell connections
- [ ] Windows stack vertically (no overlap)
- [ ] Keyboard scrolling works
- [ ] Basic logging with tracing

**Test**: Launch `weston-terminal` or `foot`, verify it appears and stacks correctly.

---

## Phase 2: Terminal Integration

### 2.1 Terminal State Machine

```rust
// terminal/src/state.rs

use alacritty_terminal::Term;
use alacritty_terminal::event::Event as TermEvent;

/// LEARNING: Explicit state prevents double-counting bugs
#[derive(Debug)]
pub enum TerminalSizingState {
    /// Terminal is stable at current size
    Stable {
        rows: u16,
        content_rows: u32,  // Total lines produced (visible + scrollback)
    },

    /// Growth requested, waiting for compositor configure
    GrowthRequested {
        current_rows: u16,
        target_rows: u16,
        content_rows: u32,
        /// Lines that scrolled off while waiting
        /// LEARNING: Track this explicitly, restore later
        pending_scrollback: u32,
    },

    /// Configure received, applying new size
    Resizing {
        from_rows: u16,
        to_rows: u16,
        content_rows: u32,
        pending_scrollback: u32,
    },
}

impl TerminalSizingState {
    /// LEARNING: Single point of truth for content counting
    /// Only increment in one place, in one state
    pub fn on_new_line(&mut self) -> SizingAction {
        match self {
            Self::Stable { content_rows, rows } => {
                *content_rows += 1;

                if *content_rows > *rows as u32 {
                    SizingAction::RequestGrowth {
                        target_rows: *content_rows as u16,
                    }
                } else {
                    SizingAction::None
                }
            }

            Self::GrowthRequested { pending_scrollback, .. } => {
                // LEARNING: Don't increment content_rows here!
                // Just track that a line scrolled off
                *pending_scrollback += 1;
                SizingAction::None
            }

            Self::Resizing { pending_scrollback, .. } => {
                *pending_scrollback += 1;
                SizingAction::None
            }
        }
    }

    pub fn on_configure(&mut self, new_rows: u16) -> SizingAction {
        match self {
            Self::GrowthRequested {
                current_rows,
                content_rows,
                pending_scrollback,
                ..
            } => {
                let scrollback = *pending_scrollback;
                *self = Self::Resizing {
                    from_rows: *current_rows,
                    to_rows: new_rows,
                    content_rows: *content_rows,
                    pending_scrollback: scrollback,
                };
                SizingAction::ApplyResize { rows: new_rows }
            }
            _ => SizingAction::None,
        }
    }

    pub fn on_resize_complete(&mut self) -> SizingAction {
        match self {
            Self::Resizing {
                to_rows,
                content_rows,
                pending_scrollback,
                ..
            } => {
                let restore = *pending_scrollback;
                *self = Self::Stable {
                    rows: *to_rows,
                    content_rows: *content_rows,
                };

                if restore > 0 {
                    SizingAction::RestoreScrollback { lines: restore }
                } else {
                    SizingAction::None
                }
            }
            _ => SizingAction::None,
        }
    }
}

pub enum SizingAction {
    None,
    RequestGrowth { target_rows: u16 },
    ApplyResize { rows: u16 },
    RestoreScrollback { lines: u32 },
}
```

### 2.2 PTY Integration

```rust
// terminal/src/pty.rs

use std::os::unix::io::{AsRawFd, RawFd};
use nix::pty::{openpty, Winsize};
use alacritty_terminal::tty::{self, Pty};

pub struct PtyManager {
    pty: Pty,
    event_loop: mio::Poll,
}

impl PtyManager {
    pub fn spawn(shell: &str, initial_size: Winsize) -> Result<Self> {
        let pty = tty::new(&tty::Options {
            shell: Some(shell.into()),
            ..Default::default()
        }, initial_size, 0)?;

        // Register for read events
        let event_loop = mio::Poll::new()?;
        event_loop.registry().register(
            &mut SourceFd(&pty.file().as_raw_fd()),
            Token(0),
            Interest::READABLE,
        )?;

        Ok(Self { pty, event_loop })
    }

    pub fn resize(&mut self, size: Winsize) -> Result<()> {
        self.pty.resize(size)
    }

    pub fn read_available(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.pty.reader().read(buf)
    }
}
```

### 2.3 Rendering Grid to Buffer

```rust
// terminal/src/render.rs

use alacritty_terminal::Term;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::vte::ansi::Color;

pub struct TerminalRenderer {
    font: Font,  // fontdue or similar
    cell_width: u32,
    cell_height: u32,
    buffer: Vec<u32>,  // ARGB pixel buffer
}

impl TerminalRenderer {
    pub fn render(&mut self, term: &Term<impl EventListener>, width: u32, height: u32) {
        self.buffer.resize((width * height) as usize, 0xFF000000);

        let content = term.renderable_content();

        for cell in content.display_iter {
            let x = cell.point.column.0 as u32 * self.cell_width;
            let y = cell.point.line.0 as u32 * self.cell_height;

            self.render_cell(x, y, &cell.cell);
        }

        // Render cursor
        if let Some(cursor) = content.cursor {
            self.render_cursor(cursor);
        }
    }

    fn render_cell(&mut self, x: u32, y: u32, cell: &Cell) {
        // Background
        let bg = self.color_to_argb(cell.bg);
        self.fill_rect(x, y, self.cell_width, self.cell_height, bg);

        // Glyph
        if cell.c != ' ' {
            let glyph = self.font.rasterize(cell.c, self.cell_height as f32);
            let fg = self.color_to_argb(cell.fg);
            self.draw_glyph(x, y, &glyph, fg);
        }
    }

    pub fn buffer(&self) -> &[u32] {
        &self.buffer
    }
}
```

### 2.4 Deliverables - Phase 2

- [ ] Terminal window spawns shell
- [ ] Characters display correctly
- [ ] Keyboard input works
- [ ] Terminal grows as content is added
- [ ] Scrollback preserved during growth
- [ ] No double-counting (verified by state machine)

**Test**: Run `for i in $(seq 1 100); do echo "line $i"; done` - terminal should grow smoothly, no empty rows.

---

## Phase 3: Testing Infrastructure

### 3.1 Test Harness Design

```rust
// test-harness/src/lib.rs

pub struct TestCompositor {
    compositor: ColumnCompositor,
    output_size: (u32, u32),
    events: Vec<TestEvent>,
}

impl TestCompositor {
    /// Start compositor in headless mode
    pub fn new_headless(width: u32, height: u32) -> Self {
        std::env::set_var("SMITHAY_BACKEND", "headless");
        // ...
    }

    /// Spawn a terminal and return handle
    pub fn spawn_terminal(&mut self) -> TerminalHandle {
        // ...
    }

    /// Send input to terminal
    pub fn send_input(&mut self, handle: &TerminalHandle, input: &str) {
        // ...
    }

    /// Wait for condition with timeout
    /// LEARNING: Event-based, not sleep-based
    pub fn wait_for<F>(&mut self, condition: F, timeout: Duration) -> Result<()>
    where
        F: Fn(&Self) -> bool,
    {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            self.dispatch_events(Duration::from_millis(10))?;
            if condition(self) { return Ok(()); }
        }

        Err(TestError::Timeout)
    }

    /// Get current state snapshot for assertions
    pub fn snapshot(&self) -> CompositorSnapshot {
        CompositorSnapshot {
            window_count: self.compositor.windows.len(),
            window_heights: self.compositor.windows.iter()
                .map(|w| w.state.current_height())
                .collect(),
            scroll_offset: self.compositor.scroll_offset,
            total_height: self.compositor.layout.total_height,
            focused_index: self.compositor.focused_index,
        }
    }
}
```

### 3.2 Property-Based Tests

```rust
// tests/properties/layout_properties.rs

use proptest::prelude::*;

proptest! {
    /// LEARNING: Test invariants, not specific values
    #[test]
    fn windows_never_overlap(
        heights in prop::collection::vec(1u32..500, 1..10),
        scroll in 0f64..1000.0,
    ) {
        let windows = heights.iter().map(|&h| mock_window(h)).collect();
        let layout = ColumnLayout::calculate(&windows, 720, scroll);

        for i in 1..layout.window_positions.len() {
            let prev = &layout.window_positions[i - 1];
            let curr = &layout.window_positions[i];

            // Previous window's bottom <= current window's top
            prop_assert!(prev.y + prev.height as i32 <= curr.y);
        }
    }

    #[test]
    fn total_height_is_sum_of_windows(
        heights in prop::collection::vec(1u32..500, 1..10),
    ) {
        let windows = heights.iter().map(|&h| mock_window(h)).collect();
        let layout = ColumnLayout::calculate(&windows, 720, 0.0);

        let sum: u32 = heights.iter().sum();
        prop_assert_eq!(layout.total_height, sum);
    }

    #[test]
    fn scroll_offset_clamps_to_valid_range(
        heights in prop::collection::vec(100u32..200, 1..5),
        scroll in -1000f64..2000.0,
    ) {
        let windows = heights.iter().map(|&h| mock_window(h)).collect();
        let output_height = 720u32;
        let layout = ColumnLayout::calculate(&windows, output_height, scroll);

        // First window top should never be below viewport top
        if !layout.window_positions.is_empty() {
            prop_assert!(layout.window_positions[0].y <= 0);
        }

        // Last window bottom should never be above viewport bottom
        // (unless total content fits in viewport)
        if layout.total_height > output_height {
            let last = layout.window_positions.last().unwrap();
            prop_assert!(last.y + last.height as i32 >= output_height as i32);
        }
    }
}
```

### 3.3 State Machine Tests

```rust
// tests/properties/terminal_state.rs

proptest! {
    /// LEARNING: Content rows only increments in Stable state
    #[test]
    fn content_rows_monotonic_in_stable(
        num_lines in 1usize..100,
    ) {
        let mut state = TerminalSizingState::Stable {
            rows: 24,
            content_rows: 0
        };

        let mut last_content_rows = 0u32;

        for _ in 0..num_lines {
            state.on_new_line();

            if let TerminalSizingState::Stable { content_rows, .. } = &state {
                prop_assert!(*content_rows >= last_content_rows);
                prop_assert!(*content_rows <= last_content_rows + 1);
                last_content_rows = *content_rows;
            }
        }
    }

    /// LEARNING: No double-counting during resize
    #[test]
    fn no_double_counting_during_resize(
        lines_before in 1usize..50,
        lines_during in 1usize..20,
    ) {
        let mut state = TerminalSizingState::Stable {
            rows: 10,
            content_rows: 0
        };

        // Generate lines until growth requested
        for _ in 0..lines_before {
            state.on_new_line();
        }

        let content_at_request = match &state {
            TerminalSizingState::Stable { content_rows, .. } => *content_rows,
            TerminalSizingState::GrowthRequested { content_rows, .. } => *content_rows,
            _ => panic!("unexpected state"),
        };

        // Trigger growth request if not already
        if matches!(state, TerminalSizingState::Stable { .. }) {
            // Force into growth state for test
            state = TerminalSizingState::GrowthRequested {
                current_rows: 10,
                target_rows: 20,
                content_rows: content_at_request,
                pending_scrollback: 0,
            };
        }

        // Lines during resize go to pending_scrollback, NOT content_rows
        for _ in 0..lines_during {
            state.on_new_line();
        }

        // Configure arrives
        state.on_configure(20);

        // Complete resize
        let action = state.on_resize_complete();

        // Verify content_rows didn't change during resize
        if let TerminalSizingState::Stable { content_rows, .. } = &state {
            prop_assert_eq!(*content_rows, content_at_request);
        }

        // Verify scrollback was tracked
        if let SizingAction::RestoreScrollback { lines } = action {
            prop_assert_eq!(lines, lines_during as u32);
        }
    }
}
```

### 3.4 Integration Tests

```rust
// tests/integration/window_growth.rs

#[test]
fn terminal_grows_with_content() {
    let mut tc = TestCompositor::new_headless(1280, 720);
    let term = tc.spawn_terminal();

    // Wait for initial size
    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .expect("terminal should appear");

    let initial_height = tc.snapshot().window_heights[0];

    // Generate content
    tc.send_input(&term, "for i in $(seq 1 50); do echo \"line $i\"; done\n");

    // LEARNING: Wait for specific condition, not sleep
    tc.wait_for(
        |c| c.snapshot().window_heights[0] > initial_height + 500,
        Duration::from_secs(5),
    ).expect("terminal should grow");

    let final_snapshot = tc.snapshot();

    // Verify: no empty rows (the v1 bug)
    let term_content = tc.get_terminal_content(&term);
    let visible_lines = term_content.lines().filter(|l| !l.is_empty()).count();
    let expected_lines = 50 + 2; // 50 echoes + prompt lines

    assert!(
        visible_lines >= expected_lines - 2,
        "should have ~{} visible lines, got {}",
        expected_lines, visible_lines
    );
}

#[test]
fn auto_scroll_on_growth() {
    let mut tc = TestCompositor::new_headless(1280, 720);
    let term = tc.spawn_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // Fill screen and trigger scroll
    tc.send_input(&term, "for i in $(seq 1 100); do echo \"line $i\"; done\n");

    tc.wait_for(
        |c| c.snapshot().scroll_offset > 0.0,
        Duration::from_secs(5),
    ).expect("should auto-scroll");

    // Verify bottom of terminal is visible
    let snapshot = tc.snapshot();
    let term_bottom = snapshot.window_heights[0] as f64;
    let viewport_bottom = snapshot.scroll_offset + 720.0;

    assert!(
        (term_bottom - viewport_bottom).abs() < 50.0,
        "terminal bottom ({}) should be near viewport bottom ({})",
        term_bottom, viewport_bottom
    );
}

#[test]
fn rapid_output_no_content_loss() {
    let mut tc = TestCompositor::new_headless(1280, 720);
    let term = tc.spawn_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // LEARNING: Stress test the async resize path
    // Rapid output with tiny delays (triggers resize during resize)
    tc.send_input(&term, r#"
        for i in $(seq 1 200); do
            echo "line $i: $(date +%s%N)"
            sleep 0.001
        done
    "#);

    tc.wait_for(
        |c| c.get_terminal_content(&tc.terminals[0]).contains("line 200"),
        Duration::from_secs(30),
    ).expect("should complete output");

    // Verify all lines present
    let content = tc.get_terminal_content(&term);
    for i in 1..=200 {
        assert!(
            content.contains(&format!("line {i}:")),
            "missing line {i}"
        );
    }
}
```

### 3.5 Regression Tests

```rust
// tests/regression/mod.rs

/// Regression: empty rows after command (v1 issue-003)
#[test]
fn no_empty_rows_after_command() {
    let mut tc = TestCompositor::new_headless(1280, 720);
    let term = tc.spawn_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    tc.send_input(&term, "ls -la\n");
    tc.wait_for(
        |c| c.get_terminal_content(&tc.terminals[0]).contains("$"),
        Duration::from_secs(5),
    ).unwrap();

    // Check for empty rows at bottom
    let content = tc.get_terminal_content(&term);
    let lines: Vec<_> = content.lines().collect();

    // Find last non-empty line
    let last_content = lines.iter().rposition(|l| !l.trim().is_empty());
    let last_line = lines.len() - 1;

    // Should be at most 1 empty line (the current prompt line)
    assert!(
        last_line - last_content.unwrap_or(0) <= 1,
        "too many empty rows at bottom: {} empty lines",
        last_line - last_content.unwrap_or(0)
    );
}

/// Regression: scrollback lost during resize (v1 issue with 1ba9983)
#[test]
fn scrollback_preserved_during_growth() {
    let mut tc = TestCompositor::new_headless(1280, 720);
    let term = tc.spawn_terminal();

    tc.wait_for(|c| c.snapshot().window_count == 1, Duration::from_secs(2))
        .unwrap();

    // Output that will definitely scroll
    tc.send_input(&term, "seq 1 1000\n");

    tc.wait_for(
        |c| c.get_terminal_content(&tc.terminals[0]).contains("1000"),
        Duration::from_secs(10),
    ).unwrap();

    // Scroll up and verify early content still exists
    tc.scroll_terminal(&term, -500); // Scroll up

    let content = tc.get_terminal_content(&term);

    // Should be able to find lines from the beginning
    assert!(content.contains("1\n") || content.contains("\n1\n"));
    assert!(content.contains("50"));
    assert!(content.contains("100"));
}
```

### 3.6 Deliverables - Phase 3

- [ ] Property tests for layout invariants
- [ ] Property tests for state machine
- [ ] Integration tests with event-based waiting
- [ ] Regression tests for all v1 bugs
- [ ] CI pipeline (GitHub Actions)
- [ ] Test coverage > 80%

---

## Phase 4: Polish and Robustness

### 4.1 Error Handling

```rust
// compositor/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum CompositorError {
    #[error("Wayland backend failed: {0}")]
    Backend(#[from] smithay::backend::BackendError),

    #[error("XDG shell error: {0}")]
    XdgShell(String),

    #[error("Terminal spawn failed: {0}")]
    TerminalSpawn(#[from] std::io::Error),
}

// LEARNING: Graceful degradation, not crashes
impl ColumnCompositor {
    fn handle_toplevel_commit(&mut self, surface: &WlSurface) -> Result<(), CompositorError> {
        let Some(toplevel) = self.find_toplevel(surface) else {
            // Unknown surface - log and continue, don't crash
            tracing::warn!(?surface, "commit from unknown surface");
            return Ok(());
        };

        // ... rest of handling
    }
}
```

### 4.2 Logging and Debugging

```rust
// compositor/src/main.rs

fn setup_logging() {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();
}

// Usage throughout code:
tracing::debug!(
    window = ?toplevel.wl_surface().id(),
    old_height = %old_height,
    new_height = %new_height,
    "window resize"
);

tracing::info!(
    scroll_offset = %self.scroll_offset,
    reason = %reason,
    "auto-scroll triggered"
);
```

### 4.3 State Inspection (for debugging)

```rust
// compositor/src/debug.rs

impl ColumnCompositor {
    /// Dump state to file (for test inspection)
    /// LEARNING: Keep from v1 - this was valuable
    pub fn dump_state(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;

        let mut f = std::fs::File::create(path)?;

        writeln!(f, "# Compositor State")?;
        writeln!(f, "window_count: {}", self.windows.len())?;
        writeln!(f, "scroll_offset: {:.2}", self.scroll_offset)?;
        writeln!(f, "total_height: {}", self.layout.total_height)?;
        writeln!(f, "focused_index: {:?}", self.focused_index)?;
        writeln!(f)?;

        for (i, window) in self.windows.iter().enumerate() {
            let pos = &self.layout.window_positions[i];
            writeln!(f, "# Window {}", i)?;
            writeln!(f, "  state: {:?}", window.state)?;
            writeln!(f, "  y: {}", pos.y)?;
            writeln!(f, "  height: {}", pos.height)?;
            writeln!(f, "  visible: {}", pos.visible)?;
        }

        Ok(())
    }
}
```

### 4.4 Deliverables - Phase 4

- [ ] Comprehensive error types
- [ ] Structured logging throughout
- [ ] State dump for debugging
- [ ] Graceful handling of edge cases
- [ ] Documentation (rustdoc)

---

## Phase 5: Feature Parity with v1

### 5.1 Features to Port

| Feature | v1 Location | v2 Approach |
|---------|-------------|-------------|
| Column layout | compositor/main.c:109-186 | layout.rs (pure function) |
| Auto-scroll | compositor/main.c:515-552 | ColumnLayout::scroll_to_show_bottom |
| Focus handling | compositor/main.c:188-266 | state.rs with explicit focus enum |
| Window insertion | compositor/main.c:435-466 | windows.insert(focused_index, new) |
| Dynamic sizing | terminal.c:3326-3404 | state machine in terminal/state.rs |
| Scrollback restore | render.c:4946 | SizingAction::RestoreScrollback |

### 5.2 New Features (v2 only)

1. **Configuration file** - TOML config for colors, fonts, keybindings
2. **Multiple outputs** - Support for multi-monitor
3. **Window close** - Currently v1 doesn't handle this gracefully
4. **Shrink on clear** - Terminal can shrink when content is cleared

### 5.3 Deliverables - Phase 5

- [ ] All v1 features working
- [ ] New features implemented
- [ ] Performance comparable to v1
- [ ] Migration guide from v1

---

## Risk Mitigation

### Known Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Smithay API changes | Medium | High | Pin version, track releases |
| alacritty_terminal doesn't expose needed APIs | Low | High | Fork if needed (Apache/MIT licensed) |
| Rust learning curve | Medium | Medium | Start with anvil example, iterate |
| Performance regression | Low | Medium | Benchmark early, profile often |
| Missing Wayland protocol support | Low | Low | Smithay has good protocol coverage |

### Decision Points

1. **Font rendering**: Use fontdue (pure Rust) vs freetype bindings
   - Recommendation: Start with fontdue, switch if quality insufficient

2. **GPU rendering**: softbuffer (CPU) vs wgpu (GPU)
   - Recommendation: Start with softbuffer for simplicity, add wgpu later

3. **Config format**: TOML vs JSON vs KDL
   - Recommendation: TOML (familiar, good Rust support)

---

## Success Criteria

### Functional Requirements
- [ ] Terminal windows grow based on content
- [ ] Windows never overlap
- [ ] Auto-scroll keeps focused window bottom visible
- [ ] Scrollback preserved during resize
- [ ] No empty rows at terminal bottom

### Quality Requirements
- [ ] No content-counting bugs (state machine prevents)
- [ ] Tests pass in CI (headless mode)
- [ ] Property tests find no invariant violations
- [ ] All v1 regression tests pass

### Performance Requirements
- [ ] Terminal renders at 60fps with 10,000 line scrollback
- [ ] Resize latency < 16ms
- [ ] Memory usage < 50MB per terminal

---

## Timeline Guidance

**Note**: No time estimates provided per project guidelines. Phases are ordered by dependency, not duration.

**Suggested approach**: Complete Phase 1-2 to get a working prototype, then iterate on Phase 3-5 based on discovered issues.

**Milestones**:
1. First window appears (Phase 1)
2. Terminal shows shell prompt (Phase 2)
3. `seq 1 100` grows terminal correctly (Phase 2)
4. All property tests pass (Phase 3)
5. All v1 features working (Phase 5)
