//! column-term - Spawn terminals in column-compositor
//!
//! This CLI tool allows spawning new terminals from the shell that share
//! the current terminal's environment. The new terminal appears above
//! the current one in the column layout.
//!
//! Commands are split into three categories:
//! - Shell builtins: run in current shell (cd, export, etc.)
//! - TUI apps: run in current terminal (vim, mc, fzf, top)
//! - GUI apps: spawn directly as Wayland clients (firefox, foot)
//! - Terminal commands: run in a new column-term (default)
//!
//! Configure in ~/.config/column-compositor/config.toml:
//! ```toml
//! gui_apps = ["firefox", "chromium", "foot"]
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
//! Add to your ~/.zshrc to make ALL commands go through column-term:
//! ```zsh
//! column-exec() {
//!     local cmd="$BUFFER"
//!     [[ -z "$cmd" ]] && return
//!     BUFFER=""
//!     column-term -c "$cmd"
//!     local ret=$?
//!     if [[ $ret -eq 2 ]]; then
//!         # Shell builtin - run in current shell
//!         eval "$cmd"
//!     elif [[ $ret -eq 3 ]]; then
//!         # TUI app - resize to full height, run, resize back
//!         column-term --resize full
//!         eval "$cmd"
//!         column-term --resize content
//!     fi
//!     zle reset-prompt
//! }
//! zle -N accept-line column-exec
//! ```
//!
//! Exit codes:
//! - 0: Command handled (spawned in terminal or as GUI app)
//! - 2: Shell builtin - run in current shell via eval
//! - 3: TUI app - resize terminal to full height, run via eval, resize back

use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[cfg(test)]
mod config_test;

/// Configuration for column-term
#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// List of commands that are GUI apps (spawn directly, not in terminal)
    #[serde(default)]
    gui_apps: Vec<String>,

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
            gui_apps: Vec::new(),
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

    /// Check if a command is a GUI app
    fn is_gui_app(&self, command: &str) -> bool {
        let program = Self::program_name(command);
        self.gui_apps.iter().any(|app| app == program)
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

    // Handle --resize flag first (before any command parsing)
    if args.len() >= 2 && args[1] == "--resize" {
        let mode = args.get(2).map(|s| s.as_str()).unwrap_or("full");
        return send_resize_request(mode);
    }

    // If we're running inside a TUI terminal (like mc's subshell), don't intercept.
    // This prevents mc's internal fish subshell commands from being spawned as
    // separate terminals, which would break mc's communication with its subshell.
    if env::var("COLUMN_COMPOSITOR_TUI").is_ok() {
        // Exit with code 2 (shell command) so the shell integration runs `eval "$cmd"`,
        // allowing mc's subshell command to actually execute in the current shell.
        std::process::exit(EXIT_SHELL_COMMAND);
    }

    // Parse arguments
    let command = parse_command(&args)?;

    // Empty command = interactive shell, always use terminal
    if command.is_empty() {
        return spawn_in_terminal(&command, false);
    }

    // Load config and check command type
    let config = Config::load();

    if config.is_shell_command(&command) {
        // Shell builtin - signal to run in current shell
        std::process::exit(EXIT_SHELL_COMMAND);
    } else if config.is_tui_app(&command) {
        // TUI app - shell should resize to full height, run command, resize back
        std::process::exit(EXIT_TUI_APP);
    } else if config.is_gui_app(&command) {
        spawn_gui_app(&command)
    } else {
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

/// Expand tilde in path to home directory
fn expand_tilde(s: &str) -> String {
    if s.starts_with("~/") {
        if let Ok(home) = env::var("HOME") {
            return format!("{}{}", home, &s[1..]);
        }
    }
    s.to_string()
}

/// Spawn command as a GUI app (directly, not in terminal)
fn spawn_gui_app(command: &str) -> Result<()> {
    // Clear the command line from the invoking terminal
    print!("\x1b[A\x1b[2K");
    std::io::stdout().flush().ok();

    // Parse command into program and args, expanding ~ in paths
    let parts: Vec<String> = command
        .split_whitespace()
        .map(expand_tilde)
        .collect();
    if parts.is_empty() {
        bail!("empty command");
    }

    let program = &parts[0];
    let args: Vec<&str> = parts[1..].iter().map(|s| s.as_str()).collect();

    // Determine which Wayland display to use
    // If COLUMN_COMPOSITOR_SOCKET is set, extract the display name from it
    // (the compositor sets both the socket and WAYLAND_DISPLAY to match)
    let wayland_display = if env::var("COLUMN_COMPOSITOR_SOCKET").is_ok() {
        // Socket is like /run/user/1000/column-compositor.sock
        // The compositor's WAYLAND_DISPLAY is in the same directory with wayland-N pattern
        // We need to query what the compositor actually uses
        // For now, use the env var which should be correct when running inside compositor
        env::var("WAYLAND_DISPLAY").ok()
    } else {
        None
    };

    // Spawn detached process
    let mut cmd = Command::new(program);
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Override WAYLAND_DISPLAY if we have one
    if let Some(display) = wayland_display {
        cmd.env("WAYLAND_DISPLAY", display);
        // GTK apps need GDK_BACKEND=wayland to use Wayland instead of defaulting to X11
        cmd.env("GDK_BACKEND", "wayland");
        // Qt apps need QT_QPA_PLATFORM=wayland similarly
        cmd.env("QT_QPA_PLATFORM", "wayland");
    }

    cmd.spawn()
        .with_context(|| format!("failed to spawn {}", program))?;

    Ok(())
}

/// Spawn command in a new column-term terminal
///
/// If is_tui is true, the terminal will be created at full viewport height
/// for TUI applications like vim, mc, fzf, etc.
fn spawn_in_terminal(command: &str, is_tui: bool) -> Result<()> {
    // Get socket path from environment
    let socket_path = env::var("COLUMN_COMPOSITOR_SOCKET")
        .context("COLUMN_COMPOSITOR_SOCKET not set - are you running inside column-compositor?")?;

    // Collect current environment
    let env_vars: std::collections::HashMap<String, String> = env::vars().collect();

    // Get current working directory
    let cwd = env::current_dir()
        .context("failed to get current directory")?
        .to_string_lossy()
        .to_string();

    // Build JSON message
    let msg = serde_json::json!({
        "type": "spawn",
        "command": command,
        "cwd": cwd,
        "env": env_vars,
        "is_tui": is_tui,
    });

    // Connect to compositor and send message
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;

    writeln!(stream, "{}", msg).context("failed to send message")?;
    stream.flush().context("failed to flush message")?;

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
