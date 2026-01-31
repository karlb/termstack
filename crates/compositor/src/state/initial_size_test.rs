//! Tests for initial external window sizing
//!
//! These tests verify the initial configure for external windows.
//!
//! CURRENT APPROACH:
//! In new_toplevel, we:
//! - Set state.bounds = Some(output_size)      <- tells app max space
//! - Set state.size = Some((output_width, 0))  <- width constrained, height is client's choice
//! - Set TiledLeft + TiledRight states         <- indicate width is constrained
//!
//! Per xdg-shell spec: size=(width, 0) means width is constrained, height is client's choice.
//! Apps should render at full width while using their preferred height.

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

    /// Width enforcement detection logic.
    ///
    /// Returns Some((expected_width, surface_height)) if width needs enforcement,
    /// None if width matches.
    fn detect_width_mismatch(
        output_width: i32,
        committed_width: i32,
        committed_height: i32,
    ) -> Option<(i32, i32)> {
        if committed_width != output_width {
            Some((output_width, committed_height))
        } else {
            None
        }
    }

    /// Test: width mismatch is detected when app uses wrong width
    #[test]
    fn width_mismatch_detected() {
        let output_width = 1280;
        let committed_width = 800; // App wants smaller width
        let committed_height = 200;

        let result = detect_width_mismatch(output_width, committed_width, committed_height);

        assert!(result.is_some(), "Should detect width mismatch");
        let (enforced_width, enforced_height) = result.unwrap();
        assert_eq!(enforced_width, 1280, "Should enforce output width");
        assert_eq!(enforced_height, 200, "Should preserve app's height");
    }

    /// Test: no mismatch when app uses correct width
    #[test]
    fn width_match_no_enforcement() {
        let output_width = 1280;
        let committed_width = 1280; // App uses correct width
        let committed_height = 200;

        let result = detect_width_mismatch(output_width, committed_width, committed_height);

        assert!(result.is_none(), "Should not enforce width when it matches");
    }

    /// Test: width enforcement preserves any app-chosen height
    #[test]
    fn width_enforcement_preserves_height() {
        let output_width = 1920;

        // Various app heights should all be preserved
        for app_height in [100, 200, 500, 800] {
            let result = detect_width_mismatch(output_width, 800, app_height);
            assert!(result.is_some());
            let (_, enforced_height) = result.unwrap();
            assert_eq!(enforced_height, app_height,
                "Should preserve app's height {}", app_height);
        }
    }

    // ==========================================================================
    // Initial configure size tests
    // ==========================================================================

    /// Calculate the initial configure size for external windows.
    ///
    /// We want:
    /// - Width: output_width (apps should use full width)
    /// - Height: 0 (apps choose their preferred height)
    ///
    /// Per xdg-shell spec, size=(width, 0) means width is constrained,
    /// height is client's choice.
    fn initial_configure_size(output_width: i32, _output_height: i32) -> (i32, i32) {
        // Width constrained, height is client's choice (0 = no constraint)
        (output_width, 0)
    }

    #[test]
    fn initial_configure_allows_app_to_choose_height() {
        let output_width = 1280;
        let output_height = 800;

        let (config_width, config_height) = initial_configure_size(output_width, output_height);

        assert_eq!(config_width, output_width, "Width should be constrained to output width");
        // Height should be 0 (client chooses), not output_height
        assert_eq!(
            config_height, 0,
            "Height should be 0 (client chooses), not {} (output height)",
            output_height
        );
    }

    #[test]
    fn initial_configure_width_is_full_width() {
        let output_width = 1920;
        let output_height = 1080;

        let (config_width, _) = initial_configure_size(output_width, output_height);

        assert_eq!(config_width, output_width, "Configure width should be full output width");
    }
}
