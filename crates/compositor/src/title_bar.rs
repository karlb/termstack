//! Title bar rendering for external windows
//!
//! Renders a title bar showing the command that spawned a GUI window.
//! Also tracks character positions for text selection hit-testing.

use std::collections::HashMap;
use terminal::Theme;

/// Title bar height in pixels
pub const TITLE_BAR_HEIGHT: u32 = 24;

/// Close button width in pixels (square button)
pub const CLOSE_BUTTON_WIDTH: u32 = 24;

/// Height of gradient transition zone (pixels)
pub const GRADIENT_HEIGHT: u32 = 24;

/// Left padding for title bar text (pixels)
pub const TITLE_BAR_PADDING: u32 = 8;

/// Character position information for text selection
#[derive(Debug, Clone)]
pub struct TitleBarCharInfo {
    /// The displayed text (may be truncated)
    pub text: String,
    /// X position of each character's left edge (in pixels from left padding)
    /// char_positions[i] is the start of character i
    /// char_positions.len() == text.chars().count()
    pub char_positions: Vec<f32>,
    /// Width of each character (for selection highlighting)
    pub char_widths: Vec<f32>,
}

impl TitleBarCharInfo {
    /// Find the character index at a given X position (relative to content area start)
    ///
    /// Returns None if x is before the text or after the last character.
    /// For positions between characters, returns the character whose cell contains x.
    pub fn char_index_at_x(&self, x: f32) -> Option<usize> {
        if self.char_positions.is_empty() || x < 0.0 {
            return None;
        }

        for (i, &start) in self.char_positions.iter().enumerate() {
            let width = self.char_widths.get(i).copied().unwrap_or(0.0);
            let end = start + width;
            if x >= start && x < end {
                return Some(i);
            }
        }

        // Past the last character - return last char index if within text bounds
        if let (Some(&last_start), Some(&last_width)) = (
            self.char_positions.last(),
            self.char_widths.last(),
        ) {
            if x < last_start + last_width + 5.0 {
                // Small tolerance
                return Some(self.char_positions.len().saturating_sub(1));
            }
        }

        None
    }

    /// Get the text from start_char to end_char (inclusive)
    pub fn text_range(&self, start_char: usize, end_char: usize) -> String {
        self.text
            .chars()
            .skip(start_char)
            .take(end_char.saturating_sub(start_char) + 1)
            .collect()
    }
}

/// Theme-specific colors for title bar
struct TitleBarColors {
    /// Background color (RGBA bytes)
    bg_r: u8,
    bg_g: u8,
    bg_b: u8,
    /// Foreground/text color (RGB bytes)
    fg_r: u8,
    fg_g: u8,
    fg_b: u8,
    /// Close button background (RGB bytes)
    btn_bg_r: u8,
    btn_bg_g: u8,
    btn_bg_b: u8,
    /// Close button text (RGB bytes)
    btn_fg_r: u8,
    btn_fg_g: u8,
    btn_fg_b: u8,
}

impl TitleBarColors {
    fn from_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                bg_r: 0x33, bg_g: 0x33, bg_b: 0x33,     // #333333
                fg_r: 0xCC, fg_g: 0xCC, fg_b: 0xCC,     // #CCCCCC
                btn_bg_r: 0x44, btn_bg_g: 0x44, btn_bg_b: 0x44, // #444444
                btn_fg_r: 0xFF, btn_fg_g: 0xFF, btn_fg_b: 0xFF, // White
            },
            Theme::Light => Self {
                bg_r: 0xE0, bg_g: 0xE0, bg_b: 0xE0,     // #E0E0E0
                fg_r: 0x1A, fg_g: 0x1A, fg_b: 0x1A,     // #1A1A1A
                btn_bg_r: 0xD0, btn_bg_g: 0xD0, btn_bg_b: 0xD0, // #D0D0D0
                btn_fg_r: 0x1A, btn_fg_g: 0x1A, btn_fg_b: 0x1A, // Dark
            },
        }
    }

    fn content_background(&self, theme: Theme) -> (u8, u8, u8) {
        match theme {
            Theme::Dark => (0x1A, 0x1A, 0x1A),  // Terminal dark bg
            Theme::Light => (0xFF, 0xFF, 0xFF), // Terminal light bg
        }
    }
}

/// Title bar renderer
pub struct TitleBarRenderer {
    /// Font for rendering
    font: fontdue::Font,

    /// Font size
    font_size: f32,

    /// Glyph cache
    glyph_cache: HashMap<char, GlyphData>,

    /// Color theme
    theme: Theme,

    /// UI scale factor (1.0 = no scaling, 2.0 = Retina)
    scale: f32,
}

