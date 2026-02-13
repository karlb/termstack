//! Test harness for termstack
//!
//! Provides infrastructure for testing the compositor in headless mode.
//!
//! # Modules
//!
//! - `headless`: Mock compositor for unit testing (no display required)
//! - `live`: Live compositor testing helpers (requires display, Linux only)
//! - `assertions`: Common test assertions
//! - `fixtures`: Test fixture helpers
//! - `e2e`: E2E test infrastructure using the real HeadlessBackend (Linux only)

pub mod assertions;
pub mod fixtures;
pub mod headless;

#[cfg(target_os = "linux")]
pub mod live;

#[cfg(all(target_os = "linux", feature = "headless-backend"))]
pub mod e2e;

pub use headless::{RenderedElement, TestCompositor};
