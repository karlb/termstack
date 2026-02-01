# Window Resize Specification

GUI windows can be manually resized by dragging their edges.

## Resize Handles

- Bottom edge of each GUI window acts as a resize handle
- Cursor changes to indicate resize capability
- Drag starts on mouse button press, ends on release

## Resize Behavior

- Dragging down increases window height
- Dragging up decreases window height
- Minimum height enforced (windows cannot shrink below minimum)
- Resize applies only to GUI windows, not terminals

Note: Terminals use content-aware sizing (see [Terminal Sizing](terminal-sizing.md)) and cannot be manually resized by dragging.

## Visual Feedback

- Window resizes live during drag
- Configure requests throttled to prevent overwhelming clients
- Resize updates window geometry immediately

## Interaction with Layout

- Resizing a window affects total column height
- May cause scroll offset adjustment to keep viewport stable
- Other windows remain at their current sizes

## Test Cases

1. Drag bottom edge down - window grows
2. Drag bottom edge up - window shrinks
3. Attempt to shrink below minimum - stops at minimum height
4. Release mouse during drag - resize completes, handle releases
5. Resize window with content - content reflows to new size
