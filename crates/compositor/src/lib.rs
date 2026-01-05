//! Column Compositor library
//!
//! This library exposes the compositor modules for testing and the main
//! compositor entry point for the unified binary.

pub mod config;
pub mod coords;
pub mod cursor;
pub mod input;
pub mod ipc;
pub mod layout;
pub mod render;
pub mod state;
pub mod terminal_manager;
pub mod title_bar;
pub mod xwayland_lifecycle;

// Re-export main compositor functions for unified binary
mod compositor_main;
pub use compositor_main::{run_compositor, setup_logging};

#[cfg(test)]
mod ipc_test;
