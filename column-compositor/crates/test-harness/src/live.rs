//! Live compositor testing infrastructure
//!
//! This module provides helpers for running tests against a real compositor
//! with actual Wayland clients (like foot terminal).
//!
//! # Requirements
//!
//! These tests require a display server (X11 via DISPLAY or Wayland via WAYLAND_DISPLAY).
//! They are marked `#[ignore]` by default and can be run with:
//!
//! ```bash
//! cargo test -- --ignored
//! ```
//!
//! # Display Detection
//!
//! Use `display_available()` to check if tests can run:
//!
//! ```ignore
//! #[test]
//! #[ignore = "requires display"]
//! fn my_live_test() {
//!     if !live::display_available() {
//!         eprintln!("Skipping: no display available");
//!         return;
//!     }
//!     // ... test code
//! }
//! ```

use std::env;

/// Check if a display is available for testing
///
/// Returns true if either DISPLAY (X11) or WAYLAND_DISPLAY is set.
/// This is used to determine if live compositor tests can run.
pub fn display_available() -> bool {
    env::var("DISPLAY").is_ok() || env::var("WAYLAND_DISPLAY").is_ok()
}

/// Check if the X11 backend should be preferred
///
/// Returns true if DISPLAY is set. This is useful because the winit
/// backend works more reliably with X11 for testing.
pub fn prefer_x11() -> bool {
    env::var("DISPLAY").is_ok()
}

/// Get the display type that's available
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayType {
    X11,
    Wayland,
    None,
}

/// Detect which display type is available
pub fn detect_display() -> DisplayType {
    if env::var("DISPLAY").is_ok() {
        DisplayType::X11
    } else if env::var("WAYLAND_DISPLAY").is_ok() {
        DisplayType::Wayland
    } else {
        DisplayType::None
    }
}

/// Environment configuration for running the compositor in tests
pub struct TestEnvironment {
    /// Original environment variables to restore after test
    _saved_env: Vec<(String, Option<String>)>,
}

impl TestEnvironment {
    /// Create a new test environment
    ///
    /// Sets up environment variables for running the compositor.
    /// Variables are restored when the TestEnvironment is dropped.
    pub fn new() -> Self {
        let mut saved = Vec::new();

        // Save and set WINIT_UNIX_BACKEND to x11 if DISPLAY is available
        // This makes winit use X11 which works better for testing
        if prefer_x11() {
            let key = "WINIT_UNIX_BACKEND";
            saved.push((key.to_string(), env::var(key).ok()));
            env::set_var(key, "x11");
        }

        Self { _saved_env: saved }
    }
}

impl Default for TestEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        // Restore saved environment variables
        for (key, value) in &self._saved_env {
            match value {
                Some(v) => env::set_var(key, v),
                None => env::remove_var(key),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_display_works() {
        // Just verify this doesn't panic
        let display = detect_display();
        match display {
            DisplayType::X11 => println!("X11 display detected"),
            DisplayType::Wayland => println!("Wayland display detected"),
            DisplayType::None => println!("No display detected"),
        }
    }
}
