//! Title bar rendering for external windows
//!
//! Renders a title bar showing the command that spawned a GUI window.

use std::collections::HashMap;
use terminal::Theme;

/// Title bar height in pixels
pub const TITLE_BAR_HEIGHT: u32 = 24;

/// Close button width in pixels (square button)
pub const CLOSE_BUTTON_WIDTH: u32 = 24;

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
    /// Create a new title bar renderer with theme
    pub fn new(theme: Theme) -> Option<Self> {
        // Try to load a font from common locations
        let font_paths = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
            "/usr/share/fonts/truetype/noto/NotoMono-Regular.ttf",
            "/usr/share/fonts/noto/NotoMono-Regular.ttf",
        ];

        for path in &font_paths {
            if let Ok(data) = std::fs::read(path) {
                if let Ok(font) = fontdue::Font::from_bytes(
                    data.as_slice(),
                    fontdue::FontSettings::default(),
                ) {
                    tracing::info!("TitleBarRenderer: loaded font from {}", path);
                    return Some(Self {
                        font,
                        font_size: 14.0,
                        glyph_cache: HashMap::new(),
                        theme,
                    });
                }
            }
        }

        tracing::warn!("TitleBarRenderer: no font found");
        None
    }

    /// Render a title bar to a pixel buffer (ARGB32)
    ///
    /// Returns (pixels, width, height)
    pub fn render(&mut self, text: &str, width: u32) -> (Vec<u8>, u32, u32) {
        let height = TITLE_BAR_HEIGHT;
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

        let display_text = text.to_string();

        // Starting position with padding
        let padding = 8u32;
        let mut x_pos = padding as f32;
        let baseline_y = (height as f32 * 0.75) as i32; // Approximate baseline

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
            if x_pos >= (width - padding - CLOSE_BUTTON_WIDTH) as f32 {
                break;
            }
        }

        // Draw close button on the right side
        self.render_close_button(&mut buffer, width, height);

        (buffer, width, height)
    }

    /// Render the close button
    fn render_close_button(&mut self, buffer: &mut [u8], width: u32, height: u32) {
        let btn_x = width - CLOSE_BUTTON_WIDTH;
        let btn_width = CLOSE_BUTTON_WIDTH;
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
