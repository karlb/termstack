#![no_main]
//! Fuzz target for IPC message parsing
//!
//! Feeds random bytes as JSON to the IPC message parser to find panics,
//! hangs, or unexpected behavior in deserialization and validation.

use libfuzzer_sys::fuzz_target;

use compositor::ipc::IpcMessage;

fuzz_target!(|data: &[u8]| {
    // Try parsing as JSON IPC message - must never panic
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<IpcMessage>(s);
    }
});
