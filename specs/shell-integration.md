# Shell Integration Specification

Shell integration intercepts commands at the prompt and routes them appropriately.
All classification happens in the fish script (`scripts/integration.fish`) — the
CLI binary is only called to spawn terminals or send IPC messages.

## Command Routing

When the user presses Enter at the prompt:

1. Fish captures the command line and prompt string
2. Empty command: sends `--builtin` IPC to create a blank prompt entry
3. `TERMSTACK_TUI` set: delegates to `commandline -f execute` (TUI subshell)
4. `gui` prefix: delegates to `commandline -f execute` (gui function)
5. Syntax invalid/incomplete (`commandline --is-valid` non-zero): delegates to fish
6. First word in `$__termstack_shell_commands`: runs via `eval` in current shell,
   captures output, sends `--builtin` IPC to create a stack entry
7. Everything else: calls `termstack -c "command"` which spawns a new terminal

## Shell Commands

Only commands that **modify the launcher shell's state** run in the current shell.
Everything else spawns in a new terminal.

Default list (`$__termstack_shell_commands`):

- Directory: `cd`, `pushd`, `popd`, `dirs`
- Environment: `set`, `export`, `unset`
- Sourcing: `source`, `.`
- Aliases: `alias`, `unalias`, `abbr`
- Session: `exit`, `logout`, `exec`
- Other: `eval`

Users can override before sourcing the integration script:

```fish
set -g __termstack_shell_commands cd pushd popd set export source eval
```

Commands intentionally excluded (not state-affecting at the launcher level):
`type`, `which`, `hash` (read-only), `jobs`, `fg`, `bg`, `disown`, `wait`
(no jobs in launcher), `builtin`, `command` (modifiers), `local`, `declare`,
`typeset`, `readonly`, `shift` (function-scoped), `trap`, `ulimit`, `umask`
(process-scoped), `history`, `fc` (read-only in practice).

## Syntax Checking

Fish's native `commandline --is-valid` (fish 3.4+) checks syntax before routing:

- Return 0: valid, proceed with classification
- Return 1: syntax error, delegate to fish (shows error)
- Return 2: incomplete, delegate to fish (shows continuation prompt)

This prevents spawning terminals for incomplete commands and avoids the latency
of spawning a subprocess for syntax checking.

## Shell Command Output

Shell commands run via `eval` with stdout/stderr captured to a temp file. The
output and exit status are sent to the compositor via `--builtin` IPC, which
creates a stack entry that looks identical to a regular terminal (same title bar
showing `$PROMPT command`, same content area showing output).

## Environment Inheritance

New terminals inherit the current shell's environment:
- All environment variables (serialized as JSON in the spawn IPC message)
- Current working directory
- But NOT shell-local variables or functions

## TUI Terminal Detection

When `TERMSTACK_TUI` is set (inside a TUI app's subshell):
- Fish delegates directly to `commandline -f execute`
- All commands run in the TUI's shell, not intercepted
- Prevents TUI subshells from spawning external terminals

## Test Cases

1. `cd /tmp` — runs in current shell, directory changes, entry appears in stack
2. `ls` — spawns new terminal, output appears there
3. `echo "unclosed` — fish shows continuation prompt
4. `for` — fish shows continuation prompt
5. `type ls` — spawns in new terminal (not state-affecting)
6. `set -gx FOO bar` — runs in current shell, env changes
7. `gui firefox` — launches GUI app
8. Empty Enter — creates blank prompt entry
9. Inside mc subshell: commands run in mc's shell, not new terminals