struct GlyphData {
    bitmap: Vec<u8>,
    width: u32,
    height: u32,
    x_offset: i32,
    y_offset: i32,
    advance: f32,
}

impl TitleBarRenderer {
    /// Common font search paths
    const FONT_SEARCH_PATHS: &'static [&'static str] = &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoMono-Regular.ttf",
        "/usr/share/fonts/noto/NotoMono-Regular.ttf",
        // macOS fonts
        "/System/Library/Fonts/Supplemental/Courier New.ttf",
        "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
        "/System/Library/Fonts/Menlo.ttc",
        "/Library/Fonts/SF-Mono-Regular.otf",
    ];

    /// Try to find and load a font from common system locations
    fn find_font() -> Option<(fontdue::Font, &'static str)> {
        Self::FONT_SEARCH_PATHS.iter().find_map(|&path| {
            let data = std::fs::read(path).ok()?;
            let font = fontdue::Font::from_bytes(
                data.as_slice(),
                fontdue::FontSettings::default(),
            ).ok()?;
            Some((font, path))
        })
    }

    /// Create a new title bar renderer with theme (scale = 1.0)
    pub fn new(theme: Theme) -> Option<Self> {
        Self::new_scaled(theme, 1.0)
    }

    /// Create a new title bar renderer with theme and UI scale factor.
    ///
    /// The scale factor multiplies font size and all title bar dimensions
    /// (height, close button width, padding). Use 1.0 for standard displays,
    /// 2.0 for Retina/HiDPI.
    pub fn new_scaled(theme: Theme, scale: f32) -> Option<Self> {
        match Self::find_font() {
            Some((font, path)) => {
                tracing::info!("TitleBarRenderer: loaded font from {} (scale={})", path, scale);
                Some(Self {
                    font,
                    font_size: 14.0 * scale,
                    glyph_cache: HashMap::new(),
                    theme,
                    scale,
                })
            }
            None => {
                tracing::warn!("TitleBarRenderer: no font found");
                None
            }
        }
    }

    /// Scaled title bar height in pixels
    pub fn title_bar_height(&self) -> u32 {
        (TITLE_BAR_HEIGHT as f32 * self.scale) as u32
    }

    /// Scaled close button width in pixels
    pub fn close_button_width(&self) -> u32 {
        (CLOSE_BUTTON_WIDTH as f32 * self.scale) as u32
    }

    /// Scaled title bar padding in pixels
    pub fn title_bar_padding(&self) -> u32 {
        (TITLE_BAR_PADDING as f32 * self.scale) as u32
    }

    /// Render a title bar to a pixel buffer (ARGB32)
    ///
    /// Returns (pixels, width, height)
    pub fn render(&mut self, text: &str, width: u32) -> (Vec<u8>, u32, u32) {
        let (buffer, w, h, _char_info) = self.render_with_char_info(text, width);
        (buffer, w, h)
    }

    /// Render a title bar and return character position information for text selection
    ///
    /// Returns (pixels, width, height, char_info)
    pub fn render_with_char_info(
        &mut self,
        text: &str,
        width: u32,
    ) -> (Vec<u8>, u32, u32, TitleBarCharInfo) {
        let height = self.title_bar_height();
        let close_btn_width = self.close_button_width();
        let padding = self.title_bar_padding();
        let mut buffer = vec![0u8; (width * height * 4) as usize];
        let colors = TitleBarColors::from_theme(self.theme);

        // Fill background
        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                buffer[idx] = colors.bg_b;     // B
                buffer[idx + 1] = colors.bg_g; // G
                buffer[idx + 2] = colors.bg_r; // R
                buffer[idx + 3] = 0xFF;        // A
            }
        }

        // Text colors from theme
        let fg_r = colors.fg_r;
        let fg_g = colors.fg_g;
        let fg_b = colors.fg_b;

        // Display text as-is - prompt already includes any prefix
        let display_text = text;

        // Starting position with padding
        let mut x_pos = padding as f32;
        let baseline_y = (height as f32 * 0.75) as i32; // Approximate baseline

        // Track character positions for selection hit-testing
        let mut char_positions: Vec<f32> = Vec::new();
        let mut char_widths: Vec<f32> = Vec::new();
        let mut displayed_text = String::new();

        for c in display_text.chars() {
            // Get or rasterize glyph
            let glyph = self.glyph_cache.entry(c).or_insert_with(|| {
                let (metrics, bitmap) = self.font.rasterize(c, self.font_size);
                GlyphData {
                    bitmap,
                    width: metrics.width as u32,
                    height: metrics.height as u32,
                    x_offset: metrics.xmin,
                    y_offset: metrics.ymin,
                    advance: metrics.advance_width,
                }
            });

            // Track character position (relative to padding start)
            char_positions.push(x_pos - padding as f32);
            char_widths.push(glyph.advance);
            displayed_text.push(c);

            // Calculate glyph position
            let glyph_x = (x_pos as i32 + glyph.x_offset).max(0) as u32;
            let glyph_y = (baseline_y - glyph.height as i32 - glyph.y_offset).max(0) as u32;

            // Draw glyph with alpha blending
            for gy in 0..glyph.height {
                let py = glyph_y + gy;
                if py >= height {
                    break;
                }

                for gx in 0..glyph.width {
                    let px = glyph_x + gx;
                    if px >= width {
                        break;
                    }

                    let alpha = glyph.bitmap[(gy * glyph.width + gx) as usize];
                    if alpha == 0 {
                        continue;
                    }

                    let idx = ((py * width + px) * 4) as usize;
                    if idx + 3 >= buffer.len() {
                        continue;
                    }

                    // Alpha blend
                    let alpha_f = alpha as f32 / 255.0;
                    let inv_alpha = 1.0 - alpha_f;

                    buffer[idx] = (fg_b as f32 * alpha_f + buffer[idx] as f32 * inv_alpha) as u8;
                    buffer[idx + 1] = (fg_g as f32 * alpha_f + buffer[idx + 1] as f32 * inv_alpha) as u8;
                    buffer[idx + 2] = (fg_r as f32 * alpha_f + buffer[idx + 2] as f32 * inv_alpha) as u8;
                    // Alpha channel stays at 0xFF
                }
            }

            x_pos += glyph.advance;

            // Stop if we're past the visible area (leave room for close button)
            if x_pos >= (width - padding - close_btn_width) as f32 {
                break;
            }
        }

        // Draw close button on the right side
        self.render_close_button(&mut buffer, width, height);

        // Render gradient at bottom of title bar (blends into content below)
        // In buffer coords: higher Y = bottom of title bar (closer to content)
        let content_bg = colors.content_background(self.theme);
        let gradient_pixels = ((GRADIENT_HEIGHT as f32 * self.scale) as u32).min(height);
        let gradient_start_y = height - gradient_pixels;  // Start N pixels from bottom

        for y_offset in gradient_start_y..height {
            let distance_from_start = y_offset - gradient_start_y;
            let blend = distance_from_start as f32 / gradient_pixels as f32;

            // Interpolate from title bg to content bg (downward)
            let r = (colors.bg_r as f32 * (1.0 - blend) + content_bg.0 as f32 * blend) as u8;
            let g = (colors.bg_g as f32 * (1.0 - blend) + content_bg.1 as f32 * blend) as u8;
            let b = (colors.bg_b as f32 * (1.0 - blend) + content_bg.2 as f32 * blend) as u8;

            for x in 0..width {
                let idx = ((y_offset * width + x) * 4) as usize;
                // Only overwrite background, preserve text/close button pixels if already rendered
                // Check if pixel is still background color before overwriting
                let is_background = buffer[idx] == colors.bg_b
                    && buffer[idx + 1] == colors.bg_g
                    && buffer[idx + 2] == colors.bg_r;

                if is_background {
                    buffer[idx] = b;
                    buffer[idx + 1] = g;
                    buffer[idx + 2] = r;
                    buffer[idx + 3] = 0xFF;
                }
            }
        }

        let char_info = TitleBarCharInfo {
            text: displayed_text,
            char_positions,
            char_widths,
        };

        (buffer, width, height, char_info)
    }

    /// Render the close button
    fn render_close_button(&mut self, buffer: &mut [u8], width: u32, height: u32) {
        let btn_width = self.close_button_width();
        let btn_x = width - btn_width;
        let btn_height = height;
        let colors = TitleBarColors::from_theme(self.theme);

        // Button background from theme
        for y in 0..btn_height {
            for x in 0..btn_width {
                let px = btn_x + x;
                let idx = ((y * width + px) * 4) as usize;
                if idx + 3 < buffer.len() {
                    buffer[idx] = colors.btn_bg_b;     // B
                    buffer[idx + 1] = colors.btn_bg_g; // G
                    buffer[idx + 2] = colors.btn_bg_r; // R
                    buffer[idx + 3] = 0xFF;            // A
                }
            }
        }

        // Draw "×" character centered in button
        let close_char = '×';
        let glyph = self.glyph_cache.entry(close_char).or_insert_with(|| {
            let (metrics, bitmap) = self.font.rasterize(close_char, self.font_size);
            GlyphData {
                bitmap,
                width: metrics.width as u32,
                height: metrics.height as u32,
                x_offset: metrics.xmin,
                y_offset: metrics.ymin,
                advance: metrics.advance_width,
            }
        });

        // Center the glyph in the button
        let glyph_x = btn_x + (btn_width.saturating_sub(glyph.width)) / 2;
        let baseline_y = (height as f32 * 0.75) as i32;
        let glyph_y = (baseline_y - glyph.height as i32 - glyph.y_offset).max(0) as u32;

        // Text color from theme
        let fg_r = colors.btn_fg_r;
        let fg_g = colors.btn_fg_g;
        let fg_b = colors.btn_fg_b;

        for gy in 0..glyph.height {
            let py = glyph_y + gy;
            if py >= height {
                break;
            }

            for gx in 0..glyph.width {
                let px = glyph_x + gx;
                if px >= width {
                    break;
                }

                let alpha = glyph.bitmap[(gy * glyph.width + gx) as usize];
                if alpha == 0 {
                    continue;
                }

                let idx = ((py * width + px) * 4) as usize;
                if idx + 3 >= buffer.len() {
                    continue;
                }

                // Alpha blend
                let alpha_f = alpha as f32 / 255.0;
                let inv_alpha = 1.0 - alpha_f;

                buffer[idx] = (fg_b as f32 * alpha_f + buffer[idx] as f32 * inv_alpha) as u8;
                buffer[idx + 1] = (fg_g as f32 * alpha_f + buffer[idx + 1] as f32 * inv_alpha) as u8;
                buffer[idx + 2] = (fg_r as f32 * alpha_f + buffer[idx + 2] as f32 * inv_alpha) as u8;
            }
        }
    }
}

