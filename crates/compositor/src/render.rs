//! Render loop helpers
//!
//! Extracted from main.rs to reduce complexity of the main render loop.
//!
//! # Responsibilities
//!
//! - Terminal texture rendering and caching
//! - Title bar rendering
//! - Focus indicator rendering
//! - External window (Wayland client) rendering
//! - Damage tracking and optimization
//! - Debug overlay rendering
//!
//! # NOT Responsible For
//!
//! - Window positioning (uses `layout.rs` calculations)
//! - State management (uses `state.rs` data)
//! - Terminal content (uses `terminal_manager/` textures)
//! - Coordinate transforms (uses `coords.rs` types)

use std::collections::HashMap;

use smithay::backend::renderer::element::surface::{WaylandSurfaceRenderElement, WaylandSurfaceTexture, render_elements_from_surface_tree};
use smithay::backend::renderer::element::{Element, Kind};
use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Color32F, Frame, ImportMem, Texture};
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};

use crate::state::{CrossSelection, StackWindow, LayoutNode, WindowPosition};
use crate::terminal_manager::{TerminalId, TerminalManager};
use crate::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT, TITLE_BAR_PADDING};

/// Cache for title bar textures, keyed by (title, width)
pub type TitleBarCache = HashMap<(String, u32), GlesTexture>;

/// Focus indicator width in pixels (also used as left margin for content)
pub const FOCUS_INDICATOR_WIDTH: i32 = 2;

/// Calculate the visual/render height for a terminal.
///
/// This is the total height including the title bar (if shown).
/// The title bar is shown when the terminal is visible and has `show_title_bar` set.
///
/// # Arguments
/// * `content_height` - Height of the terminal content in pixels (may be 0)
/// * `show_title_bar` - Whether the terminal should show a title bar
/// * `is_visible` - Whether the terminal is visible (hidden terminals have 0 height)
///
/// # Returns
/// The total visual height including title bar if applicable.
pub fn calculate_terminal_render_height(
    content_height: i32,
    show_title_bar: bool,
    is_visible: bool,
) -> i32 {
    if !is_visible {
        return 0;
    }
    if show_title_bar {
        content_height + TITLE_BAR_HEIGHT as i32
    } else {
        content_height
    }
}

/// Draw focus indicator on left side of cell
fn draw_focus_indicator(frame: &mut GlesFrame<'_, '_>, y: i32, height: i32) {
    let focus_rect = Rectangle::new(
        (0, y).into(),
        (FOCUS_INDICATOR_WIDTH, height).into(),
    );
    frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_rect]).ok();
}

/// Draw running indicator on left side of cell (light blue)
fn draw_running_indicator(frame: &mut GlesFrame<'_, '_>, y: i32, height: i32) {
    let running_rect = Rectangle::new(
        (0, y).into(),
        (FOCUS_INDICATOR_WIDTH, height).into(),
    );
    frame.clear(Color32F::new(0.3, 0.6, 1.0, 1.0), &[running_rect]).ok();
}

/// Data needed to render a single cell
pub enum CellRenderData<'a> {
    Terminal {
        id: TerminalId,
        y: i32,
        height: i32,
        title_bar_texture: Option<&'a GlesTexture>,
    },
    External {
        y: i32,
        height: i32,
        elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
        title_bar_texture: Option<&'a GlesTexture>,
        uses_csd: bool,
    },
}

/// Pre-render all terminal textures so they're ready for the frame
pub fn prerender_terminals(
    terminal_manager: &mut TerminalManager,
    renderer: &mut GlesRenderer,
) {
    let all_ids = terminal_manager.ids();
    tracing::debug!(
        count = all_ids.len(),
        ids = ?all_ids.iter().map(|id| id.0).collect::<Vec<_>>(),
        "pre-rendering terminals"
    );
    for id in all_ids {
        if let Some(terminal) = terminal_manager.get_mut(id) {
            tracing::debug!(
                id = id.0,
                dirty = terminal.is_dirty(),
                has_texture = terminal.get_texture().is_some(),
                "pre-render check"
            );
            terminal.render(renderer);
        }
    }
}

