//! Backend abstraction for the compositor
//!
//! This module provides a trait-based abstraction over different rendering backends,
//! enabling both GPU-accelerated (X11/Wayland) and software (headless) rendering.
//!
//! # Backends
//!
//! - **X11Backend** (feature: `x11-backend`): GPU-accelerated rendering using OpenGL/GLES
//!   via DRM/GBM/EGL. Used for normal compositor operation.
//!
//! - **HeadlessBackend** (feature: `headless-backend`): CPU-based software rendering
//!   for headless E2E testing. No display required.
//!
//! # Design
//!
//! The backend trait hierarchy is designed to:
//! 1. Abstract over different rendering pipelines (GPU vs CPU)
//! 2. Abstract over different event sources (X11 events vs injected test events)
//! 3. Allow the main compositor loop to be generic over the backend
//!
//! Key types:
//! - [`CompositorBackend`]: Main trait for backend lifecycle and access
//! - [`BackendEvent`]: Unified event type from any backend
//! - [`BackendConfig`]: Configuration for backend initialization

#[cfg(feature = "x11-backend")]
pub mod x11;

#[cfg(feature = "headless-backend")]
pub mod headless;

use smithay::backend::input::{InputBackend, InputEvent};
use smithay::backend::renderer::{ImportMem, Renderer, Texture};
use smithay::utils::{Physical, Size};

/// Events produced by a compositor backend
pub enum BackendEvent<I: InputBackend> {
    /// Input event from the backend
    Input(InputEvent<I>),
    /// The compositor window was resized
    Resized { width: u16, height: u16 },
    /// Close was requested (e.g., window close button)
    CloseRequested,
    /// Focus state changed
    Focus { focused: bool },
    /// Refresh requested (window needs redraw)
    Refresh,
    /// Buffer presentation completed
    PresentCompleted,
}

impl<I: InputBackend> std::fmt::Debug for BackendEvent<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Input(_) => write!(f, "Input(...)"),
            Self::Resized { width, height } => f
                .debug_struct("Resized")
                .field("width", width)
                .field("height", height)
                .finish(),
            Self::CloseRequested => write!(f, "CloseRequested"),
            Self::Focus { focused } => f.debug_struct("Focus").field("focused", focused).finish(),
            Self::Refresh => write!(f, "Refresh"),
            Self::PresentCompleted => write!(f, "PresentCompleted"),
        }
    }
}

/// Configuration for backend initialization
#[derive(Debug, Clone)]
pub struct BackendConfig {
    /// Window title
    pub title: String,
    /// Initial window size
    pub initial_size: (u16, u16),
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            title: "Column Compositor".to_string(),
            initial_size: (1280, 800),
        }
    }
}

/// Trait for compositor backends
///
/// A backend provides:
/// - A renderer for drawing
/// - A surface for presenting frames
/// - An event source for input and window events
/// - Output size tracking
pub trait CompositorBackend: Sized {
    /// The renderer type provided by this backend
    type Renderer: CompositorRenderer;

    /// The surface type for presenting frames
    type Surface: CompositorSurface<Buffer = Self::Buffer>;

    /// The buffer type used by the surface
    type Buffer;

    /// The input backend type for events
    type InputBackend: InputBackend;

    /// Create a new backend with the given configuration
    fn new(config: &BackendConfig) -> anyhow::Result<Self>;

    /// Get a mutable reference to the renderer
    fn renderer(&mut self) -> &mut Self::Renderer;

    /// Get a mutable reference to the surface
    fn surface(&mut self) -> &mut Self::Surface;

    /// Get the current output size in physical pixels
    fn output_size(&self) -> Size<i32, Physical>;

    /// Poll for backend events
    ///
    /// Returns an iterator of events that occurred since last poll.
    /// This is non-blocking - returns empty if no events are pending.
    fn poll_events(&mut self) -> impl Iterator<Item = BackendEvent<Self::InputBackend>>;

    /// Whether this backend supports cursor management
    fn supports_cursor(&self) -> bool {
        false
    }

    /// Set the cursor to a resize cursor (if supported)
    fn set_resize_cursor(&mut self, _resize: bool) {}
}

/// Trait for compositor renderers
///
/// Extends Smithay's Renderer and ImportMem traits with additional requirements
/// for the compositor's rendering pipeline.
pub trait CompositorRenderer: Renderer + ImportMem {
    /// The texture type produced by this renderer
    type CompositorTexture: Texture + Clone;

    /// Bind a buffer for rendering
    fn bind_buffer<'a, B>(
        &'a mut self,
        buffer: &'a mut B,
    ) -> anyhow::Result<BoundRenderer<'a, Self, B>>
    where
        B: RenderBuffer;
}

/// Trait for render buffers that can be bound to a renderer
pub trait RenderBuffer {}

/// A renderer bound to a buffer, ready for drawing
pub struct BoundRenderer<'a, R: CompositorRenderer + ?Sized, B: RenderBuffer> {
    pub renderer: &'a mut R,
    pub buffer: &'a mut B,
}

/// Trait for compositor surfaces (display targets)
///
/// A surface manages buffer allocation and presentation to the display.
pub trait CompositorSurface {
    /// The buffer type used by this surface
    type Buffer: RenderBuffer;

    /// Get a buffer for rendering
    ///
    /// Returns (buffer, age) where age is the buffer age for damage tracking.
    fn buffer(&mut self) -> anyhow::Result<(Self::Buffer, i32)>;

    /// Submit the rendered buffer to the display
    fn submit(&mut self) -> anyhow::Result<()>;
}

/// Backend selection based on environment variable
pub fn select_backend() -> BackendType {
    match std::env::var("TERMSTACK_BACKEND").as_deref() {
        Ok("headless") => BackendType::Headless,
        Ok("x11") => BackendType::X11,
        // Default to X11 for normal operation
        _ => BackendType::X11,
    }
}

/// Available backend types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendType {
    /// X11 backend with GPU acceleration
    X11,
    /// Headless backend with software rendering
    Headless,
}
