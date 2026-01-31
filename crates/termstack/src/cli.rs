//! termstack CLI - Spawn terminals in TermStack
//!
//! This CLI tool allows spawning new terminals from the shell that share
//! the current terminal's environment. The new terminal appears above
//! the current one in the stack layout.
//!
//! Commands are split into categories:
//! - Shell builtins: run in current shell (cd, export, etc.)
//! - All other commands: run in a new terminal window
//!
//! TUI apps (vim, mc, etc.) are auto-detected via alternate screen mode
//! and automatically resized to full viewport height.
//!
//! GUI apps automatically get an output terminal that appears below the
//! window when stderr/stdout is produced.
//!
//! # Usage
//!
//! ```fish
//! # Run a command in a new terminal
//! termstack -c "git status"
//!
//! # Run with no command (opens interactive shell)
//! termstack
//! ```
//!
//! # Shell Integration
//!
//! The integration script is available in the `scripts/` directory of the repository.
//!
//! ## Fish
//!
//! Add to your `~/.config/fish/config.fish` (or source `scripts/integration.fish`):
//! ```fish
//! if set -q TERMSTACK_SOCKET
//!     function termstack_exec
//!         set -l cmd (commandline)
//!         test -z "$cmd"; and commandline -f execute; and return
//!
//!         termstack -c "$cmd"
//!         switch $status
//!             case 2 3  # Shell builtin or incomplete syntax
//!                 commandline -f execute
//!             case '*'  # Spawned in new terminal
//!                 history append -- "$cmd"
//!                 commandline ""
//!                 commandline -f repaint
//!         end
//!     end
//!     bind \r termstack_exec
//!     bind \n termstack_exec
//! end
//! ```
//!
//! Exit codes:
//! - 0: Command spawned in new terminal
//! - 2: Shell builtin - run in current shell via eval
//! - 3: Incomplete/invalid syntax - let shell handle it

use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::shell::{detect_shell, Shell};
use crate::util::debug_enabled;

/// Configuration for termstack
#[derive(Debug, Deserialize)]
pub(crate) struct Config {
    /// List of shell builtins/commands that should run in the current shell
    #[serde(default = "Config::default_shell_commands")]
    shell_commands: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell_commands: Self::default_shell_commands(),
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

    /// Load config from ~/.config/termstack/config.toml
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
            .join("termstack")
            .join("config.toml")
    }

    /// Check if a command is a shell builtin that should run in current shell
    pub fn is_shell_command(&self, command: &str, shell: &dyn Shell) -> bool {
        shell.is_builtin(command, &self.shell_commands)
    }
}

/// Exit code indicating command should run in current shell
const EXIT_SHELL_COMMAND: i32 = 2;

/// Exit code indicating command has incomplete/invalid syntax
/// Shell integration should let the shell handle it (show continuation or error)
const EXIT_INCOMPLETE_SYNTAX: i32 = 3;


