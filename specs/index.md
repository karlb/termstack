# Termstack Specifications

Behavioral specs that define what the software should do. Reference these when implementing, testing, or debugging features.

## Specs

| Spec | Description |
|------|-------------|
| [GUI Window Routing](gui-window-routing.md) | How windows route to host vs termstack |

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
