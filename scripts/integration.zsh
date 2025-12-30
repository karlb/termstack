# Only enable column-term integration inside column-compositor
if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
    column-exec() {
        local cmd="$BUFFER"
        [[ -z "$cmd" ]] && return

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
fi