pub fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Debug: show we're running (only if DEBUG_COLUMN_TERM is set)
    let debug = debug_enabled();
    if debug {
        eprintln!("[termstack] args: {:?}", args);
        eprintln!("[termstack] TERMSTACK_SOCKET={:?}", env::var("TERMSTACK_SOCKET"));
    }

    // Handle --status flag for diagnostics
    if args.len() >= 2 && args[1] == "--status" {
        let socket = env::var("TERMSTACK_SOCKET");
        let shell = env::var("SHELL").unwrap_or_else(|_| "(not set)".to_string());

        println!("termstack status:");
        println!("  TERMSTACK_SOCKET: {}", match &socket {
            Ok(path) => format!("{} (exists: {})", path, std::path::Path::new(path).exists()),
            Err(_) => "NOT SET - shell integration will not activate".to_string(),
        });
        println!("  SHELL: {}", shell);
        println!();

        if socket.is_ok() {
            println!("Shell integration should be active.");
            println!("If 'gui' command is not found, make sure to source the integration script:");
            println!("  fish: source scripts/integration.fish");
        } else {
            println!("You are NOT inside termstack.");
            println!("Start the compositor first, then the shell integration will activate.");
        }
        return Ok(());
    }

    // Handle diagnose subcommand for X11/Wayland diagnostics
    if args.len() >= 2 && args[1] == "diagnose" {
        return run_diagnostics();
    }

    // Handle test-x11 subcommand for testing X11 connectivity
    if args.len() >= 2 && args[1] == "test-x11" {
        return test_x11_connectivity();
    }

    // Handle query-windows subcommand for testing/debugging
    if args.len() >= 2 && args[1] == "query-windows" {
        return query_windows();
    }

    // Handle --resize flag first (before any command parsing)
    if args.len() >= 2 && args[1] == "--resize" {
        let mode = args.get(2).map(|s| s.as_str()).unwrap_or("full");
        return send_resize_request(mode);
    }

    // Handle --builtin flag for shell builtin notifications
    // Usage: termstack --builtin "prompt" "command" "output" [--error]
    if args.len() >= 2 && args[1] == "--builtin" {
        return send_builtin_notification(&args[2..]);
    }

    // Handle 'gui' subcommand for launching GUI apps with foreground/background mode
    // Usage: termstack gui <command>
    // Background mode: TERMSTACK_GUI_BACKGROUND=1 termstack gui <command>
    if debug {
        eprintln!("[termstack] checking gui subcommand: args.len()={}, args[1]={:?}", args.len(), args.get(1));
    }
    if args.len() >= 2 && args[1] == "gui" {
        if args.len() < 3 {
            bail!("usage: termstack gui <command>");
        }
        let command = args[2..].join(" ");
        // Background mode is set by shell integration when user adds & suffix
        let foreground = env::var("TERMSTACK_GUI_BACKGROUND").is_err();
        if debug {
            eprintln!("[termstack] gui spawn: command={:?} foreground={}", command, foreground);
        }
        return spawn_gui_app(&command, foreground);
    }

    // If we're running inside a TUI terminal (like mc's subshell), don't intercept.
    // This prevents mc's internal fish subshell commands from being spawned as
    // separate terminals, which would break mc's communication with its subshell.
    if env::var("TERMSTACK_TUI").is_ok() {
        if debug { eprintln!("[termstack] TERMSTACK_TUI set, exit 2"); }
        // Exit with code 2 (shell command) so the shell integration runs `eval "$cmd"`,
        // allowing mc's subshell command to actually execute in the current shell.
        std::process::exit(EXIT_SHELL_COMMAND);
    }

    // Parse arguments
    let command = parse_command(&args)?;
    if debug { eprintln!("[termstack] command: {:?}", command); }

    // Get prompt from environment (set by fish integration)
    let prompt = env::var("TERMSTACK_PROMPT").unwrap_or_default();
    if debug { eprintln!("[termstack] prompt: {:?}", prompt); }

    // Empty command = interactive shell, always use terminal
    if command.is_empty() {
        if debug { eprintln!("[termstack] empty command, spawning shell"); }
        return spawn_in_terminal(&command, &prompt);
    }

    // Check if command is a termstack subcommand - execute it directly
    // This handles the case where fish integration intercepts "termstack test-x11"
    // and calls "termstack -c 'termstack test-x11'" - we run the subcommand here
    // instead of returning exit code 2 (which would use PATH, not TERMSTACK_BIN)
    if let Some(subcommand) = extract_termstack_subcommand(&command) {
        if debug { eprintln!("[termstack] executing termstack subcommand directly: {}", subcommand); }
        return execute_subcommand(&subcommand);
    }

    // Detect shell and normalize command
    let shell = detect_shell();
    let normalized_command = shell.normalize_command(&command);
    if debug && normalized_command != command {
        eprintln!("[termstack] normalized: '{}' -> '{}'", command, normalized_command);
    }

    // Load config and check command type
    let config = Config::load();

    if config.is_shell_command(&normalized_command, shell.as_ref()) {
        if debug { eprintln!("[termstack] shell command, exit 2"); }
        // Shell builtin - signal to run in current shell
        std::process::exit(EXIT_SHELL_COMMAND);
    }

    // Check if command is syntactically complete
    // If not, let the shell handle it (show continuation prompt or syntax error)
    if !shell.is_syntax_complete(&normalized_command) {
        if debug { eprintln!("[termstack] incomplete syntax, exit 3"); }
        std::process::exit(EXIT_INCOMPLETE_SYNTAX);
    }

    // Regular command - spawn terminal in termstack, but GUI windows go to host
    // Only commands with 'gui' prefix should have windows inside termstack
    if debug { eprintln!("[termstack] spawning in terminal (GUI windows go to host)"); }
    spawn_in_terminal(&normalized_command, &prompt)
}

