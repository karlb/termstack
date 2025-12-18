//! Runtime configuration

use serde::{Deserialize, Serialize};

/// Compositor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Background color (ARGB)
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            background_color: [0.1, 0.1, 0.1, 1.0],
            window_gap: 0,
            min_window_height: 50,
            max_window_height: 0,
            scroll_speed: 1.0,
            auto_scroll: true,
            keyboard: KeyboardConfig::default(),
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
            dirs::config_dir().map(|p| p.join("column-compositor/config.toml")),
            Some(std::path::PathBuf::from("/etc/column-compositor/config.toml")),
        ];

        for path in config_paths.into_iter().flatten() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            tracing::info!(?path, "loaded configuration");
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
