//! Column Compositor library
//!
//! This library exposes the compositor modules for testing and the main
//! compositor entry point for the unified binary.

pub mod backend;
pub mod config;
pub mod coords;
pub mod cursor;
pub mod icon;
pub mod input;
pub mod input_handler;
pub mod ipc;
pub mod layout;
pub mod render;
pub mod selection;
pub mod spawn_handler;
pub mod state;
pub mod terminal_manager;
pub mod terminal_output;
pub mod title_bar;
pub mod window_height;
pub mod window_lifecycle;
pub mod xwayland_lifecycle;

// Re-export main compositor functions for unified binary
mod compositor_main;
pub use compositor_main::{run_compositor, setup_logging};

#[cfg(test)]
mod ipc_test;
