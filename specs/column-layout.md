# Column Layout Specification

Windows are arranged in a single scrollable vertical column.

## Layout Order

- Windows stack vertically, newest at bottom
- Index 0 is at the top of the column
- The column can exceed viewport height (scrollable)

## Scrolling

### Viewport Scrolling

- Mouse wheel scrolls the entire column up/down
- Scroll offset is clamped to valid range (0 to max)
- Max scroll = total content height - viewport height

### Auto-Scroll Behavior

- When new content appears at bottom: auto-scroll to show it
- Exception: Don't auto-scroll if user has scrolled up manually
- New terminal spawn: scroll to show the new terminal

### Scroll Shortcuts

- `Super+Home`: Scroll to top of column
- `Super+End`: Scroll to bottom of column

## Focus

### Focus Model

- One window is focused at a time
- Focused window receives keyboard input
- Visual indicator shows which window is focused

### Focus Navigation

- `Ctrl+Shift+K` or `Ctrl+Shift+Up`: Focus previous window (toward top)
- `Ctrl+Shift+J` or `Ctrl+Shift+Down`: Focus next window (toward bottom)
- Click on window: Focus that window

### Focus and Scroll

- Focusing a window scrolls to make it visible
- Focus follows scroll position when navigating

## Window Gaps

- Small gap between adjacent windows
- Gap provides visual separation
- Click in gap area: no window receives click

## Coordinate Systems

The compositor uses three coordinate spaces:
- **Screen coords**: Y=0 at top (input events)
- **Render coords**: Y=0 at bottom (OpenGL)
- **Content coords**: Absolute position in scrollable column

Conversion: `render_y = screen_height - screen_y`

## Test Cases

1. Three windows fit in viewport - no scrolling needed
2. Five windows exceed viewport - scrolling works
3. `Ctrl+Shift+Down` at bottom window - stays at bottom (no wrap)
4. `Ctrl+Shift+Up` at top window - stays at top (no wrap)
5. Click on partially visible window - focuses and scrolls to show it
6. New terminal spawns - scrolls to show it, focuses it
