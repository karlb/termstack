//! Title bar rendering for external windows
//!
//! Renders a title bar showing the command that spawned a GUI window.

use std::collections::HashMap;

/// Title bar height in pixels
pub const TITLE_BAR_HEIGHT: u32 = 24;

/// Title bar renderer
pub struct TitleBarRenderer {
    /// Font for rendering
    font: fontdue::Font,

    /// Font size
    font_size: f32,

    /// Glyph cache
    glyph_cache: HashMap<char, GlyphData>,
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
    /// Create a new title bar renderer
    pub fn new() -> Option<Self> {
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

        // Fill background (dark gray: #333333)
        let bg_r = 0x33u8;
        let bg_g = 0x33u8;
        let bg_b = 0x33u8;
        let bg_a = 0xFFu8;

        for y in 0..height {
            for x in 0..width {
                let idx = ((y * width + x) * 4) as usize;
                buffer[idx] = bg_b;     // B
                buffer[idx + 1] = bg_g; // G
                buffer[idx + 2] = bg_r; // R
                buffer[idx + 3] = bg_a; // A
            }
        }

        // Render text (light gray: #CCCCCC)
        let fg_r = 0xCCu8;
        let fg_g = 0xCCu8;
        let fg_b = 0xCCu8;

        // Add "> " prefix to match shell prompt style
        let display_text = format!("> {}", text);

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

            // Stop if we're past the visible area
            if x_pos >= (width - padding) as f32 {
                break;
            }
        }

        (buffer, width, height)
    }
}

impl Default for TitleBarRenderer {
    fn default() -> Self {
        Self::new().expect("Failed to create TitleBarRenderer - no font available")
    }
}
