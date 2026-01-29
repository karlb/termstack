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
//! - `e2e`: E2E test infrastructure using the real HeadlessBackend

pub mod assertions;
pub mod fixtures;
pub mod headless;
pub mod live;

#[cfg(feature = "headless-backend")]
pub mod e2e;

pub use headless::{RenderedElement, TestCompositor};