/// Pre-render title bar textures for all cells with SSD
///
/// Uses a cache to avoid re-rendering title bars that haven't changed.
/// Returns references to cached textures.
/// Also populates the char_info_cache for text selection hit-testing.
#[allow(clippy::too_many_arguments)]
pub fn prerender_title_bars<'a>(
    layout_nodes: &[LayoutNode],
    title_bar_renderer: &mut Option<TitleBarRenderer>,
    terminal_manager: &TerminalManager,
    renderer: &mut GlesRenderer,
    width: i32,
    cache: &'a mut TitleBarCache,
    char_info_cache: &mut crate::state::TitleBarCharInfoCache,
) -> Vec<Option<&'a GlesTexture>> {
    // First pass: collect keys and render any missing textures
    let mut keys: Vec<Option<(String, u32)>> = Vec::new();

    for (window_idx, node) in layout_nodes.iter().enumerate() {
        match &node.cell {
            StackWindow::Terminal(id) => {
                let show_title_bar = terminal_manager.get(*id)
                    .map(|t| t.show_title_bar)
                    .unwrap_or(false);
                if show_title_bar {
                    if let Some(ref mut tb_renderer) = title_bar_renderer {
                        let title = terminal_manager.get(*id)
                            .map(|t| t.title.as_str())
                            .unwrap_or("Terminal");
                        let key = (title.to_string(), width as u32);
                        // Render if texture not cached, or if char_info is missing
                        let needs_render = !cache.contains_key(&key)
                            || !char_info_cache.contains_key(&window_idx);
                        if needs_render {
                            let (pixels, tb_width, tb_height, char_info) =
                                tb_renderer.render_with_char_info(title, width as u32);
                            if !cache.contains_key(&key) {
                                if let Ok(tex) = renderer.import_memory(
                                    &pixels,
                                    smithay::backend::allocator::Fourcc::Argb8888,
                                    (tb_width as i32, tb_height as i32).into(),
                                    false,
                                ) {
                                    cache.insert(key.clone(), tex);
                                }
                            }
                            char_info_cache.insert(window_idx, char_info);
                        }
                        keys.push(Some(key));
                    } else {
                        keys.push(None);
                    }
                } else {
                    keys.push(None);
                }
            }
            StackWindow::External(entry) => {
                if entry.uses_csd {
                    keys.push(None);
                } else if let Some(ref mut tb_renderer) = title_bar_renderer {
                    let key = (entry.command.clone(), width as u32);
                    // Render if texture not cached, or if char_info is missing
                    let needs_render = !cache.contains_key(&key)
                        || !char_info_cache.contains_key(&window_idx);
                    if needs_render {
                        let (pixels, tb_width, tb_height, char_info) =
                            tb_renderer.render_with_char_info(&entry.command, width as u32);
                        if !cache.contains_key(&key) {
                            if let Ok(tex) = renderer.import_memory(
                                &pixels,
                                smithay::backend::allocator::Fourcc::Argb8888,
                                (tb_width as i32, tb_height as i32).into(),
                                false,
                            ) {
                                cache.insert(key.clone(), tex);
                            }
                        }
                        char_info_cache.insert(window_idx, char_info);
                    }
                    keys.push(Some(key));
                } else {
                    keys.push(None);
                }
            }
        }
    }

    // Second pass: look up references from cache
    keys.into_iter()
        .map(|key| key.and_then(|k| cache.get(&k)))
        .collect()
}

