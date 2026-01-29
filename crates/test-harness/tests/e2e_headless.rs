//! E2E tests using the HeadlessBackend
//!
//! These tests run without a display by using the HeadlessBackend for event injection.
//!
//! Run with:
//! ```bash
//! cargo test -p test-harness --features headless-backend --test e2e_headless
//! ```

#![cfg(feature = "headless-backend")]

use smithay::backend::input::KeyState;
use test_harness::e2e::{keycodes, E2ETestHarness};

#[test]
fn harness_creates_successfully() {
    let harness = E2ETestHarness::new(1280, 800);
    assert_eq!(harness.framebuffer_size(), (1280, 800));
}

#[test]
fn harness_can_inject_key_events() {
    let mut harness = E2ETestHarness::new(1280, 800);

    // Inject a key press
    harness.inject_key(keycodes::KEY_A, KeyState::Pressed);
    harness.inject_key(keycodes::KEY_A, KeyState::Released);

    // Poll events and verify they're present
    let events = harness.poll_events();
    assert_eq!(events.len(), 2, "Expected 2 key events (press + release)");
}

#[test]
fn harness_can_inject_pointer_events() {
    let mut harness = E2ETestHarness::new(1280, 800);

    // Inject pointer motion
    harness.inject_pointer_motion(0.5, 0.5);

    // Inject a left click
    harness.left_click(0.5, 0.5);

    // Poll events
    let events = harness.poll_events();
    // Motion + motion from click + button press + button release = 4 events
    assert!(events.len() >= 3, "Expected at least 3 pointer events");
}

#[test]
fn harness_can_inject_scroll_events() {
    let mut harness = E2ETestHarness::new(1280, 800);

    harness.inject_scroll(0.0, 1.0);

    let events = harness.poll_events();
    assert_eq!(events.len(), 1, "Expected 1 scroll event");
}

#[test]
fn harness_can_resize() {
    let mut harness = E2ETestHarness::new(1280, 800);
    assert_eq!(harness.framebuffer_size(), (1280, 800));

    harness.resize(1920, 1080);
    assert_eq!(harness.framebuffer_size(), (1920, 1080));
}

#[test]
fn harness_type_string_generates_key_events() {
    let mut harness = E2ETestHarness::new(1280, 800);

    harness.type_string("abc");

    let events = harness.poll_events();
    // 3 characters * 2 events each (press + release) = 6 events
    assert_eq!(events.len(), 6, "Expected 6 key events for 'abc'");
}

#[test]
fn harness_converts_events_to_input_events() {
    let mut harness = E2ETestHarness::new(1280, 800);

    harness.inject_key(keycodes::KEY_ENTER, KeyState::Pressed);

    let events = harness.poll_events();
    assert_eq!(events.len(), 1);

    let input_event = harness.to_input_event(&events[0]);
    assert!(input_event.is_some(), "Should convert to InputEvent");
}

#[test]
fn framebuffer_starts_empty() {
    let harness = E2ETestHarness::new(1280, 800);
    let fb = harness.framebuffer();

    // All pixels should be zero (black) initially
    assert!(fb.iter().all(|&pixel| pixel == 0), "Framebuffer should start empty");
}
