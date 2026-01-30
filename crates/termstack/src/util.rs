//! Shared utility functions for termstack CLI

use std::env;

/// Check if debug mode is enabled via DEBUG_TSTACK environment variable
pub fn debug_enabled() -> bool {
    // Cache result to avoid repeated env lookups (inline const fn not stable yet)
    use std::sync::OnceLock;
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| env::var("DEBUG_TSTACK").is_ok())
}