/// Send a resize request to the compositor and wait for acknowledgement
///
/// This is synchronous to prevent race conditions with TUI apps that query
/// terminal size immediately after starting.
fn send_resize_request(mode: &str) -> Result<()> {
    use std::io::{BufRead, BufReader};

    let socket_path = env::var("TERMSTACK_SOCKET")
        .context("TERMSTACK_SOCKET not set - are you running inside termstack?")?;

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

/// Send a builtin command notification to the compositor
///
/// This creates a persistent entry in the stack showing the builtin command
/// and its output (if any). Called by the shell integration after executing
/// a builtin command like cd, export, alias, etc.
///
/// Usage: termstack --builtin "prompt" "command" "output" [--error]
fn send_builtin_notification(args: &[String]) -> Result<()> {
    let debug = debug_enabled();

    // Parse arguments: prompt command result [--error]
    let prompt = args.first()
        .context("missing prompt argument for --builtin")?;
    let command = args.get(1).cloned().unwrap_or_default();
    let result = args.get(2).cloned().unwrap_or_default();
    let success = !args.iter().any(|a| a == "--error");

    if debug {
        eprintln!("[termstack] builtin: prompt={:?} command={:?} result={:?} success={}", prompt, command, result, success);
    }

    // Get socket path from environment
    let socket_path = env::var("TERMSTACK_SOCKET")
        .context("TERMSTACK_SOCKET not set - are you running inside termstack?")?;

    // Build JSON message
    let msg = serde_json::json!({
        "type": "builtin",
        "prompt": prompt,
        "command": command,
        "result": result,
        "success": success,
    });

    if debug { eprintln!("[termstack] connecting to socket for builtin..."); }

    // Connect to compositor and send message
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;

    writeln!(stream, "{}", msg).context("failed to send builtin message")?;
    stream.flush().context("failed to flush builtin message")?;

    if debug { eprintln!("[termstack] builtin message sent successfully"); }

    Ok(())
}

/// Query current window state from the compositor
///
/// Outputs JSON array of window information. Useful for testing and debugging.
fn query_windows() -> Result<()> {
    use std::io::{BufRead, BufReader};

    let socket_path = env::var("TERMSTACK_SOCKET")
        .context("TERMSTACK_SOCKET not set - are you running inside termstack?")?;

    let msg = serde_json::json!({
        "type": "query_windows",
    });

    let stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;

    // Set timeout for reading response
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .context("failed to set read timeout")?;

    let mut stream_write = stream.try_clone().context("failed to clone stream")?;

    writeln!(stream_write, "{}", msg).context("failed to send query_windows message")?;
    stream_write.flush().context("failed to flush query_windows message")?;

    // Read JSON response
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).context("failed to read query_windows response")?;

    // Output the JSON response directly
    print!("{}", response);

    Ok(())
}

