# Keyboard Shortcuts Specification

Keyboard shortcuts for compositor-level actions.

## Window Navigation

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+J` or `Ctrl+Shift+Down` | Focus next window |
| `Ctrl+Shift+K` or `Ctrl+Shift+Up` | Focus previous window |
| `Super+Down` | Scroll down |
| `Super+Up` | Scroll up |
| `Super+Home` | Scroll to top of column |
| `Super+End` | Scroll to bottom of column |

## Terminal Management

| Shortcut | Action |
|----------|--------|
| `Ctrl+Shift+T` or `Ctrl+Shift+Return` | Spawn new interactive terminal |
| `Ctrl+Shift+Q` or `Super+Q` | Quit compositor |

Note: Windows can only be closed by clicking the X button in their title bar, not via keyboard shortcut.

## Scrolling & Paging

| Shortcut | Action |
|----------|--------|
| Mouse wheel | Scroll column up/down |
| `Shift+Mouse wheel` | Scroll within focused terminal (scrollback) |
| `PageUp` / `PageDown` | Page up/down through column |
| `Ctrl+Shift+PageUp` / `Ctrl+Shift+PageDown` | Page up/down (alternative) |

## Text Selection & Clipboard

| Shortcut | Action |
|----------|--------|
| Click + drag | Select text (works across terminals and title bars) |
| `Ctrl+Shift+C` | Copy selection to clipboard |
| `Ctrl+Shift+V` | Paste from clipboard |
| Select text | Auto-copy to primary selection (X11-style) |
| Middle-click | Paste from primary selection |

## Passthrough

All other key combinations pass through to the focused window:
- Regular typing
- Application shortcuts (e.g., `Ctrl+C` for SIGINT)
- Shell shortcuts (e.g., `Ctrl+R` for history search)

## Test Cases

1. `Ctrl+Shift+T` with terminal focused - spawns new terminal
2. `Ctrl+C` in terminal - sends SIGINT (passthrough)
3. `Ctrl+Shift+Up` at top window - no change
4. `Shift+scroll` on terminal - scrolls terminal scrollback
5. Regular scroll - scrolls column, not terminal scrollback
6. Middle-click in terminal - pastes primary selection
