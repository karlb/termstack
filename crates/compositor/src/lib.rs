//! Column Compositor library
//!
//! This library exposes the compositor modules for testing and the main
//! compositor entry point for the unified binary.

// Cross-platform modules (no Smithay backend/renderer dependencies)
pub mod compositor_actions;
pub mod config;
pub mod coords;
pub mod ipc;
pub mod layout;
pub mod terminal_keys;
pub mod title_bar;

// Cross-platform compositor modules (Smithay wayland_frontend + desktop features)
pub mod frame;
pub mod selection;
pub mod setup;
pub mod spawn_handler;
pub mod state;
pub mod terminal_manager;
pub mod terminal_output;
pub mod window_height;
pub mod window_lifecycle;

// Cross-platform input handling (no Smithay backend dependencies)
pub mod input_handler;

// Linux-only modules (need Smithay backends/renderer/GPU)
#[cfg(target_os = "linux")]
pub mod backend;
#[cfg(target_os = "linux")]
pub mod cursor;
#[cfg(target_os = "linux")]
pub mod icon;
#[cfg(target_os = "linux")]
pub mod input;
#[cfg(target_os = "linux")]
pub mod render;
#[cfg(target_os = "linux")]
pub mod xwayland_lifecycle;

#[cfg(target_os = "linux")]
mod compositor_main;
#[cfg(target_os = "linux")]
pub use compositor_main::run_compositor;

#[cfg(target_os = "macos")]
mod winit_backend;
#[cfg(target_os = "macos")]
pub use winit_backend::run_compositor_winit;

// setup_logging is cross-platform (only uses tracing_subscriber)
pub fn setup_logging() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,smithay=warn"));

    // Respect NO_COLOR environment variable for testing
    let use_ansi = std::env::var("NO_COLOR").is_err();

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(true)
                .with_line_number(true)
                .with_ansi(use_ansi),
        )
        .with(filter)
        .init();
}

#[cfg(all(test, target_os = "linux"))]
mod ipc_test;
