# Shell Integration Specification

Shell integration intercepts commands at the prompt and routes them appropriately.

## Command Routing

When the user presses Enter at the prompt:

1. Shell integration captures the command line
2. Sends it to `termstack -c "command"`
3. Based on exit code, either:
   - Exit 0: Command spawned in new terminal, clear prompt
   - Exit 2: Shell builtin, execute in current shell
   - Exit 3: Incomplete syntax, let shell handle it

## Shell Builtins

These commands run in the current shell (not a new terminal):

- Directory: `cd`, `pushd`, `popd`, `dirs`
- Environment: `export`, `unset`, `set`
- Sourcing: `source`, `.`
- Aliases: `alias`, `unalias`
- Job control: `jobs`, `fg`, `bg`, `disown`
- Session: `exit`, `logout`, `exec`
- Variables: `local`, `declare`, `typeset`, `readonly`
- Other: `eval`, `builtin`, `command`, `shift`, `trap`, `ulimit`, `umask`, `wait`, `history`, `fc`

Rationale: These commands affect shell state that doesn't transfer to a new process.

## Syntax Checking

Before spawning a terminal, the command is checked for completeness:

- Unclosed quotes → exit 3 (shell shows continuation prompt)
- Unclosed brackets → exit 3
- Trailing pipe/operator → exit 3
- Parse errors → exit 3

This prevents spawning terminals for incomplete commands.

## Exit Codes

| Code | Meaning | Shell Action |
|------|---------|--------------|
| 0 | Spawned in new terminal | Clear command line, add to history |
| 2 | Shell builtin | Execute via `eval "$cmd"` |
| 3 | Incomplete syntax | Let shell handle normally |

## Environment Inheritance

New terminals inherit the current shell's environment:
- All environment variables
- Current working directory
- But NOT shell-local variables or functions

## TUI Terminal Detection

When `TERMSTACK_TUI` is set (inside a TUI app's subshell):
- Always exit 2 (run in current shell)
- Prevents TUI subshells from spawning external terminals

## Test Cases

1. `cd /tmp` - runs in current shell, directory changes
2. `ls` - spawns new terminal, output appears there
3. `echo "unclosed` - shell shows continuation prompt
4. `git status | ` - shell shows continuation prompt
5. Inside mc subshell: commands run in mc's shell, not new terminals
