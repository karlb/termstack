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
| Super+T | Spawn new terminal |
| Super+J | Focus next window |
| Super+K | Focus previous window |
| Super+Down | Scroll down |
| Super+Up | Scroll up |
| Super+Home | Scroll to top |
| Super+End | Scroll to bottom |
| Page Up | Scroll up one page |
| Page Down | Scroll down one page |

## Shell Integration

For the full column-compositor experience, add shell integration so commands automatically spawn in new terminals while TUI apps run in the current terminal at full height.

### Installation

First, ensure `column-term` is in your PATH:

```bash
cargo install --path crates/column-term
# Or copy target/release/column-term to ~/.local/bin/
```

### Zsh

Add to `~/.zshrc`:

```zsh
# Column-compositor shell integration
if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
    column-exec() {
        local cmd="$BUFFER"
        [[ -z "$cmd" ]] && return
        BUFFER=""
        column-term -c "$cmd"
        local ret=$?
        if [[ $ret -eq 2 ]]; then
            # Shell builtin - run in current shell
            eval "$cmd"
        elif [[ $ret -eq 3 ]]; then
            # TUI app - resize to full height, run, resize back
            column-term --resize full
            eval "$cmd"
            column-term --resize content
        fi
        zle reset-prompt
    }
    zle -N accept-line column-exec
fi
```

### Bash

Add to `~/.bashrc`:

```bash
# Column-compositor shell integration
if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
    column_prompt_command() {
        # Reset to content size after each command (in case a TUI app ran)
        column-term --resize content 2>/dev/null
    }
    PROMPT_COMMAND="column_prompt_command${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
```

Note: Full bash integration requires a custom readline wrapper. The above provides basic TUI resize support.

### Fish

Add to `~/.config/fish/config.fish`:

```fish
# Column-compositor shell integration
if set -q COLUMN_COMPOSITOR_SOCKET
    function column_postexec --on-event fish_postexec
        column-term --resize content 2>/dev/null
    end
end
```

### Configuration

Create `~/.config/column-compositor/config.toml` to configure TUI apps:

```toml
# Apps that should run in current terminal at full height
tui_apps = ["vim", "nvim", "mc", "htop", "top", "fzf", "less", "man", "nano"]
```

### How It Works

- **Regular commands** (`ls`, `git status`): Spawn in a new terminal above current
- **Shell builtins** (`cd`, `export`): Run in current shell
- **TUI apps** (`vim`, `mc`, `fzf`): Resize terminal to full height, run, resize back
- **GUI apps**: Get an output terminal that appears when stderr is produced

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
