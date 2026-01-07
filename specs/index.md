# Termstack Specifications

Termstack enables traditional terminal workflows while integrating GUI applications seamlessly—as if they were terminal apps. Each command runs in its own cell with content-aware sizing: short output stays compact, long output can be scrolled, and uninteresting output can be dismissed. GUI windows launched with the `gui` command appear inline in the column layout alongside terminal output.

Behavioral specs that define what the software should do. Reference these when implementing, testing, or debugging features.

## Specs

| Spec | Status | Description |
|------|--------|-------------|
| [GUI Window Routing](gui-window-routing.md) | Done | How windows route to host vs termstack |
| Terminal Sizing | Planned | Content-aware height, TUI detection, resize modes |
| Shell Integration | Planned | Command interception, builtin detection, exit codes |
| Column Layout | Planned | Scrolling, focus order, window positioning |
| Keyboard Shortcuts | Planned | Key bindings and their actions |
| Selection & Clipboard | Planned | Text selection, copy/paste behavior |
| Output Terminals | Planned | GUI app stderr/stdout capture and promotion |

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
