//! Terminal grid to pixel buffer rendering
//!
//! Renders alacritty_terminal grid to an ARGB pixel buffer using fontdue.

use std::collections::HashMap;

use alacritty_terminal::event::EventListener;
use alacritty_terminal::index::Point;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::Term;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

/// Color theme for the terminal
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Get selection background color for this theme
    fn selection_bg(&self) -> u32 {
        match self {
            Theme::Dark => 0xFF264F78,  // Blue tint
            Theme::Light => 0xFFADD6FF, // Light blue
        }
    }

    /// Get cursor color for this theme
    fn cursor_color(&self) -> u32 {
        match self {
            Theme::Dark => 0xFFCCCCCC,  // Light gray
            Theme::Light => 0xFF1A1A1A, // Dark gray
        }
    }

    /// Get background color for this theme
    fn background(&self) -> u32 {
        match self {
            Theme::Dark => 0xFF1A1A1A,
            Theme::Light => 0xFFFFFFFF,
        }
    }

    /// Get foreground color for this theme
    fn foreground(&self) -> u32 {
        match self {
            Theme::Dark => 0xFFCCCCCC,
            Theme::Light => 0xFF1A1A1A,
        }
    }
}

/// Font configuration
pub struct FontConfig {
    /// Font for rendering
    pub font: fontdue::Font,

    /// Font size in pixels
    pub size: f32,

    /// Cell width in pixels
    pub cell_width: u32,

    /// Cell height in pixels
    pub cell_height: u32,
}

impl FontConfig {
    /// Create font config from a TTF font
    pub fn from_bytes(font_data: &[u8], size: f32) -> Option<Self> {
        let font = fontdue::Font::from_bytes(font_data, fontdue::FontSettings::default()).ok()?;

        // Calculate cell dimensions based on metrics
        let metrics = font.metrics('M', size);
        let cell_width = metrics.advance_width.ceil() as u32;
        let cell_height = (size * 1.2).ceil() as u32; // Line height ~1.2x font size

        Some(Self {
            font,
            size,
            cell_width,
            cell_height,
        })
    }

    /// Create default font config with the given size
    /// Returns None if no font is available (graceful degradation)
    pub fn try_default_font_with_size(size: f32) -> Option<Self> {
        // Try to load a monospace font from common locations
        let font_paths = [
            // DejaVu Sans Mono
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
            "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
            // Liberation Mono
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
            // Noto Mono
            "/usr/share/fonts/truetype/noto/NotoMono-Regular.ttf",
            "/usr/share/fonts/noto/NotoMono-Regular.ttf",
            // Hack
            "/usr/share/fonts/truetype/hack/Hack-Regular.ttf",
            // Ubuntu Mono
            "/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf",
            // Fira Code / Fira Mono
            "/usr/share/fonts/truetype/firacode/FiraCode-Regular.ttf",
            "/usr/share/fonts/opentype/firacode/FiraCode-Regular.otf",
            // macOS fonts
            "/System/Library/Fonts/Supplemental/Courier New.ttf",
            "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
            "/System/Library/Fonts/Menlo.ttc",
            "/Library/Fonts/SF-Mono-Regular.otf",
        ];

        for path in &font_paths {
            if let Ok(data) = std::fs::read(path) {
                if let Some(config) = Self::from_bytes(&data, size) {
                    tracing::info!("Loaded font from: {} (size {})", path, size);
                    return Some(config);
                }
            }
        }

        tracing::error!(
            "No monospace font found in standard paths. \
             Terminal text will not render. \
             Install a monospace font:\n\
             - Debian/Ubuntu: sudo apt install fonts-dejavu\n\
             - Fedora/RHEL: sudo dnf install dejavu-sans-mono-fonts\n\
             - Arch: sudo pacman -S ttf-dejavu"
        );

        None
    }

    /// Create default font config with the given size
    /// Returns None if no system font is available (renderer will skip text)
    pub fn default_font_with_size(size: f32) -> Option<Self> {
        Self::try_default_font_with_size(size)
    }