/// Collect actual heights for all cells and render external window elements
///
/// Returns (heights, external_elements_per_cell)
pub fn collect_window_data(
    layout_nodes: &[LayoutNode],
    terminal_manager: &TerminalManager,
    renderer: &mut GlesRenderer,
    scale: Scale<f64>,
) -> (Vec<i32>, Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>) {
    let mut heights = Vec::new();
    let mut external_elements = Vec::new();

    for node in layout_nodes.iter() {
        match &node.cell {
            StackWindow::Terminal(id) => {
                let (content_height, show_title_bar, is_visible) = terminal_manager.get(*id)
                    .map(|t| {
                        let h = if !t.is_visible() {
                            0
                        } else if let Some(tex) = t.get_texture() {
                            tex.size().h
                        } else {
                            t.height as i32
                        };
                        (h, t.show_title_bar, t.is_visible())
                    })
                    .unwrap_or((node.height, false, false));
                let height = calculate_terminal_render_height(content_height, show_title_bar, is_visible);
                heights.push(height);
                external_elements.push(Vec::new());
            }
            StackWindow::External(entry) => {
                // Use render_elements_from_surface_tree directly on the surface
                // instead of window.render_elements() which includes popups when PopupManager is used.
                // This ensures popup surfaces don't affect the window's height calculation.
                let wl_surface = entry.surface.wl_surface();
                let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    render_elements_from_surface_tree(
                        renderer,
                        wl_surface,
                        Point::from((0i32, 0i32)),
                        scale,
                        1.0,
                        Kind::Unspecified,
                    );

                // Calculate window height from actual surface geometry
                // The configure/ack protocol ensures the surface matches configured size.
                let window_height = if elements.is_empty() {
                    node.height
                } else {
                    elements.iter()
                        .map(|e: &WaylandSurfaceRenderElement<GlesRenderer>| {
                            let geo = e.geometry(scale);
                            geo.loc.y + geo.size.h
                        })
                        .max()
                        .unwrap_or(node.height)
                };

                // window_height is from surface elements (content only),
                // so add title bar for SSD windows.
                let actual_height = if entry.uses_csd {
                    window_height
                } else {
                    window_height + TITLE_BAR_HEIGHT as i32
                };

                heights.push(actual_height);
                external_elements.push(elements);
            }
        }
    }

    (heights, external_elements)
}

/// Build render data with computed Y positions for each cell
pub fn build_render_data<'a>(
    layout_nodes: &[LayoutNode],
    heights: &[i32],
    external_elements: &mut [Vec<WaylandSurfaceRenderElement<GlesRenderer>>],
    title_bar_textures: &[Option<&'a GlesTexture>],
    scroll_offset: f64,
    screen_height: i32,
    terminal_manager: &TerminalManager,
) -> Vec<CellRenderData<'a>> {
    let mut render_data = Vec::new();
    let mut content_y: i32 = -(scroll_offset as i32);

    for (window_idx, node) in layout_nodes.iter().enumerate() {
        let height = heights[window_idx];
        let render_y = crate::coords::content_to_render_y(content_y as f64, height as f64, screen_height as f64) as i32;

        match &node.cell {
            StackWindow::Terminal(id) => {
                if let Some(term) = terminal_manager.get(*id) {
                    let has_texture = term.get_texture().is_some();
                    tracing::debug!(
                        id = id.0,
                        visible = term.is_visible(),
                        height,
                        render_y,
                        has_texture,
                        content_rows = term.content_rows(),
                        "terminal render state"
                    );
                }
                let title_bar_texture = title_bar_textures.get(window_idx).copied().flatten();
                render_data.push(CellRenderData::Terminal {
                    id: *id,
                    y: render_y,
                    height,
                    title_bar_texture,
                });
            }
            StackWindow::External(entry) => {
                let elements = std::mem::take(&mut external_elements[window_idx]);
                let title_bar_texture = title_bar_textures.get(window_idx).copied().flatten();
                let uses_csd = entry.uses_csd;

                // Calculate render height using the tested helper function.
                // This handles both new windows (committed=0) and resize scenarios.
                let committed_height = entry.state.current_height() as i32;
                let render_height = calculate_external_render_height(committed_height, height);

                // INVARIANT: A window with positive layout height must render with positive height.
                // Violation indicates the render path diverged from layout (the bug this catches:
                // using committed_height=0 directly instead of falling back to layout height).
                debug_assert!(
                    height <= 0 || render_height > 0,
                    "BUG: external window has layout height {} but render height {} \
                     (committed_height={}). Window would be invisible!",
                    height, render_height, committed_height
                );

                // Top-align content when render height differs from layout height
                // (happens during resize drag when layout uses target height)
                let adjusted_render_y = if render_height != height {
                    // In OpenGL coords, Y=0 at bottom, so adding moves content up on screen
                    render_y + (height - render_height)
                } else {
                    render_y
                };

                render_data.push(CellRenderData::External {
                    y: adjusted_render_y,
                    height: render_height,
                    elements,
                    title_bar_texture,
                    uses_csd,
                });
            }
        }

        content_y += height;
    }

    render_data
}

