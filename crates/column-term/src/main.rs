//! column-term - Spawn terminals in column-compositor
//!
//! This CLI tool allows spawning new terminals from the shell that share
//! the current terminal's environment. The new terminal appears above
//! the current one in the column layout.
//!
//! Commands are split into categories:
//! - Shell builtins: run in current shell (cd, export, etc.)
//! - TUI apps: run in current terminal at full height (vim, mc, fzf, top)
//! - All other commands: run in a new column-term terminal
//!
//! GUI apps automatically get an output terminal that appears below the
//! window when stderr/stdout is produced.
//!
//! Configure in ~/.config/column-compositor/config.toml:
//! ```toml
//! tui_apps = ["vim", "nvim", "mc", "htop", "fzf", "less", "man"]
//! ```
//!
//! # Usage
//!
//! ```bash
//! # Run a command in a new terminal
//! column-term -c "git status"
//!
//! # Run with no command (opens interactive shell)
//! column-term
//! ```
//!
//! # Shell Integration
//!
//! Scripts are available in the `scripts/` directory of the repository.
//!
//! ## Zsh
//!
//! Add to your `~/.zshrc` (or source `scripts/integration.zsh`):
//! ```zsh
//! # Only enable column-term integration inside column-compositor
//! if [[ -n "$COLUMN_COMPOSITOR_SOCKET" ]]; then
//!     column-exec() {
//!         local cmd="$BUFFER"
//!         [[ -z "$cmd" ]] && return
//!         
//!         # Save to history
//!         print -s "$cmd"
//!         
//!         BUFFER=""
//!         column-term -c "$cmd"
//!         local ret=$?
//!         
//!         if [[ $ret -eq 2 ]]; then
//!             # Shell builtin - run in current shell
//!             eval "$cmd"
//!         elif [[ $ret -eq 3 ]]; then
//!             # TUI app - resize to full height, run, resize back
//!             column-term --resize full
//!             eval "$cmd"
//!             column-term --resize content
//!         fi
//!         zle reset-prompt
//!     }
//!     zle -N accept-line column-exec
//! fi
//! ```
//!
//! ## Fish
//!
//! Add to your `~/.config/fish/config.fish` (or source `scripts/integration.fish`):
//! ```fish
//! # Only enable column-term integration inside column-compositor
//! if set -q COLUMN_COMPOSITOR_SOCKET
//!     function column_exec
//!         set -l cmd (commandline)
//!         if test -z "$cmd"
//!             commandline -f execute
//!             return
//!         end
//!
//!         # Check command type
//!         column-term -c "$cmd"
//!         set -l ret $status
//!
//!         if test $ret -eq 2
//!             # Shell builtin (cd, export) - run in current shell
//!             # Let fish execute normally (handles history auto)
//!             commandline -f execute
//!         else if test $ret -eq 3
//!             # TUI app - run in current terminal
//!             history append -- "$cmd"
//!             commandline ""
//!             
//!             column-term --resize full
//!             eval "$cmd"
//!             sleep 0.05 # Allow TUI cleanup
//!             column-term --resize content
//!             
//!             commandline -f repaint
//!         else
//!             # Standard command - spawned in new terminal
//!             history append -- "$cmd"
//!             commandline ""
//!             commandline -f repaint
//!         end
//!     end
//!
//!     bind \r column_exec
//!     bind \n column_exec
//! end
//! ```
//!
//! Exit codes:
//! - 0: Command handled (spawned in terminal)
//! - 2: Shell builtin - run in current shell via eval
//! - 3: TUI app - resize terminal to full height, run via eval, resize back

use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[cfg(test)]
mod config_test;

/// Configuration for column-term
#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// List of shell builtins/commands that should run in the current shell
    #[serde(default = "Config::default_shell_commands")]
    shell_commands: Vec<String>,

    /// List of TUI apps that should run in the current terminal (vim, mc, fzf, etc.)
    #[serde(default)]
    tui_apps: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell_commands: Self::default_shell_commands(),
            tui_apps: Vec::new(),
        }
    }
}