/// Spawn command in a new termstack terminal
///
/// The terminal starts small and grows with content. TUI apps are
/// auto-detected via alternate screen mode and resized to full viewport.
fn spawn_in_terminal(command: &str, prompt: &str) -> Result<()> {
    let debug = debug_enabled();

    // Get socket path from environment
    let socket_path = env::var("TERMSTACK_SOCKET")
        .context("TERMSTACK_SOCKET not set - are you running inside termstack?")?;
    if debug { eprintln!("[termstack] socket path: {}", socket_path); }

    // Collect current environment
    let env_vars: std::collections::HashMap<String, String> = env::vars().collect();

    // Get current working directory
    let cwd = env::current_dir()
        .context("failed to get current directory")?
        .to_string_lossy()
        .to_string();
    if debug { eprintln!("[termstack] cwd: {}", cwd); }

    // Build JSON message
    let msg = serde_json::json!({
        "type": "spawn",
        "prompt": prompt,
        "command": command,
        "cwd": cwd,
        "env": env_vars,
    });

    if debug { eprintln!("[termstack] connecting to socket..."); }
    // Connect to compositor and send message
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;
    if debug { eprintln!("[termstack] connected, sending message..."); }

    writeln!(stream, "{}", msg).context("failed to send message")?;
    stream.flush().context("failed to flush message")?;
    if debug { eprintln!("[termstack] message sent successfully"); }

    // Clear the command line from the invoking terminal
    if !command.is_empty() {
        print!("\x1b[A\x1b[2K");
        std::io::stdout().flush().ok();
    }

    Ok(())
}

/// Spawn a GUI app with foreground/background mode
///
/// In foreground mode, the launching terminal is hidden until the GUI app exits.
/// In background mode, the launching terminal stays visible and usable.
fn spawn_gui_app(command: &str, foreground: bool) -> Result<()> {
    let debug = debug_enabled();

    // Get socket path from environment
    let socket_path = env::var("TERMSTACK_SOCKET")
        .context("TERMSTACK_SOCKET not set - are you running inside termstack?")?;
    if debug { eprintln!("[termstack] socket path: {}", socket_path); }

    // Collect current environment
    let env_vars: std::collections::HashMap<String, String> = env::vars().collect();

    // Get current working directory
    let cwd = env::current_dir()
        .context("failed to get current directory")?
        .to_string_lossy()
        .to_string();
    if debug { eprintln!("[termstack] cwd: {}", cwd); }

    // Build JSON message for GUI spawn (unified with terminal spawn)
    let msg = serde_json::json!({
        "type": "spawn",
        "command": command,
        "cwd": cwd,
        "env": env_vars,
        "foreground": foreground,
    });

    if debug { eprintln!("[termstack] connecting to socket for GUI spawn..."); }
    // Connect to compositor and send message
    let mut stream = UnixStream::connect(&socket_path)
        .with_context(|| format!("failed to connect to {}", socket_path))?;
    if debug { eprintln!("[termstack] connected, sending gui_spawn message..."); }

    writeln!(stream, "{}", msg).context("failed to send message")?;
    stream.flush().context("failed to flush message")?;
    if debug { eprintln!("[termstack] gui_spawn message sent successfully"); }

    // Clear the command line from the invoking terminal
    print!("\x1b[A\x1b[2K");
    std::io::stdout().flush().ok();

    Ok(())
}

/// Extract termstack subcommand from a command string
///
/// When fish integration intercepts "termstack test-x11", it calls
/// "termstack -c 'termstack test-x11'". We detect this and extract
/// the subcommand args to execute directly (avoiding PATH lookup issues).
///
/// Returns the full subcommand string (e.g., "test-x11" or "gui firefox")
fn extract_termstack_subcommand(command: &str) -> Option<String> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    // Check if first word is "termstack" (or path ending in termstack)
    let first = parts[0];
    let is_termstack = first == "termstack"
        || first.ends_with("/termstack")
        || first == "$TERMSTACK_BIN";

    if !is_termstack {
        return None;
    }

    // Check if second word is a known subcommand
    if parts.len() < 2 {
        return None;
    }

    let subcommands = ["diagnose", "test-x11", "query-windows", "gui", "--status", "--resize", "--builtin", "--help", "-h"];
    if subcommands.contains(&parts[1]) {
        // Return everything after "termstack"
        Some(parts[1..].join(" "))
    } else {
        None
    }
}