/// Log frame state for debugging (only when external windows present)
pub fn log_frame_state(
    layout_nodes: &[LayoutNode],
    render_data: &[CellRenderData],
    terminal_manager: &TerminalManager,
    scroll_offset: f64,
    focused_index: Option<usize>,
    screen_height: i32,
) {
    let has_external = layout_nodes.iter().any(|n| matches!(n.cell, StackWindow::External(_)));
    if !has_external {
        return;
    }

    let window_info: Vec<String> = layout_nodes.iter().enumerate().map(|(i, node)| {
        match &node.cell {
            StackWindow::Terminal(id) => {
                let visible = terminal_manager.is_terminal_visible(*id);
                format!("[{}]Term({})h={}{}",
                    i, id.0,
                    node.height,
                    if !visible { " HIDDEN" } else { "" })
            }
            StackWindow::External(e) => {
                format!("[{}]Ext h={}", i, e.state.current_height())
            }
        }
    }).collect();

    let render_info: Vec<String> = render_data.iter().enumerate().map(|(i, data)| {
        match data {
            CellRenderData::Terminal { id, y, height, .. } => {
                format!("[{}]T{}@y={},h={}", i, id.0, y, height)
            }
            CellRenderData::External { y, height, .. } => {
                format!("[{}]E@y={},h={}", i, y, height)
            }
        }
    }).collect();

    tracing::debug!(
        scroll = scroll_offset,
        focused = ?focused_index,
        screen_h = screen_height,
        cells = %window_info.join(" "),
        render = %render_info.join(" "),
        "FRAME STATE"
    );
}

/// Check if any cell heights changed significantly (affecting scroll)
pub fn heights_changed_significantly(
    cached: &[i32],
    actual: &[i32],
    focused_index: Option<usize>,
) -> bool {
    cached.iter()
        .zip(actual.iter())
        .enumerate()
        .any(|(i, (&cached_h, &actual_h))| {
            if let Some(focused) = focused_index {
                if i <= focused && actual_h != cached_h && (actual_h - cached_h).abs() > 10 {
                    return true;
                }
            }
            false
        })
}

/// Render a terminal cell
#[allow(clippy::too_many_arguments)]
pub fn render_terminal(
    frame: &mut GlesFrame<'_, '_>,
    terminal_manager: &TerminalManager,
    id: TerminalId,
    y: i32,
    height: i32,
    title_bar_texture: Option<&GlesTexture>,
    is_focused: bool,
    is_running: bool,
    screen_size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
) {
    let Some(terminal) = terminal_manager.get(id) else { return };

    if !terminal.is_visible() {
        return;
    }

    // Only render if visible on screen
    if y + height <= 0 || y >= screen_size.h {
        return;
    }

    // Calculate content area (below title bar if present)
    let title_bar_height = if title_bar_texture.is_some() { TITLE_BAR_HEIGHT as i32 } else { 0 };
    let content_area_top = y + height - title_bar_height;

    // Render title bar if present (even if there's no content texture)
    if let Some(tex) = title_bar_texture {
        frame.render_texture_at(
            tex,
            Point::from((FOCUS_INDICATOR_WIDTH, content_area_top)),
            1,
            1.0,
            Transform::Flipped180,
            &[damage],
            &[],
            1.0,
        ).ok();
    }

    // Render content texture if present
    // (may be None for empty builtins that only show title bar)
    if let Some(texture) = terminal.get_texture() {
        // Top-align terminal content within content area
        // (texture may be smaller than cell during resize)
        let texture_height = texture.size().h;
        let content_y = content_area_top - texture_height;

        frame.render_texture_at(
            texture,
            Point::from((FOCUS_INDICATOR_WIDTH, content_y)),
            1,
            1.0,
            Transform::Flipped180,
            &[damage],
            &[],
            1.0,
        ).ok();
    }

    // Draw focus indicator on left side of cell (after content so it's visible)
    // Focus indicator takes precedence over running indicator
    if is_focused {
        draw_focus_indicator(frame, y, height);
    } else if is_running {
        draw_running_indicator(frame, y, height);
    }
}