    /// Create default font config with default size (14.0)
    /// Returns None if no system font is available
    pub fn default_font() -> Option<Self> {
        Self::default_font_with_size(14.0)
    }
}

/// Terminal renderer
pub struct TerminalRenderer {
    /// Font configuration
    font: Option<FontConfig>,

    /// Pixel buffer (ARGB32)
    buffer: Vec<u32>,

    /// Buffer dimensions
    width: u32,
    height: u32,

    /// Glyph cache
    glyph_cache: HashMap<(char, u32), GlyphData>,

    /// Cell dimensions
    cell_width: u32,
    cell_height: u32,

    /// Color theme
    theme: Theme,
}

struct GlyphData {
    bitmap: Vec<u8>,
    width: u32,
    height: u32,
    x_offset: i32,
    y_offset: i32,
}

impl TerminalRenderer {
    /// Create a new renderer with default settings
    pub fn new() -> Self {
        Self::with_theme(Theme::default())
    }

    /// Create a new renderer with theme
    pub fn with_theme(theme: Theme) -> Self {
        Self {
            font: None,
            buffer: Vec::new(),
            width: 0,
            height: 0,
            glyph_cache: HashMap::new(),
            cell_width: 8,
            cell_height: 16,
            theme,
        }
    }

    /// Create a new renderer with font
    pub fn with_font(font: FontConfig) -> Self {
        Self::with_font_and_theme(font, Theme::default())
    }

    /// Create a new renderer with font and theme
    pub fn with_font_and_theme(font: FontConfig, theme: Theme) -> Self {
        let cell_width = font.cell_width;
        let cell_height = font.cell_height;
        Self {
            font: Some(font),
            buffer: Vec::new(),
            width: 0,
            height: 0,
            glyph_cache: HashMap::new(),
            cell_width,
            cell_height,
            theme,
        }
    }

    /// Get cell dimensions
    pub fn cell_size(&self) -> (u32, u32) {
        (self.cell_width, self.cell_height)
    }

    /// Replace font configuration and clear glyph cache
    pub fn set_font_size(&mut self, font_config: FontConfig) {
        self.cell_width = font_config.cell_width;
        self.cell_height = font_config.cell_height;
        self.glyph_cache.clear();
        self.font = Some(font_config);
    }

    /// Render terminal to buffer
    ///
    /// `viewport_offset` is how many lines we've scrolled back from the cursor:
    /// - 0 = showing live output (cursor at bottom of viewport)
    /// - >0 = scrolled into history
    pub fn render<T: EventListener>(
        &mut self,
        term: &Term<T>,
        width: u32,
        height: u32,
        show_cursor: bool,
        viewport_offset: usize,
    ) {
        // Resize buffer if needed
        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            self.buffer.resize((width * height) as usize, 0xFF000000);
        }

        // Clear with background color
        let bg_color = self.color_to_argb(&Color::Named(NamedColor::Background));
        self.buffer.fill(bg_color);

        let content = term.renderable_content();

        // Calculate viewport position
        let visible_rows = (height / self.cell_height).max(1);

        // Collect cells for two-pass processing (find last content line, then render)
        let cells: Vec<_> = content.display_iter.collect();

        // Find last non-empty line for content-based positioning
        // This avoids showing empty trailing rows when cursor is on an empty line
        let last_content_line = {
            let mut last = 0u32;
            for cell in &cells {
                let line = cell.point.line.0 as u32;
                let c = cell.c;
                if c != ' ' && c != '\0' && line > last {
                    last = line;
                }
            }
            last
        };

        // Calculate first visible line based on viewport offset
        // viewport_offset=0 means showing live output (bottom of content)
        // viewport_offset=N means scrolled N lines back into history
        let first_visible_line = if last_content_line < visible_rows {
            // Content fits in viewport - show from beginning
            0
        } else {
            // Content exceeds viewport
            // Base position shows ending at last content line
            let base_first_line = last_content_line.saturating_sub(visible_rows - 1);
            // Scroll back by viewport_offset lines
            base_first_line.saturating_sub(viewport_offset as u32)
        };

        // Get selection range for highlighting
        let selection = content.selection.as_ref();

