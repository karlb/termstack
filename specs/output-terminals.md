# Output Terminals Specification

GUI applications can have associated output terminals for stderr/stdout.

## Purpose

When a GUI app produces terminal output (errors, warnings, logs), that output appears in a dedicated terminal cell below the GUI window.

## Lifecycle

### Creation

1. User runs `gui <command>`
2. Compositor creates hidden output terminal
3. Output terminal runs the command
4. GUI window appears (linked to output terminal)

### Visibility States

- **Hidden**: No output yet, terminal not visible
- **Visible**: Output produced, terminal shown below GUI window
- **Promoted**: GUI closed, terminal becomes standalone cell

### Promotion

When GUI window closes:
- If output terminal has content → promote to standalone
- If output terminal is empty → remove it
- Promoted terminal appears where GUI window was

## Output Detection

Output terminal becomes visible when:
- Any content written to stdout
- Any content written to stderr
- Terminal gets at least one row of content

## Layout

```
┌─────────────────┐
│   GUI Window    │
├─────────────────┤
│ Output Terminal │  (only if output exists)
└─────────────────┘
```

The output terminal appears directly below its associated GUI window.

## Foreground vs Background Mode

### Foreground Mode (default)

- Launching terminal hidden while GUI runs
- Focus moves to GUI window
- When GUI closes: launcher terminal reappears

### Background Mode (`gui -b`)

- Launching terminal stays visible
- Focus stays on launching terminal
- User can continue working while GUI runs

## Test Cases

1. `gui pqiv image.png` (no output) - no output terminal shown
2. `gui app-with-warnings` - output terminal appears with warnings
3. Close GUI with output - output terminal promoted to standalone
4. Close GUI without output - output terminal removed
5. `gui -b app` - launching terminal stays visible
6. Multiple GUI windows - each has own output terminal
