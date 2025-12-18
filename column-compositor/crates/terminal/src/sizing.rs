//! Terminal sizing state machine
//!
//! Explicit state machine for tracking terminal size changes.
//! Key learning from v1: prevents double-counting bugs by only incrementing
//! content rows in the Stable state.

/// Actions that should be taken in response to state changes
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SizingAction {
    /// No action needed
    None,

    /// Request the compositor to grow the window
    RequestGrowth { target_rows: u16 },

    /// Apply the resize to the PTY
    ApplyResize { rows: u16 },

    /// Restore scrollback that accumulated during resize
    RestoreScrollback { lines: u32 },
}

/// Terminal sizing state machine
///
/// LEARNING: Explicit state prevents double-counting bugs from v1.
/// Content rows only increments in Stable state.
#[derive(Debug, Clone)]
pub enum TerminalSizingState {
    /// Terminal is stable at current size
    Stable {
        /// Current number of rows
        rows: u16,
        /// Total content lines produced (visible + scrollback)
        content_rows: u32,
    },

    /// Growth requested, waiting for compositor configure
    GrowthRequested {
        /// Current row count
        current_rows: u16,
        /// Requested row count
        target_rows: u16,
        /// Content rows at time of request
        content_rows: u32,
        /// Lines that scrolled off while waiting (restore later)
        pending_scrollback: u32,
    },

    /// Configure received, applying new size
    Resizing {
        /// Starting row count
        from_rows: u16,
        /// Target row count
        to_rows: u16,
        /// Content rows (frozen during resize)
        content_rows: u32,
        /// Lines that scrolled off during resize
        pending_scrollback: u32,
    },
}

impl TerminalSizingState {
    /// Create initial state
    pub fn new(initial_rows: u16) -> Self {
        Self::Stable {
            rows: initial_rows,
            content_rows: 0,
        }
    }

    /// Handle a new line being added to the terminal
    ///
    /// LEARNING: Single point of truth for content counting.
    /// Only increment in one place, in one state.
    pub fn on_new_line(&mut self) -> SizingAction {
        match self {
            Self::Stable { content_rows, rows } => {
                *content_rows += 1;

                // Check if we need to grow
                if *content_rows > *rows as u32 {
                    SizingAction::RequestGrowth {
                        target_rows: (*content_rows).min(u16::MAX as u32) as u16,
                    }
                } else {
                    SizingAction::None
                }
            }

            Self::GrowthRequested {
                pending_scrollback, ..
            } => {
                // LEARNING: Don't increment content_rows here!
                // Just track that a line scrolled off
                *pending_scrollback += 1;
                SizingAction::None
            }

            Self::Resizing {
                pending_scrollback, ..
            } => {
                *pending_scrollback += 1;
                SizingAction::None
            }
        }
    }

    /// Handle compositor sending a configure event
    pub fn on_configure(&mut self, new_rows: u16) -> SizingAction {
        match self {
            Self::GrowthRequested {
                current_rows,
                content_rows,
                pending_scrollback,
                ..
            } => {
                let scrollback = *pending_scrollback;
                *self = Self::Resizing {
                    from_rows: *current_rows,
                    to_rows: new_rows,
                    content_rows: *content_rows,
                    pending_scrollback: scrollback,
                };
                SizingAction::ApplyResize { rows: new_rows }
            }

            Self::Stable { rows, content_rows } => {
                // Unsolicited resize (e.g., user resized window)
                if new_rows != *rows {
                    *self = Self::Resizing {
                        from_rows: *rows,
                        to_rows: new_rows,
                        content_rows: *content_rows,
                        pending_scrollback: 0,
                    };
                    SizingAction::ApplyResize { rows: new_rows }
                } else {
                    SizingAction::None
                }
            }

            Self::Resizing { to_rows, .. } => {
                // Another configure during resize - update target
                if new_rows != *to_rows {
                    *to_rows = new_rows;
                    SizingAction::ApplyResize { rows: new_rows }
                } else {
                    SizingAction::None
                }
            }
        }
    }

