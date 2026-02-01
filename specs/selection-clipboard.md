# Selection & Clipboard Specification

Text selection and clipboard operations in terminals.

## Selection

### Mouse Selection

- Click and drag to select text
- Selection highlights with distinct background color
- Selection spans visible terminal content

### Selection Scope

- Selection works across multiple windows in the stack
- Can select text spanning terminals, GUI window titles, and title bars
- Selection appears as a continuous range from start to end position
- Enables copying commands and outputs as if the stack were one terminal

### Selection Persistence

- Selection persists while active
- Selection clears when:
  - Clicking elsewhere
  - Starting a new selection
  - New output pushes selected content off screen (TBD)

## Clipboard

### Copy (`Ctrl+Shift+C`)

- Copies selected text to system clipboard
- If no selection, does nothing
- Uses standard clipboard (Wayland data-device or X11 CLIPBOARD)

### Paste (`Ctrl+Shift+V`)

- Pastes clipboard content to focused terminal
- Content sent as keyboard input to PTY
- Works with both Wayland and X11 clipboards

### Clipboard Integration

- Uses host clipboard (not compositor-internal)
- X11 apps use X11 clipboard via xwayland-satellite
- Wayland apps use Wayland clipboard

## Primary Selection (X11-style)

### Auto-Copy on Selection

- Text is automatically copied to primary selection when selected
- Works alongside the standard clipboard (independent buffers)
- Enables traditional Unix select-to-copy workflow

### Middle-Click Paste

- Middle-click pastes from primary selection, not clipboard
- Works in any terminal window
- Content sent as keyboard input to PTY

## Test Cases

1. Select "hello" in terminal - text highlights
2. `Ctrl+Shift+C` - text copied to clipboard
3. `Ctrl+Shift+V` - clipboard content appears at cursor
4. Select text, click elsewhere - selection clears
5. Paste from external app - works in terminal
6. Select text - auto-copied to primary selection
7. Middle-click in terminal - pastes from primary selection
8. Select across two terminals - both terminals highlight, copy includes both
