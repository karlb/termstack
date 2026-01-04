//! Test harness for termstack
//!
//! Provides infrastructure for testing the compositor in headless mode.
//!
//! # Modules
//!
//! - `headless`: Mock compositor for unit testing (no display required)
//! - `live`: Live compositor testing helpers (requires display)
//! - `assertions`: Common test assertions
//! - `fixtures`: Test fixture helpers

pub mod assertions;
pub mod fixtures;
pub mod headless;
pub mod live;

pub use headless::{RenderedElement, TestCompositor};