    /// Handle resize completion (PTY acknowledged new size)
    pub fn on_resize_complete(&mut self) -> SizingAction {
        match self {
            Self::Resizing {
                to_rows,
                content_rows,
                pending_scrollback,
                ..
            } => {
                let restore = *pending_scrollback;
                *self = Self::Stable {
                    rows: *to_rows,
                    content_rows: *content_rows,
                };

                if restore > 0 {
                    SizingAction::RestoreScrollback { lines: restore }
                } else {
                    SizingAction::None
                }
            }

            _ => SizingAction::None,
        }
    }

    /// Notify that growth has been requested (transition from Stable)
    pub fn request_growth(&mut self, target_rows: u16) -> SizingAction {
        match self {
            Self::Stable { rows, content_rows } => {
                *self = Self::GrowthRequested {
                    current_rows: *rows,
                    target_rows,
                    content_rows: *content_rows,
                    pending_scrollback: 0,
                };
                SizingAction::None
            }
            _ => SizingAction::None,
        }
    }

    /// Get current row count
    pub fn current_rows(&self) -> u16 {
        match self {
            Self::Stable { rows, .. } => *rows,
            Self::GrowthRequested { current_rows, .. } => *current_rows,
            Self::Resizing { from_rows, .. } => *from_rows,
        }
    }

    /// Get content row count
    pub fn content_rows(&self) -> u32 {
        match self {
            Self::Stable { content_rows, .. } => *content_rows,
            Self::GrowthRequested { content_rows, .. } => *content_rows,
            Self::Resizing { content_rows, .. } => *content_rows,
        }
    }

    /// Check if in stable state
    pub fn is_stable(&self) -> bool {
        matches!(self, Self::Stable { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_stable() {
        let state = TerminalSizingState::new(24);
        assert!(state.is_stable());
        assert_eq!(state.current_rows(), 24);
        assert_eq!(state.content_rows(), 0);
    }

    #[test]
    fn content_rows_increment_in_stable() {
        let mut state = TerminalSizingState::new(24);

        for i in 1..=10 {
            let action = state.on_new_line();
            assert_eq!(state.content_rows(), i);
            assert_eq!(action, SizingAction::None);
        }
    }

    #[test]
    fn growth_requested_when_exceeds_rows() {
        let mut state = TerminalSizingState::new(5);

        for _ in 0..5 {
            state.on_new_line();
        }

        // 6th line should trigger growth
        let action = state.on_new_line();
        assert_eq!(
            action,
            SizingAction::RequestGrowth { target_rows: 6 }
        );
    }

    #[test]
    fn no_double_counting_during_growth_request() {
        let mut state = TerminalSizingState::new(5);

        // Fill to capacity
        for _ in 0..5 {
            state.on_new_line();
        }

        // Trigger growth
        state.on_new_line();
        assert_eq!(state.content_rows(), 6);

        // Transition to growth requested
        state.request_growth(10);

        // New lines during growth request don't increment content_rows
        state.on_new_line();
        state.on_new_line();
        assert_eq!(state.content_rows(), 6); // Still 6!

        // Pending scrollback should track the lines
        match &state {
            TerminalSizingState::GrowthRequested {
                pending_scrollback, ..
            } => {
                assert_eq!(*pending_scrollback, 2);
            }
            _ => panic!("wrong state"),
        }
    }

    #[test]
    fn scrollback_restored_after_resize() {
        let mut state = TerminalSizingState::new(5);

        // Fill and trigger growth
        for _ in 0..6 {
            state.on_new_line();
        }
        state.request_growth(10);

        // Add lines during resize
        state.on_new_line();
        state.on_new_line();

        // Configure arrives
        let action = state.on_configure(10);
        assert_eq!(action, SizingAction::ApplyResize { rows: 10 });

        // Complete resize
        let action = state.on_resize_complete();
        assert_eq!(action, SizingAction::RestoreScrollback { lines: 2 });

        // Back to stable
        assert!(state.is_stable());
        assert_eq!(state.current_rows(), 10);
    }

    #[test]
    fn content_monotonic_in_stable() {
        let mut state = TerminalSizingState::new(100);
        let mut last = 0;

        for _ in 0..50 {
            state.on_new_line();
            let current = state.content_rows();
            assert!(current >= last);
            assert!(current <= last + 1);
            last = current;
        }
    }
}