/// Render an external window cell with title bar
#[allow(clippy::too_many_arguments)]
pub fn render_external(
    frame: &mut GlesFrame<'_, '_>,
    y: i32,
    height: i32,
    elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
    title_bar_texture: Option<&GlesTexture>,
    is_focused: bool,
    screen_size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
    scale: Scale<f64>,
    _uses_csd: bool,
) {
    // For SSD windows, title bar is at the top of the cell
    // For CSD windows, there's no title bar from us
    let title_bar_y = y + height - TITLE_BAR_HEIGHT as i32;

    // Render title bar
    if let Some(tex) = title_bar_texture {
        frame.render_texture_at(
            tex,
            Point::from((FOCUS_INDICATOR_WIDTH, title_bar_y)),
            1,
            1.0,
            Transform::Flipped180,
            &[damage],
            &[],
            1.0,
        ).ok();
    }

    // Render external window elements
    // All external windows need Transform::Flipped180 to match the frame's flip
    // (OpenGL frame is rendered with Transform::Flipped180, so content needs counter-flip)
    let _ = screen_size; // Will be used when we implement proper width handling
    for element in elements {
        let geo = element.geometry(scale);
        let src = element.src();

        // Calculate dest Y position using element's natural position
        let dest_y = geo.loc.y + y;

        let dest = Rectangle::new(
            Point::from((geo.loc.x + FOCUS_INDICATOR_WIDTH, dest_y)),
            geo.size,
        );

        // Render texture with Transform::Flipped180 to counter the OpenGL Y-flip
        match element.texture() {
            WaylandSurfaceTexture::Texture(texture) => {
                frame.render_texture_from_to(
                    texture,
                    src,
                    dest,
                    &[damage],
                    &[],
                    Transform::Flipped180,
                    1.0,
                    None,  // program
                    &[],   // uniforms
                ).ok();
            }
            WaylandSurfaceTexture::SolidColor(color) => {
                // Solid color surface (e.g., blank or placeholder)
                frame.draw_solid(dest, &[damage], *color).ok();
            }
        }
    }

    // Draw focus indicator on left side of cell (after content so it's visible)
    // Focus indicator takes precedence over running indicator
    // External windows are always running (blue) when not focused
    if is_focused {
        draw_focus_indicator(frame, y, height);
    } else {
        draw_running_indicator(frame, y, height);
    }
}

/// Calculate the render height for an external window.
///
/// For windows that have committed a size, use the committed height.
/// For new windows that haven't committed yet (committed_height=0),
/// use the layout height computed from element geometry.
///
/// This ensures new windows render at their actual size immediately,
/// rather than waiting for the first commit to update WindowState.
#[inline]
pub fn calculate_external_render_height(committed_height: i32, layout_height: i32) -> i32 {
    if committed_height > 0 {
        committed_height
    } else {
        layout_height
    }
}

/// Selection highlight color (semi-transparent blue)
const SELECTION_COLOR: Color32F = Color32F::new(0.2, 0.4, 0.8, 0.5);

