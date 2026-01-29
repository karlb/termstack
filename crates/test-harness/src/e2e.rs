//! E2E test infrastructure using the real HeadlessBackend
//!
//! This module provides utilities for running E2E tests that:
//! - Run without a display (no windows popping up, no input capture)
//! - Test real terminals with real shell commands
//! - Use the actual HeadlessBackend for event injection
//!
//! # Example
//!
//! ```ignore
//! use test_harness::e2e::E2ETestHarness;
//!
//! #[test]
//! fn test_terminal_spawns() {
//!     let mut harness = E2ETestHarness::new(1280, 800);
//!     harness.inject_key(KEY_ENTER, KeyState::Pressed);
//!     harness.run_frames(10);
//!     assert_eq!(harness.window_count(), 1);
//! }
//! ```

#[cfg(feature = "headless-backend")]
pub use headless_harness::*;

#[cfg(feature = "headless-backend")]
mod headless_harness {
    use compositor::backend::headless::{HeadlessBackend, HeadlessEvent, HeadlessInputBackend};
    use smithay::backend::input::{ButtonState, InputEvent, KeyState};

    /// E2E test harness using the real HeadlessBackend
    ///
    /// This provides a high-level API for E2E tests that need to:
    /// - Inject input events (keyboard, mouse, scroll)
    /// - Run compositor frame loops
    /// - Inspect framebuffer output
    pub struct E2ETestHarness {
        backend: HeadlessBackend,
    }

    impl E2ETestHarness {
        /// Create a new E2E test harness with the given output size
        pub fn new(width: u32, height: u32) -> Self {
            Self {
                backend: HeadlessBackend::new_with_size(width, height),
            }
        }

        /// Inject a keyboard key event
        ///
        /// Key codes are evdev codes (e.g., KEY_A = 30, KEY_ENTER = 28).
        pub fn inject_key(&mut self, key: u32, state: KeyState) {
            self.backend.inject_key(key, state);
        }

        /// Inject a key press (press + release)
        pub fn press_key(&mut self, key: u32) {
            self.backend.inject_key(key, KeyState::Pressed);
            self.backend.inject_key(key, KeyState::Released);
        }

        /// Type a string by injecting key events
        ///
        /// Note: Only supports basic ASCII. For complex input, use inject_key directly.
        pub fn type_string(&mut self, s: &str) {
            for c in s.chars() {
                if let Some(key) = char_to_keycode(c) {
                    self.press_key(key);
                }
            }
        }

        /// Inject a pointer motion event
        ///
        /// Coordinates are normalized 0.0-1.0 (relative to output size).
        pub fn inject_pointer_motion(&mut self, x: f64, y: f64) {
            self.backend.inject_pointer_motion(x, y);
        }

        /// Inject a pointer button event
        ///
        /// Button codes: BTN_LEFT = 0x110, BTN_RIGHT = 0x111, BTN_MIDDLE = 0x112
        pub fn inject_pointer_button(&mut self, button: u32, state: ButtonState) {
            self.backend.inject_pointer_button(button, state);
        }

        /// Inject a click (press + release)
        pub fn click(&mut self, x: f64, y: f64, button: u32) {
            self.inject_pointer_motion(x, y);
            self.inject_pointer_button(button, ButtonState::Pressed);
            self.inject_pointer_button(button, ButtonState::Released);
        }

        /// Inject a left click
        pub fn left_click(&mut self, x: f64, y: f64) {
            self.click(x, y, 0x110); // BTN_LEFT
        }

        /// Inject a scroll event
        pub fn inject_scroll(&mut self, horizontal: f64, vertical: f64) {
            self.backend.inject_scroll(horizontal, vertical);
        }

        /// Resize the virtual display
        pub fn resize(&mut self, width: u32, height: u32) {
            self.backend.resize(width, height);
        }

        /// Poll pending events from the backend
        ///
        /// Returns events that can be processed by the compositor.
        pub fn poll_events(&mut self) -> Vec<HeadlessEvent> {
            self.backend.poll_events()
        }

