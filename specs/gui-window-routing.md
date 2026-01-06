# GUI Window Routing Specification

This document specifies how termstack routes GUI application windows between the host desktop and the termstack compositor.

## Core Principle

Every command spawns a terminal cell in termstack for its output. The routing decision only affects where GUI windows appear:

- **Host desktop**: Windows appear on the user's normal desktop (outside termstack)
- **Termstack**: Windows appear inside termstack's column layout

## Routing Rules

### Default Behavior (No `gui` Prefix)

Commands run without the `gui` prefix have their GUI windows routed to the **host desktop**.

```bash
# These open windows on the HOST desktop
mupdf document.pdf
firefox
gimp image.png
```

Environment for spawned terminals:
- `WAYLAND_DISPLAY` = host's Wayland display (from `HOST_WAYLAND_DISPLAY`)
- `DISPLAY` = host's X11 display (from `HOST_DISPLAY`)

### With `gui` Prefix

Commands run with the `gui` prefix have their GUI windows routed **inside termstack**.

```bash
# These open windows INSIDE termstack
gui mupdf document.pdf
gui firefox
gui gimp image.png
```

Environment for spawned terminals:
- `WAYLAND_DISPLAY` = compositor's Wayland socket (e.g., `wayland-1`)
- `DISPLAY` = compositor's X11 display via xwayland-satellite (e.g., `:2`)

### Foreground vs Background Mode

The `gui` command supports two modes:

**Foreground mode** (default):
- The launching terminal is hidden while the GUI app runs
- When the GUI app exits, the launching terminal reappears

```bash
gui mupdf document.pdf  # Foreground mode
```

**Background mode**:
- The launching terminal stays visible and usable
- User can continue working while the GUI app runs

```bash
gui -b mupdf document.pdf  # Background mode
gui --background firefox   # Background mode
```

## Script Compatibility

The `gui` command must work from any context:

1. **Direct invocation**: `gui mupdf file.pdf`
2. **Via sh**: `sh -c 'gui mupdf file.pdf'`
3. **From scripts**: Python, Ruby, Bash scripts calling `gui`
4. **Nested shells**: Fish calling bash calling `gui`

This is achieved by:
- Keeping `TERMSTACK_SOCKET` in spawned terminal environments
- The `gui` shell script in PATH (extracted at runtime)
- IPC communication to compositor for actual spawning

## Environment Variables

### Saved at Compositor Startup

| Variable | Purpose |
|----------|---------|
| `HOST_WAYLAND_DISPLAY` | Original Wayland display (host) |
| `HOST_DISPLAY` | Original X11 display (host) |

### Set by Compositor

| Variable | Value | Purpose |
|----------|-------|---------|
| `WAYLAND_DISPLAY` | `wayland-1` | Compositor's Wayland socket |
| `DISPLAY` | `:N` | XWayland display number |
| `TERMSTACK_SOCKET` | Socket path | IPC for CLI communication |

### Terminal Spawn Environment

**Regular spawns** (windows go to host):
```
WAYLAND_DISPLAY = $HOST_WAYLAND_DISPLAY
DISPLAY = $HOST_DISPLAY
TERMSTACK_SOCKET = (preserved for nested gui calls)
```

**GUI spawns** (windows go to termstack):
```
WAYLAND_DISPLAY = compositor's socket
DISPLAY = compositor's XWayland
TERMSTACK_SOCKET = (preserved)
```

## Implementation Files

- `crates/compositor/src/spawn_handler.rs`: Environment setup for spawns
- `crates/termstack/src/cli.rs`: CLI parsing and IPC message sending
- `scripts/bin/gui`: Shell wrapper for `termstack gui`
- `crates/compositor/src/xwayland_lifecycle.rs`: XWayland/xwayland-satellite setup

## Test Cases

1. `mupdf file.pdf` - window on host, terminal cell in termstack
2. `gui mupdf file.pdf` - window in termstack, terminal cell in termstack
3. `sh -c 'gui mupdf file.pdf'` - window in termstack (script compatibility)
4. `gui -b mupdf file.pdf` - window in termstack, launcher terminal visible
5. Wayland apps (swayimg) and X11 apps (mupdf) both work correctly