impl Default for TitleBarRenderer {
    fn default() -> Self {
        Self::new(Theme::default()).expect("Failed to create TitleBarRenderer - no font available")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_info_text_range_basic() {
        let info = TitleBarCharInfo {
            text: "Hello World".to_string(),
            char_positions: vec![0.0, 8.0, 16.0, 24.0, 32.0, 40.0, 48.0, 56.0, 64.0, 72.0, 80.0],
            char_widths: vec![8.0; 11],
        };

        assert_eq!(info.text_range(0, 4), "Hello");
        assert_eq!(info.text_range(6, 10), "World");
        assert_eq!(info.text_range(0, 10), "Hello World");
    }

    #[test]
    fn char_info_char_index_at_x() {
        let info = TitleBarCharInfo {
            text: "ABC".to_string(),
            char_positions: vec![0.0, 10.0, 20.0],
            char_widths: vec![10.0, 10.0, 10.0],
        };

        // First character (0-10)
        assert_eq!(info.char_index_at_x(0.0), Some(0));
        assert_eq!(info.char_index_at_x(5.0), Some(0));
        assert_eq!(info.char_index_at_x(9.9), Some(0));

        // Second character (10-20)
        assert_eq!(info.char_index_at_x(10.0), Some(1));
        assert_eq!(info.char_index_at_x(15.0), Some(1));

        // Third character (20-30)
        assert_eq!(info.char_index_at_x(20.0), Some(2));
        assert_eq!(info.char_index_at_x(25.0), Some(2));

        // Just past the last character (with tolerance)
        assert_eq!(info.char_index_at_x(30.0), Some(2));
        assert_eq!(info.char_index_at_x(34.0), Some(2));

        // Before text
        assert_eq!(info.char_index_at_x(-1.0), None);

        // Way past the text
        assert_eq!(info.char_index_at_x(100.0), None);
    }

    #[test]
    fn char_info_empty() {
        let info = TitleBarCharInfo {
            text: String::new(),
            char_positions: vec![],
            char_widths: vec![],
        };

        assert_eq!(info.char_index_at_x(0.0), None);
        assert_eq!(info.char_index_at_x(10.0), None);
        assert_eq!(info.text_range(0, 5), "");
    }

    #[test]
    fn render_with_char_info_produces_positions() {
        // Skip this test if no font is available
        let Some(mut renderer) = TitleBarRenderer::new(Theme::Dark) else {
            return;
        };

        let text = "test";
        let (_, _, _, char_info) = renderer.render_with_char_info(text, 200);

        assert_eq!(char_info.text.len(), text.len());
        assert_eq!(char_info.char_positions.len(), text.len());
        assert_eq!(char_info.char_widths.len(), text.len());

        // Positions should be increasing
        for i in 1..char_info.char_positions.len() {
            assert!(
                char_info.char_positions[i] > char_info.char_positions[i - 1],
                "character positions should be increasing"
            );
        }
    }
}
