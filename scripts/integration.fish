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
        else
            # Standard command - spawned in new terminal
            # TUI apps are auto-detected via alternate screen mode
            history append -- "$cmd"
            commandline ""
            commandline -f repaint
        end
    end

    bind \r column_exec
    bind \n column_exec
end
