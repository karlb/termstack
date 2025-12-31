//! Terminal window implementation using alacritty_terminal
//!
//! This crate provides content-aware terminal windows that can report
//! their content height and request dynamic resizing.

pub mod pty;
pub mod render;
pub mod sizing;
pub mod state;

pub use render::Theme;
pub use sizing::TerminalSizingState;
pub use state::Terminal;
