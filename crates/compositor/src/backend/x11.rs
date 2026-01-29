//! X11 backend implementation
//!
//! This module provides utilities for the X11 backend.
//! The actual X11 initialization remains in compositor_main.rs for now,
//! as the Smithay API is tightly coupled to specific renderer types.
//!
//! # Future Work
//!
//! As the backend abstraction matures, more X11-specific code can be
//! moved here to provide a cleaner separation.

use smithay::backend::x11::{X11Event, X11Input};

use super::BackendEvent;

/// Convert X11Event to BackendEvent
pub fn convert_x11_event(event: X11Event) -> Option<BackendEvent<X11Input>> {
    match event {
        X11Event::Input { event, .. } => Some(BackendEvent::Input(event)),
        X11Event::Resized { new_size, .. } => Some(BackendEvent::Resized {
            width: new_size.w,
            height: new_size.h,
        }),
        X11Event::CloseRequested { .. } => Some(BackendEvent::CloseRequested),
        X11Event::Focus { focused, .. } => Some(BackendEvent::Focus { focused }),
        X11Event::Refresh { .. } => Some(BackendEvent::Refresh),
        X11Event::PresentCompleted { .. } => Some(BackendEvent::PresentCompleted),
    }
}