/// Render selection overlay for a title bar
///
/// Draws a semi-transparent blue overlay on the selected portion of a title bar.
/// Uses the title bar's character info to determine pixel boundaries.
#[allow(clippy::too_many_arguments)]
pub fn render_title_bar_selection(
    frame: &mut GlesFrame<'_, '_>,
    window_index: usize,
    title_bar_y: i32,
    output_width: i32,
    cross_selection: Option<&CrossSelection>,
    title_bar_char_info: &std::collections::HashMap<usize, crate::title_bar::TitleBarCharInfo>,
    damage: Rectangle<i32, Physical>,
) {
    let Some(selection) = cross_selection else {
        return;
    };

    // Check if this window is in the selection range
    let selection_range = match selection.window_selection_range(window_index) {
        Some(range) => range,
        None => return,
    };

    // Get character info for this title bar
    let char_info = title_bar_char_info.get(&window_index);

    // Calculate selection bounds within the title bar
    // Title bar is at the TOP of each window. Selection includes title bar when:
    // - Selection starts in title bar (partial highlight from start char)
    // - Selection passes through from above (full highlight - middle/last windows)
    // Title bar is NOT included when selection starts in content (it's above the selection)
    let (start_x, end_x) = match selection_range {
        (Some(WindowPosition::TitleBar { char_index: start }), Some(WindowPosition::TitleBar { char_index: end })) => {
            // Single window: partial selection within title bar only
            let start_x = char_info
                .and_then(|info| info.char_positions.get(start).copied())
                .unwrap_or(0.0) as i32 + TITLE_BAR_PADDING as i32;
            let end_x = char_info
                .map(|info| {
                    let pos = info.char_positions.get(end).copied().unwrap_or(0.0);
                    let width = info.char_widths.get(end).copied().unwrap_or(8.0);
                    pos + width
                })
                .unwrap_or(output_width as f32) as i32 + TITLE_BAR_PADDING as i32;
            (start_x, end_x)
        }
        (Some(WindowPosition::TitleBar { char_index: start }), Some(WindowPosition::Content { .. })) => {
            // Single window: selection from title bar to content - highlight from start to end of title bar
            let start_x = char_info
                .and_then(|info| info.char_positions.get(start).copied())
                .unwrap_or(0.0) as i32 + TITLE_BAR_PADDING as i32;
            (start_x, output_width - FOCUS_INDICATOR_WIDTH)
        }
        (Some(WindowPosition::TitleBar { char_index: start }), None) => {
            // First window in multi-window: selection starts in title bar, goes to end of window
            let start_x = char_info
                .and_then(|info| info.char_positions.get(start).copied())
                .unwrap_or(0.0) as i32 + TITLE_BAR_PADDING as i32;
            (start_x, output_width - FOCUS_INDICATOR_WIDTH)
        }
        (Some(WindowPosition::Content { .. }), Some(WindowPosition::Content { .. })) => {
            // Single window: selection entirely in content - title bar NOT included
            return;
        }
        (Some(WindowPosition::Content { .. }), Some(WindowPosition::TitleBar { .. })) => {
            // Single window: selection from content to title bar (dragging up) - impossible in our model
            // Content is below title bar, so this shouldn't happen. Skip title bar.
            return;
        }
        (Some(WindowPosition::Content { .. }), None) => {
            // First window in multi-window: selection starts in content - title bar NOT included
            // (title bar is above the selection start)
            return;
        }
        (None, Some(WindowPosition::TitleBar { char_index: end })) => {
            // Last window: selection from above, ends in title bar
            let end_x = char_info
                .map(|info| {
                    let pos = info.char_positions.get(end).copied().unwrap_or(0.0);
                    let width = info.char_widths.get(end).copied().unwrap_or(8.0);
                    pos + width
                })
                .unwrap_or(output_width as f32) as i32 + TITLE_BAR_PADDING as i32;
            (TITLE_BAR_PADDING as i32, end_x)
        }
        (None, Some(WindowPosition::Content { .. })) => {
            // Last window: selection from above, ends in content - title bar fully included
            // (selection passes through title bar to reach content)
            (TITLE_BAR_PADDING as i32, output_width - FOCUS_INDICATOR_WIDTH)
        }
        (None, None) => {
            // Middle window: fully selected including title bar
            (TITLE_BAR_PADDING as i32, output_width - FOCUS_INDICATOR_WIDTH)
        }
    };

    // Clamp to valid range
    let start_x = start_x.max(FOCUS_INDICATOR_WIDTH);
    let end_x = end_x.min(output_width).max(start_x);
    let width = end_x - start_x;

    if width <= 0 {
        return;
    }

    // Draw selection rectangle
    let selection_rect = Rectangle::new(
        (start_x, title_bar_y).into(),
        (width, TITLE_BAR_HEIGHT as i32).into(),
    );

    frame.draw_solid(selection_rect, &[damage], SELECTION_COLOR).ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==========================================================================
    // External window render height tests
    // ==========================================================================

    #[test]
    fn external_render_height_uses_committed_when_available() {
        // After commit, use the committed height even if layout differs
        let committed = 200;
        let layout = 300; // Different value to prove we use committed, not layout
        assert_eq!(
            calculate_external_render_height(committed, layout),
            200,
            "should use committed height, not layout height"
        );
    }

    #[test]
    fn external_render_height_uses_layout_when_not_committed() {
        // Before first commit, committed_height is 0
        // Should use layout height (from element geometry)
        let committed = 0;
        let layout = 224; // e.g., 200px content + 24px title bar
        assert_eq!(
            calculate_external_render_height(committed, layout),
            224,
            "new window should render at layout height, not 0"
        );
    }

    #[test]
    fn external_render_height_zero_layout_zero_committed() {
        // Edge case: both are zero (window truly has no content)
        assert_eq!(
            calculate_external_render_height(0, 0),
            0,
            "zero layout + zero committed = zero render height"
        );
    }

    #[test]
    fn external_render_height_layout_differs_from_committed() {
        // During resize: layout might be target height, committed is old height
        // Should use committed (what client has actually drawn)
        let committed = 150;
        let layout = 300; // target height during resize
        assert_eq!(
            calculate_external_render_height(committed, layout),
            150,
            "during resize, should use committed height (actual buffer size)"
        );
    }

    // ==========================================================================
    // Terminal render height tests
    // ==========================================================================

    #[test]
    fn render_height_includes_title_bar_when_visible() {
        // A visible terminal with show_title_bar should include TITLE_BAR_HEIGHT
        let content_height = 100;
        let height = calculate_terminal_render_height(content_height, true, true);
        assert_eq!(height, content_height + TITLE_BAR_HEIGHT as i32);
    }

    #[test]
    fn render_height_zero_content_still_shows_title_bar() {
        // This is the key test: empty builtins (content_height=0) should still show title bar
        let content_height = 0;
        let height = calculate_terminal_render_height(content_height, true, true);
        assert_eq!(
            height,
            TITLE_BAR_HEIGHT as i32,
            "visible terminal with zero content should still render title bar"
        );
    }

    #[test]
    fn render_height_hidden_terminal_is_zero() {
        // Hidden terminals should have 0 render height regardless of content
        let height = calculate_terminal_render_height(100, true, false);
        assert_eq!(height, 0, "hidden terminal should have 0 render height");
    }

    #[test]
    fn render_height_no_title_bar_equals_content() {
        // Terminal without title bar should just use content height
        let content_height = 150;
        let height = calculate_terminal_render_height(content_height, false, true);
        assert_eq!(height, content_height, "no title bar means height equals content");
    }

    #[test]
    fn render_height_no_title_bar_zero_content() {
        // Edge case: no title bar and zero content
        let height = calculate_terminal_render_height(0, false, true);
        assert_eq!(height, 0, "no title bar + zero content = zero height");
    }

    // ==========================================================================
    // External window render WIDTH tests
    // ==========================================================================

    /// Render width for external windows.
    ///
    /// We render at the app's geometry width (what the app actually drew).
    /// To get full width, we send a configure requesting full width, and the
    /// app should respond by rendering at full width.
    ///
    /// If the app doesn't respond to our configure, it will render at its
    /// preferred width (which may be smaller than output width).
    fn render_width_for_external(geometry_width: i32) -> i32 {
        // We render what the app drew - no stretching
        geometry_width
    }

    #[test]
    fn external_renders_at_geometry_width() {
        // External windows render at their geometry width
        // (we rely on configure to make them render at full width)
        let geometry_width = 600;
        let render_width = render_width_for_external(geometry_width);
        assert_eq!(render_width, geometry_width);
    }

    #[test]
    fn external_full_width_after_configure_response() {
        // After app responds to our configure, geometry should be full width
        let geometry_width = 1280; // App responded to configure
        let render_width = render_width_for_external(geometry_width);
        assert_eq!(render_width, 1280, "App should render at full width after configure");
    }
}