/// Execute a termstack subcommand directly
fn execute_subcommand(subcommand: &str) -> Result<()> {
    let parts: Vec<&str> = subcommand.split_whitespace().collect();
    if parts.is_empty() {
        bail!("empty subcommand");
    }

    match parts[0] {
        "diagnose" => run_diagnostics(),
        "test-x11" => test_x11_connectivity(),
        "query-windows" => query_windows(),
        "--status" => {
            // Inline the status output
            let socket = env::var("TERMSTACK_SOCKET");
            let shell = env::var("SHELL").unwrap_or_else(|_| "(not set)".to_string());

            println!("termstack status:");
            println!("  TERMSTACK_SOCKET: {}", match &socket {
                Ok(path) => format!("{} (exists: {})", path, std::path::Path::new(path).exists()),
                Err(_) => "NOT SET - shell integration will not activate".to_string(),
            });
            println!("  SHELL: {}", shell);
            println!();

            if socket.is_ok() {
                println!("Shell integration should be active.");
            } else {
                println!("You are NOT inside termstack.");
            }
            Ok(())
        }
        "--resize" => {
            let mode = parts.get(1).copied().unwrap_or("full");
            send_resize_request(mode)
        }
        "--builtin" => {
            // Convert &str parts back to String for send_builtin_notification
            let builtin_args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            send_builtin_notification(&builtin_args)
        }
        "gui" => {
            if parts.len() < 2 {
                bail!("usage: termstack gui <command>");
            }
            let gui_command = parts[1..].join(" ");
            let foreground = env::var("TERMSTACK_GUI_BACKGROUND").is_err();
            spawn_gui_app(&gui_command, foreground)
        }
        "--help" | "-h" => {
            println!("termstack - Terminal compositor CLI");
            println!();
            println!("Subcommands:");
            println!("  diagnose       Run X11/Wayland diagnostics");
            println!("  test-x11       Test X11 connectivity");
            println!("  query-windows  Query current window state (JSON output)");
            println!("  gui <cmd>      Launch GUI app inside termstack");
            println!("  --status       Show termstack status");
            println!("  --resize       Resize focused terminal");
            Ok(())
        }
        _ => bail!("unknown subcommand: {}", parts[0]),
    }
}

/// Parse command from arguments
///
/// Supports:
/// - `termstack -c "command"` - run command
/// - `termstack` - run interactive shell (empty command)
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

    bail!("usage: termstack [-c command]");
}

