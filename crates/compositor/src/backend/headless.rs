//! Headless backend for E2E testing
//!
//! This module provides a CPU-based software renderer that runs without a display.
//! It's designed for comprehensive E2E tests that:
//! - Run without a display (no windows popping up, no input capture)
//! - Test real terminals with real shell commands
//! - Verify shell integration behavior
//!
//! # Usage
//!
//! Set `TERMSTACK_BACKEND=headless` to use this backend.
//!
//! # Design
//!
//! The headless backend does NOT implement Smithay's full Renderer trait.
//! Instead, it provides:
//! - Event injection for simulating input
//! - Direct access to terminal pixel buffers
//! - Test utilities for E2E scenarios
//!
//! External Wayland windows are not rendered in headless mode (terminals only).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use smithay::backend::input::{
    Axis, AxisRelativeDirection, AxisSource, ButtonState, Device, DeviceCapability, Event,
    InputBackend, InputEvent, KeyState, Keycode, KeyboardKeyEvent, PointerAxisEvent,
    PointerButtonEvent, PointerMotionAbsoluteEvent, AbsolutePositionEvent, UnusedEvent,
};
use smithay::utils::{Physical, Size};

use super::BackendConfig;

/// Headless backend for testing without a display
///
/// This backend does NOT implement the full CompositorBackend trait because
/// Smithay's Renderer trait is complex and designed for GPU rendering.
/// Instead, it provides direct access to test facilities.
pub struct HeadlessBackend {
    /// Current output size
    output_size: Size<i32, Physical>,
    /// Queued events for injection
    event_queue: Arc<Mutex<VecDeque<HeadlessEvent>>>,
    /// Framebuffer for testing (optional)
    framebuffer: Vec<u32>,
}

/// Events that can be injected into the headless backend
#[derive(Debug, Clone)]
pub enum HeadlessEvent {
    /// Keyboard key event
    Key { key: u32, state: KeyState },
    /// Pointer motion (absolute 0.0-1.0)
    PointerMotion { x: f64, y: f64 },
    /// Pointer button
    PointerButton { button: u32, state: ButtonState },
    /// Scroll
    Scroll { horizontal: f64, vertical: f64 },
    /// Resize
    Resize { width: u16, height: u16 },
    /// Close requested
    CloseRequested,
}

/// Headless input backend for event injection
#[derive(Debug, Clone)]
pub struct HeadlessInputBackend;

/// Headless input device
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct HeadlessDevice {
    name: String,
}

/// Headless keyboard key event
#[derive(Debug, Clone)]
pub struct HeadlessKeyboardEvent {
    /// Time of event in milliseconds
    pub time: u32,
    /// Key code (evdev)
    pub key: u32,
    /// Key state (pressed/released)
    pub state: KeyState,
}

/// Headless pointer motion event
#[derive(Debug, Clone)]
pub struct HeadlessPointerMotionEvent {
    /// Time of event in milliseconds
    pub time: u32,
    /// Absolute X position (0.0 to 1.0)
    pub x: f64,
    /// Absolute Y position (0.0 to 1.0)
    pub y: f64,
    /// Output size for coordinate conversion
    pub output_size: Size<i32, Physical>,
}

/// Headless pointer button event
#[derive(Debug, Clone)]
pub struct HeadlessPointerButtonEvent {
    /// Time of event in milliseconds
    pub time: u32,
    /// Button code
    pub button: u32,
    /// Button state (pressed/released)
    pub state: ButtonState,
}

/// Headless pointer axis (scroll) event
#[derive(Debug, Clone)]
pub struct HeadlessPointerAxisEvent {
    /// Time of event in milliseconds
    pub time: u32,
    /// Horizontal scroll amount
    pub horizontal: f64,
    /// Vertical scroll amount
    pub vertical: f64,
    /// Scroll source
    pub source: AxisSource,
}

// Implement InputBackend trait for HeadlessInputBackend

impl InputBackend for HeadlessInputBackend {
    type Device = HeadlessDevice;
    type KeyboardKeyEvent = HeadlessKeyboardEvent;
    type PointerAxisEvent = HeadlessPointerAxisEvent;
    type PointerButtonEvent = HeadlessPointerButtonEvent;
    type PointerMotionEvent = UnusedEvent;
    type PointerMotionAbsoluteEvent = HeadlessPointerMotionEvent;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

impl Device for HeadlessDevice {
    fn id(&self) -> String {
        "headless-device".to_string()
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        matches!(capability, DeviceCapability::Keyboard | DeviceCapability::Pointer)
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<std::path::PathBuf> {
        None
    }
}

impl Event<HeadlessInputBackend> for HeadlessKeyboardEvent {
    fn time(&self) -> u64 {
        self.time as u64
    }

    fn device(&self) -> HeadlessDevice {
        HeadlessDevice { name: "headless-keyboard".to_string() }
    }
}

impl KeyboardKeyEvent<HeadlessInputBackend> for HeadlessKeyboardEvent {
    fn key_code(&self) -> Keycode {
        Keycode::new(self.key)
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        // For headless testing, we just track this single key
        1
    }
}

impl Event<HeadlessInputBackend> for HeadlessPointerMotionEvent {
    fn time(&self) -> u64 {
        self.time as u64
    }

    fn device(&self) -> HeadlessDevice {
        HeadlessDevice { name: "headless-pointer".to_string() }
    }
}

impl AbsolutePositionEvent<HeadlessInputBackend> for HeadlessPointerMotionEvent {
    fn x(&self) -> f64 {
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        self.x * width as f64
    }

