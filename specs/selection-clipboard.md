# Selection & Clipboard Specification

Text selection and clipboard operations in terminals.

## Selection

### Mouse Selection

- Click and drag to select text
- Selection highlights with distinct background color
- Selection spans visible terminal content

### Selection Boundaries

- Selection is constrained to single terminal
- Cannot select across multiple windows
- Selection coordinates are in terminal grid (col, row)

### Selection Persistence

- Selection persists while terminal is focused
- Selection clears when:
  - Clicking elsewhere in terminal
  - Clicking on different window
  - New output pushes selected content off screen (TBD)

## Clipboard

### Copy (`Ctrl+Shift+C`)

- Copies selected text to system clipboard
- If no selection, does nothing
- Uses primary clipboard (Wayland) or X11 clipboard

### Paste (`Ctrl+Shift+V`)

- Pastes clipboard content to focused terminal
- Content sent as keyboard input to PTY
- Works with both Wayland and X11 clipboards

### Clipboard Integration

- Uses host clipboard (not compositor-internal)
- X11 apps use X11 clipboard via xwayland-satellite
- Wayland apps use Wayland clipboard

## Coordinate Conversion

Selection uses terminal grid coordinates:
- `col`: Character column (0-indexed from left)
- `row`: Line row (0-indexed from top of visible area)

Mouse position must be converted:
1. Screen coords → Render coords (Y-flip)
2. Render coords → Terminal-relative position
3. Pixel position → Grid cell (col, row)

## Test Cases

1. Select "hello" in terminal - text highlights
2. `Ctrl+Shift+C` - text copied to clipboard
3. `Ctrl+Shift+V` - clipboard content appears at cursor
4. Select text, click elsewhere - selection clears
5. Select text, focus other window - selection clears
6. Paste from external app - works in terminal