/// Run X11/Wayland diagnostics to help debug GUI app issues
fn run_diagnostics() -> Result<()> {
    use std::process::Command;

    println!("=== TermStack Diagnostics ===\n");

    // Check if we're inside termstack
    let socket = env::var("TERMSTACK_SOCKET");
    println!("Environment:");
    println!("  TERMSTACK_SOCKET: {}", match &socket {
        Ok(path) => {
            let exists = std::path::Path::new(path).exists();
            format!("{} (exists: {})", path, exists)
        }
        Err(_) => "NOT SET (not inside termstack)".to_string(),
    });

    // X11 diagnostics
    println!("\nX11:");
    let display = env::var("DISPLAY");
    println!("  DISPLAY: {}", match &display {
        Ok(d) => d.clone(),
        Err(_) => "NOT SET".to_string(),
    });

    let xauthority = env::var("XAUTHORITY");
    println!("  XAUTHORITY: {}", match &xauthority {
        Ok(path) => {
            let exists = std::path::Path::new(path).exists();
            if exists {
                format!("{} (OK)", path)
            } else {
                format!("{} (WARNING: file does not exist)", path)
            }
        }
        Err(_) => "<not set> (WARNING: GTK apps may fail)".to_string(),
    });

    // Test X11 connection with xdpyinfo
    if display.is_ok() {
        print!("  X server test: ");
        let mut cmd = Command::new("xdpyinfo");
        // Use XAUTHORITY if set, otherwise try without
        if let Ok(ref xa) = xauthority {
            cmd.env("XAUTHORITY", xa);
        }
        match cmd
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(status) if status.success() => println!("OK"),
            Ok(status) => println!("FAILED (exit code: {:?})", status.code()),
            Err(e) => println!("FAILED ({})", e),
        }
    }

    // Check for xwayland-satellite
    print!("  xwayland-satellite: ");
    match Command::new("pgrep")
        .args(["-x", "xwayland-satel"]) // pgrep truncates to 15 chars
        .output()
    {
        Ok(output) if output.status.success() => {
            let pids = String::from_utf8_lossy(&output.stdout);
            let pid = pids.trim().lines().next().unwrap_or("?");
            println!("running (PID {})", pid);
        }
        _ => println!("not running"),
    }

    // Wayland diagnostics
    println!("\nWayland:");
    let wayland_display = env::var("WAYLAND_DISPLAY");
    println!("  WAYLAND_DISPLAY: {}", match &wayland_display {
        Ok(d) => d.clone(),
        Err(_) => "NOT SET".to_string(),
    });

    if let Ok(ref wd) = wayland_display {
        let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run/user/1000".to_string());
        let socket_path = format!("{}/{}", runtime_dir, wd);
        let exists = std::path::Path::new(&socket_path).exists();
        println!("  Socket: {} (exists: {})", socket_path, exists);
    }

    // Summary
    println!("\n=== Summary ===");
    if socket.is_err() {
        println!("You are NOT inside termstack. Start the compositor first.");
    } else if display.is_err() {
        println!("DISPLAY not set. XWayland may not have started properly.");
    } else if xauthority.is_err() {
        println!("WARNING: XAUTHORITY not set. GTK X11 apps may fail.");
        println!("The compositor should create an xauth file on startup.");
    } else {
        println!("Configuration looks correct for X11 GUI apps.");
    }

    Ok(())
}