    fn y_transformed(&self, height: i32) -> f64 {
        self.y * height as f64
    }
}

impl PointerMotionAbsoluteEvent<HeadlessInputBackend> for HeadlessPointerMotionEvent {}

impl Event<HeadlessInputBackend> for HeadlessPointerButtonEvent {
    fn time(&self) -> u64 {
        self.time as u64
    }

    fn device(&self) -> HeadlessDevice {
        HeadlessDevice { name: "headless-pointer".to_string() }
    }
}

impl PointerButtonEvent<HeadlessInputBackend> for HeadlessPointerButtonEvent {
    fn button_code(&self) -> u32 {
        self.button
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

impl Event<HeadlessInputBackend> for HeadlessPointerAxisEvent {
    fn time(&self) -> u64 {
        self.time as u64
    }

    fn device(&self) -> HeadlessDevice {
        HeadlessDevice { name: "headless-pointer".to_string() }
    }
}

impl PointerAxisEvent<HeadlessInputBackend> for HeadlessPointerAxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        match axis {
            Axis::Horizontal => Some(self.horizontal),
            Axis::Vertical => Some(self.vertical),
        }
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        // Convert to v120 units (120 units = 1 notch)
        match axis {
            Axis::Horizontal => Some(self.horizontal * 120.0),
            Axis::Vertical => Some(self.vertical * 120.0),
        }
    }

    fn source(&self) -> AxisSource {
        self.source
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

// Implement HeadlessBackend

impl HeadlessBackend {
    /// Create a new headless backend with the given size
    pub fn new_with_size(width: u32, height: u32) -> Self {
        Self {
            output_size: Size::from((width as i32, height as i32)),
            event_queue: Arc::new(Mutex::new(VecDeque::new())),
            framebuffer: vec![0; (width * height) as usize],
        }
    }

    /// Create from config
    pub fn new(config: &BackendConfig) -> Self {
        Self::new_with_size(config.initial_size.0 as u32, config.initial_size.1 as u32)
    }

    /// Get the current output size
    pub fn output_size(&self) -> Size<i32, Physical> {
        self.output_size
    }

    /// Inject an event for processing
    pub fn inject_event(&self, event: HeadlessEvent) {
        self.event_queue.lock().unwrap().push_back(event);
    }

    /// Inject a keyboard key event
    pub fn inject_key(&self, key: u32, state: KeyState) {
        self.inject_event(HeadlessEvent::Key { key, state });
    }

    /// Inject a pointer motion event (absolute coordinates 0.0-1.0)
    pub fn inject_pointer_motion(&self, x: f64, y: f64) {
        self.inject_event(HeadlessEvent::PointerMotion { x, y });
    }

    /// Inject a pointer button event
    pub fn inject_pointer_button(&self, button: u32, state: ButtonState) {
        self.inject_event(HeadlessEvent::PointerButton { button, state });
    }

    /// Inject a scroll event
    pub fn inject_scroll(&self, horizontal: f64, vertical: f64) {
        self.inject_event(HeadlessEvent::Scroll { horizontal, vertical });
    }

    /// Simulate a resize
    pub fn resize(&mut self, width: u32, height: u32) {
        self.output_size = Size::from((width as i32, height as i32));
        self.framebuffer = vec![0; (width * height) as usize];
        self.inject_event(HeadlessEvent::Resize {
            width: width as u16,
            height: height as u16,
        });
    }

    /// Poll for pending events
    pub fn poll_events(&mut self) -> Vec<HeadlessEvent> {
        let mut queue = self.event_queue.lock().unwrap();
        std::mem::take(&mut *queue).into_iter().collect()
    }

    /// Convert HeadlessEvent to InputEvent for processing
    pub fn to_input_event(&self, event: &HeadlessEvent) -> Option<InputEvent<HeadlessInputBackend>> {
        match event {
            HeadlessEvent::Key { key, state } => {
                let ke = HeadlessKeyboardEvent {
                    time: 0,
                    key: *key,
                    state: *state,
                };
                Some(InputEvent::Keyboard { event: ke })
            }
            HeadlessEvent::PointerMotion { x, y } => {
                let pe = HeadlessPointerMotionEvent {
                    time: 0,
                    x: *x,
                    y: *y,
                    output_size: self.output_size,
                };
                Some(InputEvent::PointerMotionAbsolute { event: pe })
            }
            HeadlessEvent::PointerButton { button, state } => {
                let pe = HeadlessPointerButtonEvent {
                    time: 0,
                    button: *button,
                    state: *state,
                };
                Some(InputEvent::PointerButton { event: pe })
            }
            HeadlessEvent::Scroll { horizontal, vertical } => {
                let pe = HeadlessPointerAxisEvent {
                    time: 0,
                    horizontal: *horizontal,
                    vertical: *vertical,
                    source: AxisSource::Wheel,
                };
                Some(InputEvent::PointerAxis { event: pe })
            }
            HeadlessEvent::Resize { .. } | HeadlessEvent::CloseRequested => None,
        }
    }

    /// Get a reference to the framebuffer
    pub fn framebuffer(&self) -> &[u32] {
        &self.framebuffer
    }

    /// Get the framebuffer size
    pub fn framebuffer_size(&self) -> (u32, u32) {
        (self.output_size.w as u32, self.output_size.h as u32)
    }
}
