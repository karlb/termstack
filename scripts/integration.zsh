# Only enable column-term integration inside column-compositor
if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
    # Define 'gui' as a function for launching GUI apps
    # Usage: gui <command>           # foreground mode (launcher hidden until GUI exits)
    # Usage: gui -b <command>        # background mode (launcher stays visible)
    gui() {
        local background=0
        local args=()

        # Parse arguments
        while [[ $# -gt 0 ]]; do
            case "$1" in
                -b|--background)
                    background=1
                    shift
                    ;;
                *)
                    args+=("$1")
                    shift
                    ;;
            esac
        done

        if [[ ${#args[@]} -eq 0 ]]; then
            echo "Usage: gui [-b|--background] <command> [args...]" >&2
            echo "  -b, --background  Keep launching terminal visible" >&2
            return 1
        fi

        if [[ $background -eq 1 ]]; then
            COLUMN_GUI_BACKGROUND=1 column-term gui "${args[@]}"
        else
            column-term gui "${args[@]}"
        fi
    }

    column-exec() {
        local cmd="$BUFFER"
        [[ -z "$cmd" ]] && return

        # Let 'gui' commands execute normally (handled by gui function above)
        if [[ "$cmd" =~ ^gui($|[[:space:]]) ]]; then
            zle .accept-line
            return
        fi

        # Check command type and syntax via column-term
        # Exit codes:
        #   0 = spawned in new terminal
        #   2 = shell builtin, run in current shell
        #   3 = incomplete/invalid syntax, let shell handle it
        column-term -c "$cmd"
        local ret=$?

        if [[ $ret -eq 3 ]]; then
            # Incomplete/invalid syntax - let zsh handle it
            # (shows continuation prompt or syntax error)
            zle .accept-line
            return
        fi

        if [[ $ret -eq 2 ]]; then
            # Shell builtin - run in current shell
            print -s "$cmd"
            BUFFER=""
            eval "$cmd"
        else
            # Standard command - spawned in new terminal
            print -s "$cmd"
            BUFFER=""
        fi
        zle reset-prompt
    }
    zle -N accept-line column-exec
elif [[ -o interactive ]] && [[ -z "$__column_integration_sourced" ]]; then
    echo "Note: column-compositor shell integration not active" >&2
    echo "      (COLUMN_COMPOSITOR_SOCKET not set)" >&2
    echo "      Start column-compositor first, then source this script." >&2
fi

# Mark that we've been sourced (prevents repeated messages)
__column_integration_sourced=1
