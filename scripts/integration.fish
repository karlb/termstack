# Only enable termstack integration inside termstack

if set -q TERMSTACK_SOCKET
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
            TERMSTACK_GUI_BACKGROUND=1 termstack gui $args
        else
            termstack gui $args
        end

        if set -q TERMSTACK_DEBUG
            echo "[gui] termstack exit code: $status" >&2
        end
    end

    function termstack_exec
        set -l cmd (commandline)
        if test -z "$cmd"
            commandline -f execute
            return
        end

        # Debug: show what command we're processing
        if set -q TERMSTACK_DEBUG
            echo "[termstack_exec] cmd='$cmd'" >&2
        end

        # Let 'gui' commands execute normally (handled by gui function above)
        # Check both with and without leading/trailing whitespace
        set -l trimmed_cmd (string trim "$cmd")
        if string match -q 'gui' "$trimmed_cmd"; or string match -q 'gui *' "$trimmed_cmd"
            if set -q TERMSTACK_DEBUG
                echo "[termstack_exec] detected gui command, executing normally" >&2
            end
            commandline -f execute
            return
        end

        # Check command type and syntax via termstack
        # Exit codes:
        #   0 = spawned in new terminal
        #   2 = shell builtin, run in current shell
        #   3 = incomplete/invalid syntax, let shell handle it
        termstack -c "$cmd"
        set -l ret $status

        switch $ret
            case 2 3
                # Shell builtin or incomplete syntax - let fish handle it
                # (fish shows continuation prompt for incomplete, error for invalid)
                commandline -f execute
            case '*'
                # Standard command - spawned in new terminal
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