/// Systematically test X11 connectivity with detailed diagnostics
fn test_x11_connectivity() -> Result<()> {
    use std::process::{Command, Stdio};

    println!("=== X11 Connectivity Test ===\n");

    // Step 1: Check environment
    let display = env::var("DISPLAY");
    let xauthority = env::var("XAUTHORITY");

    println!("Step 1: Environment");
    println!("  DISPLAY: {:?}", display);
    println!("  XAUTHORITY: {:?}", xauthority);

    let display = match display {
        Ok(d) => d,
        Err(_) => {
            println!("\nFAILED: DISPLAY not set. Are you inside termstack?");
            return Ok(());
        }
    };

    // Step 2: Check xauth file
    println!("\nStep 2: X Authority File");
    if let Ok(ref path) = xauthority {
        let exists = std::path::Path::new(path).exists();
        println!("  File exists: {}", exists);
        if exists {
            // List entries
            let output = Command::new("xauth")
                .args(["-f", path, "list"])
                .output();
            if let Ok(out) = output {
                let entries = String::from_utf8_lossy(&out.stdout);
                for line in entries.lines() {
                    println!("    {}", line);
                }
            }
        } else {
            println!("  WARNING: xauth file does not exist!");
        }
    } else {
        println!("  No XAUTHORITY set");
    }

    // Step 3: Test X11 connection with different auth scenarios
    println!("\nStep 3: X11 Connection Tests");

    // Test 3a: With current XAUTHORITY
    print!("  3a. xdpyinfo with current env: ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = Command::new("xdpyinfo")
        .env("DISPLAY", &display)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match result {
        Ok(out) if out.status.success() => println!("OK"),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!("FAILED: {}", stderr.lines().next().unwrap_or("unknown error"));
        }
        Err(e) => println!("FAILED: {}", e),
    }

    // Test 3b: Without XAUTHORITY
    print!("  3b. xdpyinfo without XAUTHORITY: ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = Command::new("xdpyinfo")
        .env("DISPLAY", &display)
        .env_remove("XAUTHORITY")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match result {
        Ok(out) if out.status.success() => println!("OK"),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            println!("FAILED: {}", stderr.lines().next().unwrap_or("unknown error"));
        }
        Err(e) => println!("FAILED: {}", e),
    }

    // Step 4: Test xeyes (simple X11 app)
    println!("\nStep 4: Simple X11 App Test (xeyes)");
    print!("  Spawning xeyes for 2 seconds: ");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let result = Command::new("timeout")
        .args(["2", "xeyes"])
        .env("DISPLAY", &display)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status();
    match result {
        Ok(status) if status.code() == Some(124) => println!("OK (killed after timeout)"),
        Ok(status) if status.success() => println!("OK"),
        Ok(status) => println!("FAILED (exit code: {:?})", status.code()),
        Err(e) => println!("FAILED: {}", e),
    }

    // Step 5: Test surf specifically
    println!("\nStep 5: Surf Browser Test");
    let has_surf = Command::new("which")
        .arg("surf")
        .stdout(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if has_surf {
        print!("  Testing surf with current env: ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let result = Command::new("timeout")
            .args(["3", "surf", "about:blank"])
            .env("DISPLAY", &display)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match result {
            Ok(out) if out.status.code() == Some(124) => println!("OK (killed after timeout)"),
            Ok(out) if out.status.success() => println!("OK"),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                println!("FAILED: {}", first_line);
            }
            Err(e) => println!("FAILED: {}", e),
        }

        print!("  Testing surf without XAUTHORITY: ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let result = Command::new("timeout")
            .args(["3", "surf", "about:blank"])
            .env("DISPLAY", &display)
            .env_remove("XAUTHORITY")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match result {
            Ok(out) if out.status.code() == Some(124) => println!("OK (killed after timeout)"),
            Ok(out) if out.status.success() => println!("OK"),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                println!("FAILED: {}", first_line);
            }
            Err(e) => println!("FAILED: {}", e),
        }
        // Test with explicit GDK_BACKEND
        print!("  Testing surf with GDK_BACKEND=wayland: ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let wayland_display = env::var("WAYLAND_DISPLAY").unwrap_or_default();
        let xdg_runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_default();
        let result = Command::new("timeout")
            .args(["3", "surf", "about:blank"])
            .env("WAYLAND_DISPLAY", &wayland_display)
            .env("XDG_RUNTIME_DIR", &xdg_runtime)
            .env("GDK_BACKEND", "wayland")
            .env_remove("DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match result {
            Ok(out) if out.status.code() == Some(124) => println!("OK (killed after timeout)"),
            Ok(out) if out.status.success() => println!("OK"),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                println!("FAILED: {}", first_line);
            }
            Err(e) => println!("FAILED: {}", e),
        }

        print!("  Testing surf with GDK_BACKEND=x11: ");
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let result = Command::new("timeout")
            .args(["3", "surf", "about:blank"])
            .env("DISPLAY", &display)
            .env("GDK_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match result {
            Ok(out) if out.status.code() == Some(124) => println!("OK (killed after timeout)"),
            Ok(out) if out.status.success() => println!("OK"),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                println!("FAILED: {}", first_line);
            }
            Err(e) => println!("FAILED: {}", e),
        }
    } else {
        println!("  surf not found");
    }

    // Step 6: Wayland environment
    println!("\nStep 6: Wayland Environment");
    let wayland_display = env::var("WAYLAND_DISPLAY");
    let xdg_runtime = env::var("XDG_RUNTIME_DIR");
    let gdk_backend = env::var("GDK_BACKEND");
    println!("  WAYLAND_DISPLAY: {:?}", wayland_display);
    println!("  XDG_RUNTIME_DIR: {:?}", xdg_runtime);
    println!("  GDK_BACKEND: {:?}", gdk_backend);

    if let (Ok(wd), Ok(xdg)) = (&wayland_display, &xdg_runtime) {
        let socket_path = format!("{}/{}", xdg, wd);
        let socket_exists = std::path::Path::new(&socket_path).exists();
        println!("  Wayland socket exists: {} ({})", socket_exists, socket_path);
    }

    println!("\n=== Test Complete ===");
    Ok(())
}
