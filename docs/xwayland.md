# XWayland Support via xwayland-satellite

## Overview

This compositor supports X11 applications through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite), which acts as an X11 window manager and presents X11 windows as standard Wayland `ToplevelSurface` windows.

### Architecture

```
X11 App <-> XWayland <-> xwayland-satellite <-> Compositor
                         (acts as X11 WM)      (sees Wayland windows)
```

**Key benefits:**
- X11 windows appear as normal Wayland windows - no special handling needed
- Eliminates complex X11 protocol workarounds
- Production-tested (used by niri and other compositors)
- Clean separation of concerns

## Installation

xwayland-satellite is an **optional dependency**. The compositor will work without it in Wayland-only mode.

### Install from crates.io
```bash
cargo install xwayland-satellite
```

### Install from source
```bash
cargo install --git https://github.com/Supreeeme/xwayland-satellite.git xwayland-satellite
```

### System packages
Check your distribution's package manager for `xwayland-satellite`.

### Verify installation
```bash
which xwayland-satellite
# Should output: /home/<user>/.cargo/bin/xwayland-satellite (or system path)
```

## Implementation Details

### Lifecycle Management

**Initialization** (`crates/compositor/src/main.rs:initialize_xwayland()`):
1. Compositor spawns XWayland without a window manager
2. Waits for `XWaylandEvent::Ready` event
3. Sets `DISPLAY` environment variable (e.g., `:1`)
4. Spawns xwayland-satellite process with the display number

**Crash Detection and Auto-Restart** (main event loop):
- Every frame, checks if xwayland-satellite is still running via `try_wait()`
- If crashed, implements smart backoff logic:
  - **Rapid crash** = within 10 seconds of previous crash
  - **Counter**: Increments on rapid crash, resets after stable runtime
  - **Max attempts**: 3 automatic restarts
  - **Give up**: After 3 rapid crashes, disables X11 for the session

**Shutdown** (compositor exit):
- Sends `SIGKILL` to xwayland-satellite
- Waits for process cleanup
- Logs termination

### Environment Variables

**DISPLAY**: Set by compositor when XWayland is ready
- Example: `DISPLAY=:1`
- Allows X11 apps to connect to the XWayland instance

**WAYLAND_DISPLAY**: Set to `wayland-1` for xwayland-satellite
- In nested setups: host compositor is `wayland-0`, our compositor is `wayland-1`
- Critical: xwayland-satellite MUST connect to OUR compositor, not the host

### Crash Tracking State

`XWaylandSatelliteMonitor` struct (crates/compositor/src/state.rs:81-89):
```rust
pub struct XWaylandSatelliteMonitor {
    pub child: std::process::Child,
    pub last_crash_time: Option<Instant>,
    pub crash_count: u32,
}
```

## Testing

### Integration Tests
```bash
# Requires xwayland-satellite installed
cargo test -p test-harness --features gui-tests --test x11_integration
```

Tests verify:
- X11 app launch via xwayland-satellite
- Multiple simultaneous X11 apps
- Compositor continues without xwayland-satellite (graceful degradation)

### Manual Testing
```bash
# Start compositor (will spawn xwayland-satellite automatically)
cargo run --bin column-compositor

# In another terminal, launch X11 apps (wait for compositor to set DISPLAY)
sleep 2
export DISPLAY=:1  # or whatever display number the compositor uses
xeyes &
xclock &
xterm &
```

## Troubleshooting

### X11 apps don't launch

**Check if xwayland-satellite is installed:**
```bash
which xwayland-satellite
```

**Check compositor logs:**
```bash
RUST_LOG=column_compositor=debug cargo run --bin column-compositor
```

Look for:
- `xwayland-satellite not found` - Install xwayland-satellite
- `xwayland-satellite crashed` - Check stderr in logs
- `XWayland ready on display :N` - Success!

### xwayland-satellite keeps crashing

**Check version compatibility:**
- Requires xwayland-satellite >= 0.7
- Update: `cargo install xwayland-satellite --force`

**Check logs for error messages:**
- Compositor captures xwayland-satellite stderr
- Look for specific error messages in `xwayland-satellite crashed` log entries

**Reset after 3 rapid crashes:**
- Compositor gives up after 3 crashes within 10 seconds
- Restart compositor to retry

### "DISPLAY not set" errors

**Wait for XWayland initialization:**
- Compositor spawns initial terminal only after XWayland is ready
- Launching X11 apps too early will fail
- Check logs for `XWayland ready on display :N` message

## Why xwayland-satellite Instead of Smithay X11Wm?

Smithay's built-in X11Wm integration had fundamental limitations for tiling compositor use cases:
- Incomplete protocol implementation (resize, configure events)
- Complex coordinate transform requirements
- Difficult-to-debug rendering issues

xwayland-satellite solves these by handling all X11 protocol complexity and presenting a clean Wayland interface to the compositor.