impl Config {
    fn default_shell_commands() -> Vec<String> {
        vec![
            "cd", "pushd", "popd", "dirs",
            "export", "unset", "set",
            "source", ".",
            "alias", "unalias",
            "hash", "type", "which",
            "jobs", "fg", "bg", "disown",
            "exit", "logout",
            "exec",
            "eval",
            "builtin", "command",
            "local", "declare", "typeset", "readonly",
            "shift",
            "trap",
            "ulimit", "umask",
            "wait",
            "history", "fc",
        ].into_iter().map(String::from).collect()
    }

    /// Load config from ~/.config/column-compositor/config.toml
    fn load() -> Self {
        let config_path = Self::config_path();

        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            match toml::from_str(&contents) {
                Ok(config) => return config,
                Err(e) => {
                    eprintln!("warning: failed to parse config: {}", e);
                }
            }
        }

        Self::default()
    }

    fn config_path() -> PathBuf {
        let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("column-compositor")
            .join("config.toml")
    }

    /// Extract the program name from a command (first word, without path)
    fn program_name(command: &str) -> &str {
        command
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
    }

    /// Check if a command is a shell builtin that should run in current shell
    fn is_shell_command(&self, command: &str) -> bool {
        let program = Self::program_name(command);
        self.shell_commands.iter().any(|cmd| cmd == program)
    }

    /// Check if a command is a TUI app that should run in current terminal
    ///
    /// This checks ALL commands in a pipeline/chain, not just the first.
    /// For example, "echo a | fzf" should detect fzf as a TUI app.
    fn is_tui_app(&self, command: &str) -> bool {
        // Split on pipe and command separators to get all commands
        for segment in command.split(['|', ';', '&']) {
            let segment = segment.trim();
            // Skip empty segments and && continuation
            if segment.is_empty() || segment == "&" {
                continue;
            }
            let program = Self::program_name(segment);
            if self.tui_apps.iter().any(|app| app == program) {
                return true;
            }
        }
        false
    }
}

/// Exit code indicating command should run in current shell
const EXIT_SHELL_COMMAND: i32 = 2;

/// Exit code indicating TUI app - shell should resize terminal, run, then resize back
const EXIT_TUI_APP: i32 = 3;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Debug: show we're running (only if DEBUG_COLUMN_TERM is set)
    let debug = env::var("DEBUG_COLUMN_TERM").is_ok();
    if debug {
        eprintln!("[column-term] args: {:?}", args);
        eprintln!("[column-term] COLUMN_COMPOSITOR_SOCKET={:?}", env::var("COLUMN_COMPOSITOR_SOCKET"));
    }

    // Handle --resize flag first (before any command parsing)
    if args.len() >= 2 && args[1] == "--resize" {
        let mode = args.get(2).map(|s| s.as_str()).unwrap_or("full");
        return send_resize_request(mode);
    }

    // If we're running inside a TUI terminal (like mc's subshell), don't intercept.
    // This prevents mc's internal fish subshell commands from being spawned as
    // separate terminals, which would break mc's communication with its subshell.
    if env::var("COLUMN_COMPOSITOR_TUI").is_ok() {
        if debug { eprintln!("[column-term] COLUMN_COMPOSITOR_TUI set, exit 2"); }
        // Exit with code 2 (shell command) so the shell integration runs `eval "$cmd"`,
        // allowing mc's subshell command to actually execute in the current shell.
        std::process::exit(EXIT_SHELL_COMMAND);
    }

    // Parse arguments
    let command = parse_command(&args)?;
    if debug { eprintln!("[column-term] command: {:?}", command); }

    // Empty command = interactive shell, always use terminal
    if command.is_empty() {
        if debug { eprintln!("[column-term] empty command, spawning shell"); }
        return spawn_in_terminal(&command, false);
    }

    // Load config and check command type
    let config = Config::load();

    if config.is_shell_command(&command) {
        if debug { eprintln!("[column-term] shell command, exit 2"); }
        // Shell builtin - signal to run in current shell
        std::process::exit(EXIT_SHELL_COMMAND);
    } else if config.is_tui_app(&command) {
        if debug { eprintln!("[column-term] TUI app, exit 3"); }
        // TUI app - shell should resize to full height, run command, resize back
        std::process::exit(EXIT_TUI_APP);
    } else {
        if debug { eprintln!("[column-term] spawning in terminal"); }
        // Regular command or GUI app - run in terminal
        // (GUI apps will have their output terminal hidden until errors appear)
        spawn_in_terminal(&command, false)
    }
}