        /// Convert a HeadlessEvent to an InputEvent for processing
        pub fn to_input_event(
            &self,
            event: &HeadlessEvent,
        ) -> Option<InputEvent<HeadlessInputBackend>> {
            self.backend.to_input_event(event)
        }

        /// Get the current framebuffer
        ///
        /// Returns a slice of RGBA pixels (u32 per pixel).
        pub fn framebuffer(&self) -> &[u32] {
            self.backend.framebuffer()
        }

        /// Get framebuffer dimensions
        pub fn framebuffer_size(&self) -> (u32, u32) {
            self.backend.framebuffer_size()
        }

        /// Get a mutable reference to the underlying backend
        pub fn backend_mut(&mut self) -> &mut HeadlessBackend {
            &mut self.backend
        }

        /// Get a reference to the underlying backend
        pub fn backend(&self) -> &HeadlessBackend {
            &self.backend
        }
    }

    /// Convert a character to an evdev keycode
    ///
    /// Only supports basic ASCII characters. Returns None for unsupported chars.
    fn char_to_keycode(c: char) -> Option<u32> {
        // evdev key codes (from linux/input-event-codes.h)
        match c {
            'a' | 'A' => Some(30),  // KEY_A
            'b' | 'B' => Some(48),  // KEY_B
            'c' | 'C' => Some(46),  // KEY_C
            'd' | 'D' => Some(32),  // KEY_D
            'e' | 'E' => Some(18),  // KEY_E
            'f' | 'F' => Some(33),  // KEY_F
            'g' | 'G' => Some(34),  // KEY_G
            'h' | 'H' => Some(35),  // KEY_H
            'i' | 'I' => Some(23),  // KEY_I
            'j' | 'J' => Some(36),  // KEY_J
            'k' | 'K' => Some(37),  // KEY_K
            'l' | 'L' => Some(38),  // KEY_L
            'm' | 'M' => Some(50),  // KEY_M
            'n' | 'N' => Some(49),  // KEY_N
            'o' | 'O' => Some(24),  // KEY_O
            'p' | 'P' => Some(25),  // KEY_P
            'q' | 'Q' => Some(16),  // KEY_Q
            'r' | 'R' => Some(19),  // KEY_R
            's' | 'S' => Some(31),  // KEY_S
            't' | 'T' => Some(20),  // KEY_T
            'u' | 'U' => Some(22),  // KEY_U
            'v' | 'V' => Some(47),  // KEY_V
            'w' | 'W' => Some(17),  // KEY_W
            'x' | 'X' => Some(45),  // KEY_X
            'y' | 'Y' => Some(21),  // KEY_Y
            'z' | 'Z' => Some(44),  // KEY_Z
            '0' => Some(11),        // KEY_0
            '1' => Some(2),         // KEY_1
            '2' => Some(3),         // KEY_2
            '3' => Some(4),         // KEY_3
            '4' => Some(5),         // KEY_4
            '5' => Some(6),         // KEY_5
            '6' => Some(7),         // KEY_6
            '7' => Some(8),         // KEY_7
            '8' => Some(9),         // KEY_8
            '9' => Some(10),        // KEY_9
            ' ' => Some(57),        // KEY_SPACE
            '\n' => Some(28),       // KEY_ENTER
            '\t' => Some(15),       // KEY_TAB
            '-' => Some(12),        // KEY_MINUS
            '=' => Some(13),        // KEY_EQUAL
            '[' => Some(26),        // KEY_LEFTBRACE
            ']' => Some(27),        // KEY_RIGHTBRACE
            ';' => Some(39),        // KEY_SEMICOLON
            '\'' => Some(40),       // KEY_APOSTROPHE
            '`' => Some(41),        // KEY_GRAVE
            '\\' => Some(43),       // KEY_BACKSLASH
            ',' => Some(51),        // KEY_COMMA
            '.' => Some(52),        // KEY_DOT
            '/' => Some(53),        // KEY_SLASH
            _ => None,
        }
    }

