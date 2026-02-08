#![no_main]
//! Fuzz target for config TOML parsing
//!
//! Feeds random bytes as TOML to the config parser to find panics,
//! hangs, or unexpected behavior in deserialization and validation.

use libfuzzer_sys::fuzz_target;

use compositor::config::Config;

fuzz_target!(|data: &[u8]| {
    // Try parsing as TOML config - must never panic
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(config) = toml::from_str::<Config>(s) {
            // If parsing succeeded, validation must not panic either
            let _ = config.validate();
        }
    }
});
