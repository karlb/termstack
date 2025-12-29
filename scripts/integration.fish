# Only enable column-term integration inside column-compositor
if set -q COLUMN_COMPOSITOR_SOCKET
    function column_exec
        set -l cmd (commandline)
        if test -z "$cmd"
            commandline -f execute
            return
        end

        # Check command type
        column-term -c "$cmd"
        set -l ret $status

        if test $ret -eq 2
            # Shell builtin (cd, export) - run in current shell
            # Let fish execute normally (handles history auto)
            commandline -f execute
        else if test $ret -eq 3
            # TUI app - run in current terminal
            history append -- "$cmd"
            commandline ""
            
            column-term --resize full
            eval "$cmd"
            sleep 0.05 # Allow TUI cleanup
            column-term --resize content
            
            commandline -f repaint
        else
            # Standard command - spawned in new terminal
            history append -- "$cmd"
            commandline ""
            commandline -f repaint
        end
    end

    bind \r column_exec
    bind \n column_exec
end
