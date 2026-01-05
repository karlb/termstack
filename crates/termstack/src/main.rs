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
//! The mode is detected automatically based on environment context,
//! providing a seamless user experience with a single binary.

use std::env;

mod cli;
mod shell;

#[cfg(test)]
mod config_test;

fn main() -> anyhow::Result<()> {
    // Smart mode detection based on TERMSTACK_SOCKET environment variable
    if env::var("TERMSTACK_SOCKET").is_ok() {
        // CLI mode - running inside a termstack terminal
        // The socket indicates we're already in a compositor session
        cli::run()
    } else {
        // Compositor mode - start the Wayland compositor
        // This is the main application entry point
        compositor::setup_logging();
        compositor::run_compositor()
    }
}
