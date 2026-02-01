# Output Terminals Specification

GUI applications can have associated output terminals for stderr/stdout.

## Purpose

When a GUI app produces terminal output (errors, warnings, logs), that output appears in a dedicated terminal cell below the GUI window.

## Lifecycle

1. User runs `gui <command>` - output terminal created (hidden)
2. Output produced - terminal becomes visible below GUI window
3. GUI closes:
   - If output exists → terminal promoted to standalone
   - If empty → terminal removed

**Visibility:** Terminal appears when first content (stdout/stderr) is written.

**Promotion:** On GUI close, the output terminal takes the GUI's position in the stack.

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
