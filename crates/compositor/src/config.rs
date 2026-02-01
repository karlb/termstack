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

#[cfg(test)]
mod tests {
    use super::*;

    // ========== Theme tests ==========

    #[test]
    fn theme_dark_background_color() {
        let color = Theme::Dark.background_color();
        // Dark theme: #1A1A1A = [0.1, 0.1, 0.1, 1.0]
        assert!((color[0] - 0.1).abs() < 0.01);
        assert!((color[1] - 0.1).abs() < 0.01);
        assert!((color[2] - 0.1).abs() < 0.01);
        assert!((color[3] - 1.0).abs() < 0.01);
    }

    #[test]
    fn theme_light_background_color() {
        let color = Theme::Light.background_color();
        // Light theme: #FFFFFF = [1.0, 1.0, 1.0, 1.0]
        assert!((color[0] - 1.0).abs() < 0.01);
        assert!((color[1] - 1.0).abs() < 0.01);
        assert!((color[2] - 1.0).abs() < 0.01);
        assert!((color[3] - 1.0).abs() < 0.01);
    }

    // ========== Config::default tests ==========

    #[test]
    fn config_default_has_expected_values() {
        let config = Config::default();

        assert_eq!(config.theme, Theme::Dark);
        assert!((config.font_size - 14.0).abs() < 0.01);
        assert_eq!(config.window_gap, 0);
        assert_eq!(config.min_window_height, 50);
        assert_eq!(config.max_window_height, 0); // 0 = unlimited
        assert!((config.scroll_speed - 1.0).abs() < 0.01);
        assert!(config.auto_scroll);
        assert!(config.csd_apps.is_empty());
    }

    #[test]
    fn config_default_keyboard_values() {
        let config = Config::default();

        assert!(config.keyboard.layout.is_empty());
        assert!(config.keyboard.variant.is_empty());
        assert!(config.keyboard.model.is_empty());
        assert!(config.keyboard.options.is_empty());
        assert_eq!(config.keyboard.repeat_delay, 400);
        assert_eq!(config.keyboard.repeat_rate, 25);
    }

    // ========== is_csd_app tests ==========

    #[test]
    fn is_csd_app_exact_match() {
        let mut config = Config::default();
        config.csd_apps = vec!["firefox".to_string(), "chromium".to_string()];

        assert!(config.is_csd_app("firefox"));
        assert!(config.is_csd_app("chromium"));
        assert!(!config.is_csd_app("firefox-esr")); // Not exact match
        assert!(!config.is_csd_app("other-app"));
    }

    #[test]
    fn is_csd_app_prefix_match() {
        let mut config = Config::default();
        config.csd_apps = vec!["org.gnome.*".to_string()];

        assert!(config.is_csd_app("org.gnome.Calculator"));
        assert!(config.is_csd_app("org.gnome.Nautilus"));
        assert!(config.is_csd_app("org.gnome.")); // Empty suffix still matches
        assert!(!config.is_csd_app("org.kde.dolphin"));
        assert!(!config.is_csd_app("gnome-terminal")); // Doesn't start with "org.gnome."
    }

    #[test]
    fn is_csd_app_mixed_patterns() {
        let mut config = Config::default();
        config.csd_apps = vec![
            "firefox".to_string(),
            "org.gnome.*".to_string(),
            "com.github.*".to_string(),
        ];

        assert!(config.is_csd_app("firefox"));
        assert!(config.is_csd_app("org.gnome.Settings"));
        assert!(config.is_csd_app("com.github.SomeApp"));
        assert!(!config.is_csd_app("chromium"));
    }

    #[test]
    fn is_csd_app_empty_list() {
        let config = Config::default();
        assert!(!config.is_csd_app("anything"));
    }

    #[test]
    fn is_csd_app_empty_app_id() {
        let mut config = Config::default();
        config.csd_apps = vec!["*".to_string()]; // Matches everything with prefix

        assert!(config.is_csd_app("")); // Empty prefix matches empty string
        assert!(config.is_csd_app("anything"));
    }

    // ========== TOML roundtrip tests ==========

    #[test]
    fn config_toml_roundtrip() {
        let mut config = Config::default();
        config.theme = Theme::Light;
        config.font_size = 16.0;
        config.window_gap = 4;
        config.csd_apps = vec!["firefox".to_string(), "org.gnome.*".to_string()];
        config.keyboard.layout = "us".to_string();
        config.keyboard.repeat_delay = 300;

        // Serialize
        let toml_str = toml::to_string(&config).expect("Failed to serialize");

        // Deserialize
        let parsed: Config = toml::from_str(&toml_str).expect("Failed to deserialize");

        assert_eq!(parsed.theme, config.theme);
        assert!((parsed.font_size - config.font_size).abs() < 0.01);
        assert_eq!(parsed.window_gap, config.window_gap);
        assert_eq!(parsed.csd_apps, config.csd_apps);
        assert_eq!(parsed.keyboard.layout, config.keyboard.layout);
        assert_eq!(parsed.keyboard.repeat_delay, config.keyboard.repeat_delay);
    }

    #[test]
    fn config_partial_toml_uses_defaults() {
        // Only specify some fields
        let partial_toml = r#"
            theme = "light"
            font_size = 18.0
        "#;

        let parsed: Config = toml::from_str(partial_toml).expect("Failed to parse partial TOML");

        // Specified values
        assert_eq!(parsed.theme, Theme::Light);
        assert!((parsed.font_size - 18.0).abs() < 0.01);

        // Default values
        assert_eq!(parsed.window_gap, 0);
        assert_eq!(parsed.keyboard.repeat_delay, 400);
        assert!(parsed.csd_apps.is_empty());
    }

    #[test]
    fn config_invalid_toml_returns_error() {
        let invalid_toml = "this is not valid { toml [";

        let result: Result<Config, _> = toml::from_str(invalid_toml);
        assert!(result.is_err());
    }

    // ========== apply_theme_defaults tests ==========

    #[test]
    fn apply_theme_defaults_updates_light_theme_background() {
        // When theme is light but background_color is still dark default,
        // apply_theme_defaults should update it
        let mut config = Config::default();
        config.theme = Theme::Light;
        // background_color is still the dark default

        config.apply_theme_defaults();

        // Now background should be light theme color
        let expected = Theme::Light.background_color();
        assert_eq!(config.background_color, expected);
    }

    #[test]
    fn apply_theme_defaults_preserves_custom_background() {
        // If user specified a custom background, don't override it
        let mut config = Config::default();
        config.theme = Theme::Light;
        config.background_color = [0.5, 0.5, 0.5, 1.0]; // Custom gray

        config.apply_theme_defaults();

        // Should still be custom
        assert!((config.background_color[0] - 0.5).abs() < 0.01);
    }
}
