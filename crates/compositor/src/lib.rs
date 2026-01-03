//! Column Compositor library
//!
//! This library exposes the compositor modules for testing.

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

#[cfg(test)]
mod ipc_test;
