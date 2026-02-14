# Termstack Specifications

Behavioral specs that define what the software should do. Reference these when implementing or debugging features. See [Glossary](glossary.md) for terminology.

## Specs

| Spec | Description |
|------|-------------|
| [Glossary](glossary.md) | Terminology used throughout termstack |
| [GUI Window Routing](gui-window-routing.md) | How windows route to host vs termstack |
| [Terminal Sizing](terminal-sizing.md) | Content-aware height, TUI detection, resize modes |
| [Shell Integration](shell-integration.md) | Command interception, builtin detection, syntax checking |
| [Column Layout](column-layout.md) | Scrolling, focus order, window positioning |
| [Keyboard Shortcuts](keyboard-shortcuts.md) | Key bindings and their actions |
| [Selection & Clipboard](selection-clipboard.md) | Text selection, copy/paste behavior |
| [Window Resize](window-resize.md) | Manual GUI window height adjustment |
| [Output Terminals](output-terminals.md) | GUI app stderr/stdout capture and promotion |

## Writing Specs

**Include:**
- Observable behavior ("when X happens, Y should occur")
- Invariants and constraints
- Edge cases that are easy to forget
- Test scenarios to verify correctness
- Decision rationale (why approach A over B)

**Avoid:**
- Implementation details (file paths, function names)
- Step-by-step algorithms
- Anything that changes weekly

Specs should be stable enough that they don't need updating every commit, but specific enough to catch bugs and guide implementation.
