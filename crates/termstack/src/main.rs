//! termstack - Unified binary for compositor and CLI
//!
//! This binary operates in two modes based on environment detection:
//!
//! ## Compositor Mode (default)
//! When TERMSTACK_SOCKET is NOT set, starts the Wayland compositor.
//! This is the main application entry point.
//!
//! ```bash
//! termstack          # Starts the compositor
//! ```
//!
//! ## CLI Mode (inside compositor)
//! When TERMSTACK_SOCKET is set (running inside a termstack terminal),
//! acts as a CLI tool for spawning new terminals and GUI apps.
//!
//! ```bash
//! termstack -c "git status"  # Spawn command in new terminal
//! termstack gui pqiv img.png # Launch GUI app
//! termstack --resize full    # Resize focused terminal
//! ```
//!
//! ## Desktop Integration
//! ```bash
//! termstack install    # Install .desktop file and icons for GNOME
//! termstack uninstall  # Remove desktop integration files
//! ```
//!
//! The mode is detected automatically based on environment context,
//! providing a seamless user experience with a single binary.

use std::env;

mod cli;
mod desktop;
mod shell;

#[cfg(test)]
mod config_test;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().collect();

    // Handle install/uninstall commands first (work in any context)
    if args.len() >= 2 {
        match args[1].as_str() {
            "install" => return desktop::install(),
            "uninstall" => return desktop::uninstall(),
            _ => {}
        }
    }

    // Check for CLI-specific subcommands (gui, --resize, etc.)
    // These require TERMSTACK_SOCKET and should error immediately if missing
    let is_cli_command = args.len() >= 2 && matches!(args[1].as_str(), "gui" | "--resize");

    // Smart mode detection based on TERMSTACK_SOCKET environment variable
    if env::var("TERMSTACK_SOCKET").is_ok() {
        // CLI mode - running inside a termstack terminal
        // The socket indicates we're already in a compositor session
        cli::run()
    } else if is_cli_command {
        // CLI command without socket - error immediately instead of launching compositor
        anyhow::bail!(
            "termstack CLI commands require running inside a termstack session.\n\
             The TERMSTACK_SOCKET environment variable is not set.\n\
             Start the compositor first with: termstack"
        )
    } else {
        // Compositor mode - start the Wayland compositor
        // This is the main application entry point
        compositor::setup_logging();
        compositor::run_compositor()
    }
}
