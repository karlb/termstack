# Only enable termstack integration inside termstack

if set -q TERMSTACK_SOCKET
    # Use TERMSTACK_BIN if set, otherwise fall back to 'termstack' in PATH
    if not set -q TERMSTACK_BIN
        set TERMSTACK_BIN termstack
    end

    # Shell commands that modify launcher shell state — run in current shell.
    # Users can extend via: set -g __termstack_shell_commands ... in config.fish
    if not set -q __termstack_shell_commands
        set -g __termstack_shell_commands \
            cd pushd popd dirs \
            set export unset \
            source . \
            alias unalias abbr \
            exit logout exec \
            eval
    end

    # Define 'gui' as a function for launching GUI apps
    # Usage: gui <command>           # foreground mode (launcher hidden until GUI exits)
    # Usage: gui -b <command>        # background mode (launcher stays visible)
    # Usage: gui --background <command>
    function gui --description "Launch GUI app in termstack"
        set -l background 0
        set -l args

        # Parse arguments
        for arg in $argv
            switch $arg
                case -b --background
                    set background 1
                case '*'
                    set -a args $arg
            end
        end

        if test (count $args) -eq 0
            echo "Usage: gui [-b|--background] <command> [args...]" >&2
            echo "  -b, --background  Keep launching terminal visible" >&2
            return 1
        end

        if set -q TERMSTACK_DEBUG
            echo "[gui] calling: termstack gui $args" >&2
        end

        if test $background -eq 1
            TERMSTACK_GUI_BACKGROUND=1 $TERMSTACK_BIN gui $args
        else
            $TERMSTACK_BIN gui $args
        end

        if set -q TERMSTACK_DEBUG
            echo "[gui] termstack exit code: $status" >&2
        end
    end

    function termstack_exec
        set -l cmd (commandline)

        # Capture prompt BEFORE any command execution (shows state at command entry time)
        set -l prompt_str (fish_prompt | string collect)

        # Handle empty command (just pressing Enter)
        if test -z "$cmd"
            # Create entry showing just the prompt (like a normal terminal)
            $TERMSTACK_BIN --builtin "$prompt_str" "" ""
            commandline ""
            commandline -f repaint
            return
        end

        set -l trimmed (string trim "$cmd")
        set -l first_word (string split ' ' -- $trimmed)[1]

        # Debug: show what command we're processing
        if set -q TERMSTACK_DEBUG
            echo "[termstack_exec] cmd='$cmd' first_word='$first_word'" >&2
        end

        # TUI subshell — run everything in current shell
        if set -q TERMSTACK_TUI
            commandline -f execute
            return
        end

        # Let 'gui' commands execute normally (handled by gui function above)
        if test "$first_word" = gui
            commandline -f execute
            return
        end

        # Syntax check (fish 3.4+): 0 = valid, 1 = error, 2 = incomplete
        commandline --is-valid
        if test $status -ne 0
            commandline -f execute
            return
        end

        # State-affecting commands — run in current shell, record in stack
        if contains -- $first_word $__termstack_shell_commands
            set -l tmpfile (mktemp)

            # eval runs in current shell context, so cd/export/etc affect this shell
            # Redirect stdout+stderr to temp file to capture output
            eval $cmd >$tmpfile 2>&1
            set -l exit_status $status

            # Read captured output (may be empty for cd, export, etc.)
            set -l output (cat $tmpfile)
            rm -f $tmpfile

            # Determine success/error flag
            set -l error_flag
            if test $exit_status -ne 0
                set error_flag "--error"
            end

            # Send to compositor (creates persistent entry in stack)
            $TERMSTACK_BIN --builtin "$prompt_str" "$cmd" "$output" $error_flag

            # Add to history and clear command line
            history append -- "$cmd"
            commandline ""
            commandline -f repaint
        else
            # Regular command — spawn in new terminal
            TERMSTACK_PROMPT="$prompt_str" $TERMSTACK_BIN -c "$cmd"

            history append -- "$cmd"
            commandline ""
            commandline -f repaint
        end
    end

    bind \r termstack_exec
    bind \n termstack_exec
else
    # Not inside compositor - only show message if sourced interactively (not from config.fish)
    if status --is-interactive; and test -z "$__termstack_integration_sourced"
        echo "Note: termstack shell integration not active" >&2
        echo "      (TERMSTACK_SOCKET not set)" >&2
        echo "      Start termstack first, then source this script." >&2
    end
end

# Mark that we've been sourced (prevents repeated messages in config.fish)
set -g __termstack_integration_sourced 1