/// Send a resize request to the compositor and wait for acknowledgement
///
/// This is synchronous to prevent race conditions with TUI apps that query
/// terminal size immediately after starting.
fn send_resize_request(mode: &str) -> Result<()> {
    use std::io::{BufRead, BufReader};

    let socket_path = env::var("COLUMN_COMPOSITOR_SOCKET")
        .context("COLUMN_COMPOSITOR_SOCKET not set - are you running inside column-compositor?")?;

    let mode_str = match mode {
        "full" => "full",
        "content" => "content",
        _ => bail!("invalid resize mode: {} (expected 'full' or 'content')", mode),
    };

    let msg = serde_json::json!({
        "type": "resize",
        "mode": mode_str,
    });

    let stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;

    // Set timeout for reading ACK (don't want to hang forever)
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .context("failed to set read timeout")?;

    let mut stream_write = stream.try_clone().context("failed to clone stream")?;

    writeln!(stream_write, "{}", msg).context("failed to send resize message")?;
    stream_write.flush().context("failed to flush resize message")?;

    // Wait for ACK from compositor - this ensures resize is complete before we return
    let mut reader = BufReader::new(stream);
    let mut ack = String::new();
    reader.read_line(&mut ack).context("failed to read resize ACK")?;

    if ack.trim() != "ok" {
        bail!("unexpected resize ACK: {}", ack.trim());
    }

    Ok(())
}

/// Spawn command in a new column-term terminal
///
/// If is_tui is true, the terminal will be created at full viewport height
/// for TUI applications like vim, mc, fzf, etc.
fn spawn_in_terminal(command: &str, is_tui: bool) -> Result<()> {
    let debug = env::var("DEBUG_COLUMN_TERM").is_ok();

    // Get socket path from environment
    let socket_path = env::var("COLUMN_COMPOSITOR_SOCKET")
        .context("COLUMN_COMPOSITOR_SOCKET not set - are you running inside column-compositor?")?;
    if debug { eprintln!("[column-term] socket path: {}", socket_path); }

    // Collect current environment
    let env_vars: std::collections::HashMap<String, String> = env::vars().collect();

    // Get current working directory
    let cwd = env::current_dir()
        .context("failed to get current directory")?
        .to_string_lossy()
        .to_string();
    if debug { eprintln!("[column-term] cwd: {}", cwd); }

    // Build JSON message
    let msg = serde_json::json!({
        "type": "spawn",
        "command": command,
        "cwd": cwd,
        "env": env_vars,
        "is_tui": is_tui,
    });

    if debug { eprintln!("[column-term] connecting to socket..."); }
    // Connect to compositor and send message
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;
    if debug { eprintln!("[column-term] connected, sending message..."); }

    writeln!(stream, "{}", msg).context("failed to send message")?;
    stream.flush().context("failed to flush message")?;
    if debug { eprintln!("[column-term] message sent successfully"); }

    // Clear the command line from the invoking terminal
    if !command.is_empty() {
        print!("\x1b[A\x1b[2K");
        std::io::stdout().flush().ok();
    }

    Ok(())
}

/// Parse command from arguments
///
/// Supports:
/// - `column-term -c "command"` - run command
/// - `column-term` - run interactive shell (empty command)
fn parse_command(args: &[String]) -> Result<String> {
    if args.len() == 1 {
        // No arguments - spawn interactive shell
        return Ok(String::new());
    }

    if args.len() == 3 && args[1] == "-c" {
        // -c "command"
        return Ok(args[2].clone());
    }

    if args.len() == 2 && args[1] == "-c" {
        bail!("missing command after -c");
    }

    // Treat all remaining args as the command
    if args.len() >= 2 {
        if args[1] == "-c" {
            // -c with multiple args: join them
            return Ok(args[2..].join(" "));
        }
        // No -c: join all args as command
        return Ok(args[1..].join(" "));
    }

    bail!("usage: column-term [-c command]");
}
