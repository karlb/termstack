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

use crate::state::{StackWindow, LayoutNode};
use crate::terminal_manager::{TerminalId, TerminalManager};
use crate::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Cache for title bar textures, keyed by (title, width)
pub type TitleBarCache = HashMap<(String, u32), GlesTexture>;

/// Focus indicator width in pixels (also used as left margin for content)
pub const FOCUS_INDICATOR_WIDTH: i32 = 2;

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
pub fn prerender_title_bars<'a>(
    layout_nodes: &[LayoutNode],
    title_bar_renderer: &mut Option<TitleBarRenderer>,
    terminal_manager: &TerminalManager,
    renderer: &mut GlesRenderer,
    width: i32,
    cache: &'a mut TitleBarCache,
) -> Vec<Option<&'a GlesTexture>> {
    // First pass: collect keys and render any missing textures
    let mut keys: Vec<Option<(String, u32)>> = Vec::new();

    for node in layout_nodes.iter() {
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
                        if !cache.contains_key(&key) {
                            let (pixels, tb_width, tb_height) = tb_renderer.render(title, width as u32);
                            if let Ok(tex) = renderer.import_memory(
                                &pixels,
                                smithay::backend::allocator::Fourcc::Argb8888,
                                (tb_width as i32, tb_height as i32).into(),
                                false,
                            ) {
                                cache.insert(key.clone(), tex);
                            }
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
                    if !cache.contains_key(&key) {
                        let (pixels, tb_width, tb_height) = tb_renderer.render(&entry.command, width as u32);
                        if let Ok(tex) = renderer.import_memory(
                            &pixels,
                            smithay::backend::allocator::Fourcc::Argb8888,
                            (tb_width as i32, tb_height as i32).into(),
                            false,
                        ) {
                            cache.insert(key.clone(), tex);
                        }
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
                let (content_height, show_title_bar) = terminal_manager.get(*id)
                    .map(|t| {
                        let h = if !t.is_visible() {
                            0
                        } else if let Some(tex) = t.get_texture() {
                            tex.size().h
                        } else {
                            t.height as i32
                        };
                        (h, t.show_title_bar)
                    })
                    .unwrap_or((node.height, false));
                // Add title bar height only if title bar is shown
                let height = if content_height > 0 && show_title_bar {
                    content_height + TITLE_BAR_HEIGHT as i32
                } else {
                    content_height
                };
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

                // For external windows, check if height differs from committed
                // (happens during resize drag when using target height for layout)
                let committed_height = entry.state.current_height() as i32;
                let adjusted_render_y = if committed_height != height {
                    // Top-align content: shift render_y up by the height difference
                    // (in OpenGL coords, Y=0 at bottom, so adding moves content up on screen)
                    render_y + (height - committed_height)
                } else {
                    render_y
                };

                render_data.push(CellRenderData::External {
                    y: adjusted_render_y,
                    height: committed_height,  // Use committed height for rendering
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

    let Some(texture) = terminal.get_texture() else { return };

    // Only render if visible
    if y + height <= 0 || y >= screen_size.h {
        return;
    }

    // Calculate content area (below title bar if present)
    let title_bar_height = if title_bar_texture.is_some() { TITLE_BAR_HEIGHT as i32 } else { 0 };
    let content_area_top = y + height - title_bar_height;

    // Render title bar if present
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
    _screen_size: Size<i32, Physical>,
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
