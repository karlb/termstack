//! Tests for initial external window sizing
//!
//! These tests verify the initial configure for external windows.
//!
//! CURRENT APPROACH:
//! In new_toplevel, we:
//! - Set state.bounds = Some(output_size) <- tells app max space
//! - Do NOT set state.size               <- lets app use its preferred size
//!
//! On first commit, we enforce width while keeping the app's height.
//! This is more compatible than setting size=(width, 0) which some apps
//! interpret as "use 0 height".

#[cfg(test)]
mod tests {
    use smithay::utils::{Physical, Size};
    use crate::state::initial_configure_bounds;

    /// Test: initial configure bounds match output size
    ///
    /// Bounds tell apps the maximum available space.
    #[test]
    fn initial_configure_bounds_match_output() {
        let output_size: Size<i32, Physical> = Size::from((1920, 1080));

        let bounds = initial_configure_bounds(output_size);

        assert_eq!(bounds.w, 1920, "Bounds width should match output");
        assert_eq!(bounds.h, 1080, "Bounds height should match output");
    }

    /// Test: bounds function returns full output size
    ///
    /// This is used by new_toplevel to set state.bounds.
    /// Apps can use any size up to this maximum.
    #[test]
    fn bounds_are_full_output_size() {
        let output_size: Size<i32, Physical> = Size::from((2560, 1440));
        let bounds = initial_configure_bounds(output_size);

        // Bounds = full output, apps can use less
        assert_eq!(bounds.w, 2560);
        assert_eq!(bounds.h, 1440);
    }
}
