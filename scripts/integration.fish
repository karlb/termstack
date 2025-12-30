# Only enable column-term integration inside column-compositor
if set -q COLUMN_COMPOSITOR_SOCKET
    function column_exec
        set -l cmd (commandline)
        if test -z "$cmd"
            commandline -f execute
            return
        end

        # Check command type and syntax via column-term
        # Exit codes:
        #   0 = spawned in new terminal
        #   2 = shell builtin, run in current shell
        #   3 = incomplete/invalid syntax, let shell handle it
        column-term -c "$cmd"
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

    bind \r column_exec
    bind \n column_exec
end
