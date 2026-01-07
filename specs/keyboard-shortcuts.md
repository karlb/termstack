# Keyboard Shortcuts Specification

Keyboard shortcuts for compositor-level actions.

## Window Navigation

| Shortcut | Action |
|----------|--------|
| `Ctrl+Up` | Focus previous window |
| `Ctrl+Down` | Focus next window |
| `Ctrl+Home` | Scroll to top of column |
| `Ctrl+End` | Scroll to bottom of column |

## Terminal Management

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+N` | Spawn new interactive terminal |
| `Ctrl+Shift+W` | Close focused terminal |
| `Ctrl+Shift+Q` | Quit compositor |

## Scrolling

| Shortcut | Action |
|----------|--------|
| Mouse wheel | Scroll column |
| `Shift+Mouse wheel` | Scroll within focused terminal (scrollback) |

## Text Selection

| Shortcut | Action |
|----------|--------|
| Click + drag | Select text in terminal |
| `Ctrl+Shift+C` | Copy selection to clipboard |
| `Ctrl+Shift+V` | Paste from clipboard |

## Passthrough

All other key combinations pass through to the focused window:
- Regular typing
- Application shortcuts (e.g., `Ctrl+C` for SIGINT)
- Shell shortcuts (e.g., `Ctrl+R` for history search)

## Modifier Key Behavior

- `Ctrl+Shift+` prefix: Compositor shortcuts
- `Ctrl+` prefix: Navigation shortcuts or passthrough
- No modifier: Passthrough to focused window

## Test Cases

1. `Ctrl+Shift+N` with terminal focused - spawns new terminal
2. `Ctrl+C` in terminal - sends SIGINT (passthrough)
3. `Ctrl+Up` at top window - no change
4. `Shift+scroll` on terminal - scrolls terminal scrollback
5. Regular scroll - scrolls column, not terminal scrollback
