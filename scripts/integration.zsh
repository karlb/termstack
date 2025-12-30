# Only enable column-term integration inside column-compositor
if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
    column-exec() {
        local cmd="$BUFFER"
        [[ -z "$cmd" ]] && return

        # Save to history
        print -s "$cmd"

        BUFFER=""
        column-term -c "$cmd"
        local ret=$?

        if [[ $ret -eq 2 ]]; then
            # Shell builtin - run in current shell
            eval "$cmd"
        fi
        zle reset-prompt
    }
    zle -N accept-line column-exec
fi
