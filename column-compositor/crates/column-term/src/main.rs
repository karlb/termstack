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
//!     fi
//!     zle reset-prompt
//! }
//! zle -N accept-line column-exec
//! ```
//!
//! Exit codes:
//! - 0: Command handled (spawned in terminal or as GUI app)
//! - 2: Shell/TUI command - should run in current shell/terminal

use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Configuration for column-term
#[derive(Debug, Deserialize)]
struct Config {
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
    fn is_tui_app(&self, command: &str) -> bool {
        let program = Self::program_name(command);
        self.tui_apps.iter().any(|app| app == program)
    }
}

/// Exit code indicating command should run in current shell
const EXIT_SHELL_COMMAND: i32 = 2;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Parse arguments
    let command = parse_command(&args)?;

    // Empty command = interactive shell, always use terminal
    if command.is_empty() {
        return spawn_in_terminal(&command);
    }

    // Load config and check command type
    let config = Config::load();

    if config.is_shell_command(&command) {
        // Shell builtin - signal to run in current shell
        std::process::exit(EXIT_SHELL_COMMAND);
    } else if config.is_tui_app(&command) {
        // TUI app - signal to run in current terminal
        std::process::exit(EXIT_SHELL_COMMAND);
    } else if config.is_gui_app(&command) {
        spawn_gui_app(&command)
    } else {
        spawn_in_terminal(&command)
    }
}

/// Spawn command as a GUI app (directly, not in terminal)
fn spawn_gui_app(command: &str) -> Result<()> {
    // Clear the command line from the invoking terminal
    print!("\x1b[A\x1b[2K");
    std::io::stdout().flush().ok();

    // Parse command into program and args
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        bail!("empty command");
    }

    let program = parts[0];
    let args = &parts[1..];

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
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Override WAYLAND_DISPLAY if we have one
    if let Some(display) = wayland_display {
        cmd.env("WAYLAND_DISPLAY", display);
    }

    cmd.spawn()
        .with_context(|| format!("failed to spawn {}", program))?;

    Ok(())
}

/// Spawn command in a new column-term terminal
fn spawn_in_terminal(command: &str) -> Result<()> {
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