    /// Common evdev key codes for tests
    pub mod keycodes {
        pub const KEY_ESC: u32 = 1;
        pub const KEY_1: u32 = 2;
        pub const KEY_2: u32 = 3;
        pub const KEY_3: u32 = 4;
        pub const KEY_4: u32 = 5;
        pub const KEY_5: u32 = 6;
        pub const KEY_6: u32 = 7;
        pub const KEY_7: u32 = 8;
        pub const KEY_8: u32 = 9;
        pub const KEY_9: u32 = 10;
        pub const KEY_0: u32 = 11;
        pub const KEY_MINUS: u32 = 12;
        pub const KEY_EQUAL: u32 = 13;
        pub const KEY_BACKSPACE: u32 = 14;
        pub const KEY_TAB: u32 = 15;
        pub const KEY_Q: u32 = 16;
        pub const KEY_W: u32 = 17;
        pub const KEY_E: u32 = 18;
        pub const KEY_R: u32 = 19;
        pub const KEY_T: u32 = 20;
        pub const KEY_Y: u32 = 21;
        pub const KEY_U: u32 = 22;
        pub const KEY_I: u32 = 23;
        pub const KEY_O: u32 = 24;
        pub const KEY_P: u32 = 25;
        pub const KEY_ENTER: u32 = 28;
        pub const KEY_LEFTCTRL: u32 = 29;
        pub const KEY_A: u32 = 30;
        pub const KEY_S: u32 = 31;
        pub const KEY_D: u32 = 32;
        pub const KEY_F: u32 = 33;
        pub const KEY_G: u32 = 34;
        pub const KEY_H: u32 = 35;
        pub const KEY_J: u32 = 36;
        pub const KEY_K: u32 = 37;
        pub const KEY_L: u32 = 38;
        pub const KEY_LEFTSHIFT: u32 = 42;
        pub const KEY_Z: u32 = 44;
        pub const KEY_X: u32 = 45;
        pub const KEY_C: u32 = 46;
        pub const KEY_V: u32 = 47;
        pub const KEY_B: u32 = 48;
        pub const KEY_N: u32 = 49;
        pub const KEY_M: u32 = 50;
        pub const KEY_SPACE: u32 = 57;
        pub const KEY_F1: u32 = 59;
        pub const KEY_F2: u32 = 60;
        pub const KEY_F3: u32 = 61;
        pub const KEY_F4: u32 = 62;
        pub const KEY_F5: u32 = 63;
        pub const KEY_F6: u32 = 64;
        pub const KEY_F7: u32 = 65;
        pub const KEY_F8: u32 = 66;
        pub const KEY_F9: u32 = 67;
        pub const KEY_F10: u32 = 68;
        pub const KEY_F11: u32 = 87;
        pub const KEY_F12: u32 = 88;
        pub const KEY_UP: u32 = 103;
        pub const KEY_LEFT: u32 = 105;
        pub const KEY_RIGHT: u32 = 106;
        pub const KEY_DOWN: u32 = 108;

        // Mouse buttons
        pub const BTN_LEFT: u32 = 0x110;
        pub const BTN_RIGHT: u32 = 0x111;
        pub const BTN_MIDDLE: u32 = 0x112;
    }

    /// Assertion helpers for E2E tests
    pub mod assertions {
        use super::E2ETestHarness;

        /// Assert that the framebuffer is not all black (contains some non-zero pixels)
        pub fn assert_framebuffer_not_empty(harness: &E2ETestHarness) {
            let fb = harness.framebuffer();
            let non_black = fb.iter().any(|&pixel| pixel != 0);
            assert!(non_black, "Framebuffer is completely black (empty)");
        }

        /// Assert that a specific region of the framebuffer contains non-black pixels
        pub fn assert_region_not_empty(
            harness: &E2ETestHarness,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
        ) {
            let (fb_width, _fb_height) = harness.framebuffer_size();
            let fb = harness.framebuffer();

            for row in y..(y + height) {
                for col in x..(x + width) {
                    let idx = (row * fb_width + col) as usize;
                    if fb.get(idx).copied().unwrap_or(0) != 0 {
                        return; // Found a non-black pixel
                    }
                }
            }
            panic!(
                "Region ({}, {}) {}x{} is completely black",
                x, y, width, height
            );
        }
    }
}
