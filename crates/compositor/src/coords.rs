//! Coordinate type safety
//!
//! NewType wrappers that make coordinate space explicit and prevent
//! mixing coordinates from different systems at compile time.
//!
//! The compositor deals with three coordinate systems:
//! - Screen: Y=0 at top, as received from Winit input events
//! - Render: Y=0 at bottom, as used by OpenGL/Smithay
//! - Content: Absolute position in scrollable content (render + scroll offset)

/// Screen Y coordinate (Y=0 at top, from Winit)
///
/// This is the coordinate system used by input events from Winit.
/// When the user clicks at the top of the window, Y is near 0.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ScreenY(pub f64);

/// Render Y coordinate (Y=0 at bottom, for OpenGL/Smithay)
///
/// This is the coordinate system used by OpenGL and Smithay's Space.
/// When rendering at the bottom of the window, Y is near 0.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct RenderY(pub f64);

/// Content Y coordinate (absolute position in scrollable content)
///
/// This is the coordinate in the full content space, independent of scroll.
/// content_y = render_y + scroll_offset
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ContentY(pub f64);

impl ScreenY {
    /// Create a new screen Y coordinate
    pub fn new(y: f64) -> Self {
        Self(y)
    }

    /// Convert screen coordinates to render coordinates
    ///
    /// The Y-flip: screen Y=0 at top becomes render Y=height at bottom
    pub fn to_render(self, output_height: i32) -> RenderY {
        RenderY(output_height as f64 - self.0)
    }

    /// Get the raw value
    pub fn value(self) -> f64 {
        self.0
    }
}

impl RenderY {
    /// Create a new render Y coordinate
    pub fn new(y: f64) -> Self {
        Self(y)
    }

    /// Convert render coordinates to content coordinates
    ///
    /// Adds scroll offset to get absolute content position
    pub fn to_content(self, scroll_offset: f64) -> ContentY {
        ContentY(self.0 + scroll_offset)
    }

    /// Convert render coordinates back to screen coordinates
    ///
    /// The inverse Y-flip
    pub fn to_screen(self, output_height: i32) -> ScreenY {
        ScreenY(output_height as f64 - self.0)
    }

    /// Get the raw value
    pub fn value(self) -> f64 {
        self.0
    }

    /// Convert to i32 (truncating)
    pub fn as_i32(self) -> i32 {
        self.0 as i32
    }
}

impl ContentY {
    /// Create a new content Y coordinate
    pub fn new(y: f64) -> Self {
        Self(y)
    }

    /// Convert content coordinates back to render coordinates
    ///
    /// Subtracts scroll offset
    pub fn to_render(self, scroll_offset: f64) -> RenderY {
        RenderY(self.0 - scroll_offset)
    }

    /// Get the raw value
    pub fn value(self) -> f64 {
        self.0
    }

    /// Convert to i32 (truncating)
    pub fn as_i32(self) -> i32 {
        self.0 as i32
    }
}

/// A 2D point in screen coordinates
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: ScreenY,
}

/// A 2D point in render coordinates
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderPoint {
    pub x: f64,
    pub y: RenderY,
}

impl ScreenPoint {
    /// Create a new screen point
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y: ScreenY(y) }
    }

    /// Convert to render point
    pub fn to_render(self, output_height: i32) -> RenderPoint {
        RenderPoint {
            x: self.x,
            y: self.y.to_render(output_height),
        }
    }
}

impl RenderPoint {
    /// Create a new render point
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y: RenderY(y) }
    }

    /// Convert to screen point
    pub fn to_screen(self, output_height: i32) -> ScreenPoint {
        ScreenPoint {
            x: self.x,
            y: self.y.to_screen(output_height),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_to_render_flip() {
        let output_height = 720;

        // Top of screen (screen Y=0) should be bottom of render (render Y=720)
        let screen_top = ScreenY::new(0.0);
        assert_eq!(screen_top.to_render(output_height).value(), 720.0);

        // Bottom of screen (screen Y=720) should be top of render (render Y=0)
        let screen_bottom = ScreenY::new(720.0);
        assert_eq!(screen_bottom.to_render(output_height).value(), 0.0);

        // Middle should stay middle
        let screen_middle = ScreenY::new(360.0);
        assert_eq!(screen_middle.to_render(output_height).value(), 360.0);
    }

    #[test]
    fn render_to_screen_flip() {
        let output_height = 720;

        // Top of render (render Y=0) should be bottom of screen (screen Y=720)
        let render_top = RenderY::new(0.0);
        assert_eq!(render_top.to_screen(output_height).value(), 720.0);

        // Bottom of render (render Y=720) should be top of screen (screen Y=0)
        let render_bottom = RenderY::new(720.0);
        assert_eq!(render_bottom.to_screen(output_height).value(), 0.0);
    }

    #[test]
    fn roundtrip_screen_render_screen() {
        let output_height = 720;

        for y in [0.0, 100.0, 360.0, 500.0, 720.0] {
            let original = ScreenY::new(y);
            let roundtrip = original.to_render(output_height).to_screen(output_height);
            assert_eq!(original.value(), roundtrip.value(), "roundtrip failed for y={}", y);
        }
    }

    #[test]
    fn roundtrip_render_screen_render() {
        let output_height = 720;

        for y in [0.0, 100.0, 360.0, 500.0, 720.0] {
            let original = RenderY::new(y);
            let roundtrip = original.to_screen(output_height).to_render(output_height);
            assert_eq!(original.value(), roundtrip.value(), "roundtrip failed for y={}", y);
        }
    }

    #[test]
    fn render_to_content_adds_scroll() {
        let scroll_offset = 100.0;

        let render = RenderY::new(50.0);
        let content = render.to_content(scroll_offset);

        assert_eq!(content.value(), 150.0);
    }

    #[test]
    fn content_to_render_subtracts_scroll() {
        let scroll_offset = 100.0;

        let content = ContentY::new(150.0);
        let render = content.to_render(scroll_offset);

        assert_eq!(render.value(), 50.0);
    }

    #[test]
    fn roundtrip_render_content_render() {
        let scroll_offset = 100.0;

        for y in [0.0, 50.0, 100.0, 200.0] {
            let original = RenderY::new(y);
            let roundtrip = original.to_content(scroll_offset).to_render(scroll_offset);
            assert_eq!(original.value(), roundtrip.value(), "roundtrip failed for y={}", y);
        }
    }

    #[test]
    fn point_conversions() {
        let output_height = 720;

        let screen_point = ScreenPoint::new(100.0, 50.0);
        let render_point = screen_point.to_render(output_height);

        assert_eq!(render_point.x, 100.0); // X unchanged
        assert_eq!(render_point.y.value(), 670.0); // Y flipped: 720 - 50 = 670

        let back = render_point.to_screen(output_height);
        assert_eq!(back.x, screen_point.x);
        assert_eq!(back.y.value(), screen_point.y.value());
    }
}
