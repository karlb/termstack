//! Runtime configuration

use serde::{Deserialize, Serialize};

/// Color theme for the terminal (config file format)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl Theme {
    /// Get default background color for this theme (as f32 RGBA)
    pub fn background_color(&self) -> [f32; 4] {
        match self {
            Theme::Dark => [0.1, 0.1, 0.1, 1.0],   // #1A1A1A
            Theme::Light => [1.0, 1.0, 1.0, 1.0],  // #FFFFFF
        }
    }

    /// Convert to terminal crate's Theme type
    pub fn to_terminal_theme(&self) -> terminal::Theme {
        match self {
            Theme::Dark => terminal::Theme::Dark,
            Theme::Light => terminal::Theme::Light,
        }
    }
}

/// Compositor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Color theme (light or dark)
    pub theme: Theme,

    /// Font size in pixels (default: 14.0)
    pub font_size: f32,

    /// Background color (ARGB) - overrides theme default if set
    pub background_color: [f32; 4],

    /// Gap between windows (pixels)
    pub window_gap: u32,

    /// Minimum window height (pixels)
    pub min_window_height: u32,

    /// Maximum window height (pixels, 0 = unlimited)
    pub max_window_height: u32,

    /// Scroll speed multiplier
    pub scroll_speed: f64,

    /// Auto-scroll when focused window grows
    pub auto_scroll: bool,

    /// Keyboard configuration
    pub keyboard: KeyboardConfig,

    /// App IDs that use client-side decorations (skip compositor title bar)
    /// Supports prefix matching with "*" (e.g., "org.gnome.*")
    pub csd_apps: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        let theme = Theme::default();
        Self {
            background_color: theme.background_color(),
            theme,
            font_size: 14.0,
            window_gap: 0,
            min_window_height: 50,
            max_window_height: 0,
            scroll_speed: 1.0,
            auto_scroll: true,
            keyboard: KeyboardConfig::default(),
            csd_apps: Vec::new(),
        }
    }
}

/// Keyboard configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyboardConfig {
    /// XKB layout
    pub layout: String,

    /// XKB variant
    pub variant: String,

    /// XKB model
    pub model: String,

    /// XKB options
    pub options: String,

    /// Key repeat delay (ms)
    pub repeat_delay: u32,

    /// Key repeat rate (keys per second)
    pub repeat_rate: u32,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: String::new(),
            variant: String::new(),
            model: String::new(),
            options: String::new(),
            repeat_delay: 400,
            repeat_rate: 25,
        }
    }
}

impl Config {
    /// Load configuration from file, falling back to defaults
    pub fn load() -> Self {
        let config_paths = [
            dirs::config_dir().map(|p| p.join("termstack/config.toml")),
            Some(std::path::PathBuf::from("/etc/termstack/config.toml")),
        ];

        for path in config_paths.into_iter().flatten() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str::<Config>(&content) {
                        Ok(mut config) => {
                            // Apply theme-based background if not explicitly set
                            // (check if it's still the dark default when theme is light)
                            config.apply_theme_defaults();
                            tracing::info!(?path, ?config.theme, csd_apps = ?config.csd_apps, "loaded configuration");
                            return config;
                        }
                        Err(e) => {
                            tracing::warn!(?path, error = %e, "failed to parse config");
                        }
                    },
                    Err(e) => {
                        tracing::warn!(?path, error = %e, "failed to read config");
                    }
                }
            }
        }

        tracing::info!("using default configuration");
        Self::default()
    }

    /// Apply theme-based defaults for colors that weren't explicitly set
    fn apply_theme_defaults(&mut self) {
        // If background_color is still the serde default (dark theme),
        // update it to match the actual theme
        let dark_bg = Theme::Dark.background_color();
        if self.background_color == dark_bg && self.theme == Theme::Light {
            self.background_color = self.theme.background_color();
        }
    }

    /// Check if an app_id matches the CSD apps patterns
    /// Supports exact match and prefix match with "*" suffix (e.g., "org.gnome.*")
    pub fn is_csd_app(&self, app_id: &str) -> bool {
        self.csd_apps.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix('*') {
                app_id.starts_with(prefix)
            } else {
                app_id == pattern
            }
        })
    }
}

/// Helper for getting XDG directories
mod dirs {
    use std::path::PathBuf;

    pub fn config_dir() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }
}
