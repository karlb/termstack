//! Test harness for column-compositor
//!
//! Provides infrastructure for testing the compositor in headless mode.

pub mod assertions;
pub mod fixtures;
pub mod headless;

pub use headless::TestCompositor;