        // Render each cell, offsetting by viewport position
        for cell in cells {
            let col = cell.point.column.0 as u32;
            let line = cell.point.line.0 as u32;

            // Skip lines outside viewport
            if line < first_visible_line {
                continue;
            }

            let x = col * self.cell_width;
            // Adjust Y to viewport: subtract first_visible_line
            let y = (line - first_visible_line) * self.cell_height;

            if x >= width || y >= height {
                continue;
            }

            // Check if this cell is selected
            let is_selected = selection
                .map(|sel| sel.contains(Point::new(cell.point.line, cell.point.column)))
                .unwrap_or(false);

            self.render_cell(x, y, cell.cell, is_selected);
        }

        // Render cursor (only if process is running and at live view)
        // Don't show cursor when scrolled into history
        if show_cursor && viewport_offset == 0 {
            let cursor = content.cursor;
            let x = cursor.point.column.0 as u32 * self.cell_width;
            // Adjust cursor Y to viewport
            let y = (cursor.point.line.0 as u32).saturating_sub(first_visible_line) * self.cell_height;

            if x < width && y < height {
                self.render_cursor(x, y);
            }
        }
    }

    fn render_cell(&mut self, x: u32, y: u32, cell: &alacritty_terminal::term::cell::Cell, is_selected: bool) {
        // Background - use selection color if selected, otherwise cell's background
        let bg = if is_selected {
            self.theme.selection_bg()
        } else {
            self.color_to_argb(&cell.bg)
        };
        self.fill_rect(x, y, self.cell_width, self.cell_height, bg);

        // Don't render space characters
        let c = cell.c;
        if c == ' ' || c == '\0' {
            return;
        }

        // Debug: log all rendered characters
        tracing::trace!("Rendering char: {:?} (U+{:04X}) at ({}, {})", c, c as u32, x, y);

        // Foreground (glyph) - use contrasting text on selection
        let fg = if is_selected {
            // Light theme: dark text on light blue selection
            // Dark theme: white text on dark blue selection
            match self.theme {
                Theme::Dark => 0xFFFFFFFF,
                Theme::Light => 0xFF000000,
            }
        } else {
            self.color_to_argb(&cell.fg)
        };
        self.draw_glyph(x, y, c, fg, cell.flags);
    }

    fn render_cursor(&mut self, x: u32, y: u32) {
        let cursor_color = self.theme.cursor_color();
        // Draw block cursor
        self.fill_rect(x, y, self.cell_width, self.cell_height, cursor_color);
    }

    fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        for dy in 0..h {
            let row_y = y + dy;
            if row_y >= self.height {
                break;
            }

            for dx in 0..w {
                let col_x = x + dx;
                if col_x >= self.width {
                    break;
                }

                let idx = (row_y * self.width + col_x) as usize;
                if idx < self.buffer.len() {
                    self.buffer[idx] = color;
                }
            }
        }
    }

    fn draw_glyph(&mut self, x: u32, y: u32, c: char, fg: u32, _flags: Flags) {
        let Some(font) = &self.font else {
            return;
        };

        let size_key = (font.size * 10.0) as u32;
        let cache_key = (c, size_key);

        // Get or rasterize glyph
        self.glyph_cache.entry(cache_key).or_insert_with(|| {
            let (metrics, bitmap) = font.font.rasterize(c, font.size);

            tracing::debug!(
                "Glyph '{}': size={}x{}, xmin={}, ymin={}, advance={}, bitmap_len={}",
                c, metrics.width, metrics.height, metrics.xmin, metrics.ymin,
                metrics.advance_width, bitmap.len()
            );

            GlyphData {
                bitmap,
                width: metrics.width as u32,
                height: metrics.height as u32,
                x_offset: metrics.xmin,
                y_offset: metrics.ymin,
            }
        });

        let glyph = &self.glyph_cache[&cache_key];

        // Calculate position with offset
        let baseline_y = y + (self.cell_height as i32 - 4) as u32; // Approximate baseline
        let glyph_x = (x as i32 + glyph.x_offset).max(0) as u32;
        let glyph_y = (baseline_y as i32 - glyph.height as i32 - glyph.y_offset).max(0) as u32;

        // Draw glyph bitmap
        let fg_r = (fg >> 16) & 0xFF;
        let fg_g = (fg >> 8) & 0xFF;
        let fg_b = fg & 0xFF;

        for gy in 0..glyph.height {
            let py = glyph_y + gy;
            if py >= self.height {
                break;
            }

            for gx in 0..glyph.width {
                let px = glyph_x + gx;
                if px >= self.width {
                    break;
                }

                let alpha = glyph.bitmap[(gy * glyph.width + gx) as usize] as u32;
                if alpha == 0 {
                    continue;
                }

                let idx = (py * self.width + px) as usize;
                if idx >= self.buffer.len() {
                    continue;
                }

                // Alpha blend
                let bg = self.buffer[idx];
                let bg_r = (bg >> 16) & 0xFF;
                let bg_g = (bg >> 8) & 0xFF;
                let bg_b = bg & 0xFF;

                let r = (fg_r * alpha + bg_r * (255 - alpha)) / 255;
                let g = (fg_g * alpha + bg_g * (255 - alpha)) / 255;
                let b = (fg_b * alpha + bg_b * (255 - alpha)) / 255;

                self.buffer[idx] = 0xFF000000 | (r << 16) | (g << 8) | b;
            }
        }
    }

    fn color_to_argb(&self, color: &Color) -> u32 {
        match color {
            Color::Named(named) => self.named_color_to_argb(*named),
            Color::Spec(rgb) => {
                0xFF000000 | ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32)
            }
            Color::Indexed(idx) => self.indexed_color_to_argb(*idx),
        }
    }

    fn named_color_to_argb(&self, color: NamedColor) -> u32 {
        // Standard terminal colors (theme-dependent for foreground/background/cursor)
        match color {
            NamedColor::Black => 0xFF000000,
            NamedColor::Red => 0xFFCC0000,
            NamedColor::Green => 0xFF00CC00,
            NamedColor::Yellow => 0xFFCCCC00,
            NamedColor::Blue => 0xFF0000CC,
            NamedColor::Magenta => 0xFFCC00CC,
            NamedColor::Cyan => 0xFF00CCCC,
            NamedColor::White => 0xFFCCCCCC,
            NamedColor::BrightBlack => 0xFF666666,
            NamedColor::BrightRed => 0xFFFF0000,
            NamedColor::BrightGreen => 0xFF00FF00,
            NamedColor::BrightYellow => 0xFFFFFF00,
            NamedColor::BrightBlue => 0xFF0000FF,
            NamedColor::BrightMagenta => 0xFFFF00FF,
            NamedColor::BrightCyan => 0xFF00FFFF,
            NamedColor::BrightWhite => 0xFFFFFFFF,
            NamedColor::Foreground => self.theme.foreground(),
            NamedColor::Background => self.theme.background(),
            NamedColor::Cursor => self.theme.cursor_color(),
            _ => self.theme.foreground(),
        }
    }

    fn indexed_color_to_argb(&self, idx: u8) -> u32 {
        if idx < 16 {
            // Standard colors
            self.named_color_to_argb(match idx {
                0 => NamedColor::Black,
                1 => NamedColor::Red,
                2 => NamedColor::Green,
                3 => NamedColor::Yellow,
                4 => NamedColor::Blue,
                5 => NamedColor::Magenta,
                6 => NamedColor::Cyan,
                7 => NamedColor::White,
                8 => NamedColor::BrightBlack,
                9 => NamedColor::BrightRed,
                10 => NamedColor::BrightGreen,
                11 => NamedColor::BrightYellow,
                12 => NamedColor::BrightBlue,
                13 => NamedColor::BrightMagenta,
                14 => NamedColor::BrightCyan,
                15 => NamedColor::BrightWhite,
                _ => NamedColor::White,
            })
        } else if idx < 232 {
            // 6x6x6 color cube
            let idx = idx - 16;
            let r = ((idx / 36) % 6) * 51;
            let g = ((idx / 6) % 6) * 51;
            let b = (idx % 6) * 51;
            0xFF000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        } else {
            // Grayscale
            let gray = ((idx - 232) * 10 + 8) as u32;
            0xFF000000 | (gray << 16) | (gray << 8) | gray
        }
    }

    /// Get the rendered buffer
    pub fn buffer(&self) -> &[u32] {
        &self.buffer
    }

    /// Get buffer dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl Default for TerminalRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::Terminal;

    /// Default theme's selection background color (for tests)
    const TEST_SELECTION_BG: u32 = 0xFF264F78;

    /// Get the background color at a specific character position from Terminal's buffer
    fn get_bg_color_at(terminal: &Terminal, col: u32, row: u32) -> u32 {
        let (cell_width, cell_height) = terminal.cell_size();
        let x = col * cell_width + 1; // Sample near center of cell
        let y = row * cell_height + 1;
        let buffer = terminal.buffer();
        let width = 80 * cell_width; // Assumes we rendered at 80 cols
        let idx = (y * width + x) as usize;
        buffer[idx]
    }

    #[test]
    fn selection_uses_distinct_background_color() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");
        terminal.inject_bytes(b"Hello World\r\n");

        let (cell_width, cell_height) = terminal.cell_size();
        let width = 80 * cell_width;
        let height = 24 * cell_height;

        // Render without selection
        terminal.render(width, height, true);
        let unselected_bg = get_bg_color_at(&terminal, 0, 0);

        // Make a selection on first characters (left-to-right)
        terminal.start_selection(0, 0);
        terminal.update_selection(0, 0, 4, 0);

        // Render with selection
        terminal.render(width, height, true);
        let selected_bg = get_bg_color_at(&terminal, 0, 0);

        // Selected background should be different (selection color)
        assert_ne!(
            unselected_bg, selected_bg,
            "selection background should differ from normal background"
        );
        assert_eq!(
            selected_bg, TEST_SELECTION_BG,
            "selected cell should use selection color"
        );
    }

    #[test]
    fn selection_only_affects_selected_cells() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");
        terminal.inject_bytes(b"ABCDEFGHIJ\r\n");

        // Select first 3 characters (cols 0-2), left-to-right
        terminal.start_selection(0, 0);
        terminal.update_selection(0, 0, 2, 0);

        let (cell_width, cell_height) = terminal.cell_size();
        let width = 80 * cell_width;
        let height = 24 * cell_height;
        terminal.render(width, height, true);

        // Cols 0-2 should have selection background
        for col in 0..=2 {
            let bg = get_bg_color_at(&terminal, col, 0);
            assert_eq!(
                bg, TEST_SELECTION_BG,
                "col {} should be selected",
                col
            );
        }

        // Cols 3+ should NOT have selection background
        for col in 3..10 {
            let bg = get_bg_color_at(&terminal, col, 0);
            assert_ne!(
                bg, TEST_SELECTION_BG,
                "col {} should not be selected",
                col
            );
        }
    }

    #[test]
    fn cleared_selection_restores_normal_background() {
        let mut terminal = Terminal::new(80, 24).expect("terminal creation");
        terminal.inject_bytes(b"Hello\r\n");

        let (cell_width, cell_height) = terminal.cell_size();
        let width = 80 * cell_width;
        let height = 24 * cell_height;

        // Render without selection first
        terminal.render(width, height, true);
        let normal_bg = get_bg_color_at(&terminal, 0, 0);

        // Add selection (left-to-right)
        terminal.start_selection(0, 0);
        terminal.update_selection(0, 0, 4, 0);
        terminal.render(width, height, true);
        let selected_bg = get_bg_color_at(&terminal, 0, 0);
        assert_eq!(selected_bg, TEST_SELECTION_BG);

        // Clear selection
        terminal.clear_selection();
        terminal.render(width, height, true);
        let cleared_bg = get_bg_color_at(&terminal, 0, 0);

        // Background should return to normal
        assert_eq!(
            cleared_bg, normal_bg,
            "cleared selection should restore normal background"
        );
    }
}
