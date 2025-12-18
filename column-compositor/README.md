# Column Compositor

A Wayland compositor with content-aware, dynamically-sizing terminal windows arranged in a scrollable vertical column.

## Overview

Column Compositor is a specialized Wayland compositor designed for terminal-centric workflows. Unlike traditional tiling window managers, it arranges all windows in a single vertical column and dynamically resizes terminal windows based on their content.

### Key Features

- **Content-aware sizing**: Terminal windows grow as they produce output
- **Scrollable column**: All windows stack vertically and can be scrolled
- **Auto-scroll**: Automatically scrolls to keep the active terminal visible
- **No content loss**: Explicit state machine prevents double-counting bugs

## Architecture

```
column-compositor/
├── crates/
│   ├── compositor/     # Smithay-based Wayland compositor
│   ├── terminal/       # Terminal emulation using alacritty_terminal
│   └── test-harness/   # Testing infrastructure
└── tests/
    ├── integration/    # Integration tests
    └── properties/     # Property-based tests
```

### Tech Stack

- **Compositor**: [Smithay](https://github.com/Smithay/smithay) - Rust Wayland compositor library
- **Terminal**: [alacritty_terminal](https://github.com/alacritty/alacritty) - Terminal emulation
- **Rendering**: fontdue (font rendering) + softbuffer (pixel buffer)
- **Testing**: proptest (property-based testing)

## Building

```bash
cd column-compositor
cargo build --release
```

### Dependencies

On Debian/Ubuntu:
```bash
sudo apt install \
    libwayland-dev \
    libxkbcommon-dev \
    libudev-dev \
    libinput-dev \
    libgbm-dev \
    libdrm-dev \
    libseat-dev
```

## Running

```bash
# Start the compositor (opens in a winit window for development)
cargo run --release

# Or run on real hardware (requires seat access)
SMITHAY_BACKEND=udev cargo run --release
```

## Key Bindings

| Key | Action |
|-----|--------|
| Super+Q | Quit compositor |
| Super+J | Focus next window |
| Super+K | Focus previous window |
| Super+Down | Scroll down |
| Super+Up | Scroll up |
| Super+Home | Scroll to top |
| Super+End | Scroll to bottom |
| Page Up | Scroll up one page |
| Page Down | Scroll down one page |

## Testing

```bash
# Run all tests
cargo test

# Run property-based tests
cargo test --test properties

# Run integration tests
cargo test --test integration
```

## Design Decisions

### Explicit State Machine

The terminal sizing uses an explicit state machine to prevent the content-counting bugs from v1:

```rust
enum TerminalSizingState {
    Stable { rows, content_rows },
    GrowthRequested { current_rows, target_rows, content_rows, pending_scrollback },
    Resizing { from_rows, to_rows, content_rows, pending_scrollback },
}
```

Content rows only increment in the `Stable` state. Lines that arrive during resize are tracked in `pending_scrollback` and restored after the resize completes.

### Pure Layout Calculation

The layout algorithm is a pure function with no side effects:

```rust
impl ColumnLayout {
    pub fn calculate(windows: &[WindowEntry], output_height: u32, scroll_offset: f64) -> Self;
}
```

This makes the layout testable and predictable.

## License

MIT
