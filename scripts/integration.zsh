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
        elif [[ $ret -eq 3 ]]; then
            # TUI app - resize to full height, run, resize back
            column-term --resize full
            eval "$cmd"
            column-term --resize content
        fi
        zle reset-prompt
    }
    zle -N accept-line column-exec
fi
