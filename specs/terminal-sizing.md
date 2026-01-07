# Terminal Sizing Specification

Terminals in termstack have content-aware sizing that adapts to their output.

## Core Behavior

### Content-Aware Height

- Terminals start small and grow as output is produced
- Height tracks actual content rows, not a fixed size
- Maximum height is capped at viewport height
- Long output scrolls within the terminal cell

### Minimum Size

- Terminals always show at least a prompt line
- Empty terminals display at minimum height (not zero)

## TUI Detection

TUI applications (vim, mc, htop, etc.) are detected via alternate screen mode.

### When Alternate Screen Activates

- Terminal automatically resizes to full viewport height
- PTY size is updated so the app knows the available space
- Resize is synchronous (app sees correct size immediately)

### When Alternate Screen Deactivates

- Terminal returns to content-aware sizing
- Height shrinks to match actual content

## Resize Modes

### Full Mode (`termstack --resize full`)

- Expands terminal to full viewport height
- Used for TUI apps that don't trigger alternate screen
- Manual override for content-aware sizing

### Content Mode (`termstack --resize content`)

- Returns to content-aware sizing
- Shrinks to match actual content rows

## PTY vs Grid Size

- **Grid size**: Internal storage (1000 rows), holds scrollback
- **PTY size**: Reported to programs via `tcgetwinsize`
- Programs only see PTY size; grid size is internal

## Test Cases

1. `echo hello` - terminal shows 1-2 rows of content
2. `seq 1000` - terminal grows to viewport height, content scrolls
3. `vim file` - alternate screen triggers, full height immediately
4. Exit vim - returns to content-aware sizing
5. `termstack --resize full` then `--resize content` - manual toggle works
