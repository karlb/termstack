//! Render loop helpers
//!
//! Extracted from main.rs to reduce complexity of the main render loop.

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Element, RenderElement};
use smithay::backend::renderer::gles::{GlesFrame, GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Color32F, Frame, ImportMem, Texture};
use smithay::utils::{Physical, Point, Rectangle, Scale, Size, Transform};

use crate::state::ColumnCell;
use crate::terminal_manager::{TerminalId, TerminalManager};
use crate::title_bar::{TitleBarRenderer, TITLE_BAR_HEIGHT};

/// Data needed to render a single cell
pub enum CellRenderData<'a> {
    Terminal {
        id: TerminalId,
        y: i32,
        height: i32,
    },
    External {
        y: i32,
        height: i32,
        elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>>,
        title_bar_texture: Option<&'a GlesTexture>,
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

/// Pre-render title bar textures for external windows (SSD only)
pub fn prerender_title_bars(
    cells: &[ColumnCell],
    title_bar_renderer: &mut Option<TitleBarRenderer>,
    renderer: &mut GlesRenderer,
    width: i32,
) -> Vec<Option<GlesTexture>> {
    let mut textures = Vec::new();

    for cell in cells.iter() {
        if let ColumnCell::External(entry) = cell {
            if entry.uses_csd {
                textures.push(None);
            } else if let Some(ref mut tb_renderer) = title_bar_renderer {
                let (pixels, tb_width, tb_height) = tb_renderer.render(&entry.command, width as u32);
                let tex = renderer.import_memory(
                    &pixels,
                    smithay::backend::allocator::Fourcc::Argb8888,
                    (tb_width as i32, tb_height as i32).into(),
                    false,
                ).ok();
                textures.push(tex);
            } else {
                textures.push(None);
            }
        } else {
            textures.push(None);
        }
    }

    textures
}

/// Collect actual heights for all cells and render external window elements
///
/// Returns (heights, external_elements_per_cell)
pub fn collect_cell_data(
    cells: &[ColumnCell],
    terminal_manager: &TerminalManager,
    cached_heights: &[i32],
    renderer: &mut GlesRenderer,
    scale: Scale<f64>,
) -> (Vec<i32>, Vec<Vec<WaylandSurfaceRenderElement<GlesRenderer>>>) {
    let mut heights = Vec::new();
    let mut external_elements = Vec::new();

    for cell in cells.iter() {
        let cached_height = cached_heights.get(heights.len()).copied().unwrap_or(200);

        match cell {
            ColumnCell::Terminal(id) => {
                let height = terminal_manager.get(*id)
                    .map(|t| {
                        if t.hidden {
                            0
                        } else if let Some(tex) = t.get_texture() {
                            tex.size().h
                        } else {
                            t.height as i32
                        }
                    })
                    .unwrap_or(cached_height);
                heights.push(height);
                external_elements.push(Vec::new());
            }
            ColumnCell::External(entry) => {
                let window = &entry.window;
                let location: Point<i32, Physical> = Point::from((0, 0));
                let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
                    window.render_elements(renderer, location, scale, 1.0);

                let window_height = if elements.is_empty() {
                    cached_height
                } else {
                    elements.iter()
                        .map(|e| {
                            let geo = e.geometry(scale);
                            geo.loc.y + geo.size.h
                        })
                        .max()
                        .unwrap_or(cached_height)
                };

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
    cells: &[ColumnCell],
    heights: &[i32],
    external_elements: &mut [Vec<WaylandSurfaceRenderElement<GlesRenderer>>],
    title_bar_textures: &'a [Option<GlesTexture>],
    scroll_offset: f64,
    screen_height: i32,
    terminal_manager: &TerminalManager,
) -> Vec<CellRenderData<'a>> {
    let mut render_data = Vec::new();
    let mut content_y: i32 = -(scroll_offset as i32);

    for (cell_idx, cell) in cells.iter().enumerate() {
        let height = heights[cell_idx];
        let render_y = screen_height - content_y - height;

        match cell {
            ColumnCell::Terminal(id) => {
                if let Some(term) = terminal_manager.get(*id) {
                    let has_texture = term.get_texture().is_some();
                    tracing::debug!(
                        id = id.0,
                        hidden = term.hidden,
                        height,
                        render_y,
                        has_texture,
                        content_rows = term.content_rows(),
                        "terminal render state"
                    );
                }
                render_data.push(CellRenderData::Terminal {
                    id: *id,
                    y: render_y,
                    height,
                });
            }
            ColumnCell::External(_entry) => {
                let elements = std::mem::take(&mut external_elements[cell_idx]);
                let title_bar_texture = title_bar_textures.get(cell_idx).and_then(|t| t.as_ref());
                render_data.push(CellRenderData::External {
                    y: render_y,
                    height,
                    elements,
                    title_bar_texture,
                });
            }
        }

        content_y += height;
    }

    render_data
}

/// Log frame state for debugging (only when external windows present)
pub fn log_frame_state(
    cells: &[ColumnCell],
    cached_heights: &[i32],
    render_data: &[CellRenderData],
    terminal_manager: &TerminalManager,
    scroll_offset: f64,
    focused_index: Option<usize>,
    screen_height: i32,
) {
    let has_external = cells.iter().any(|c| matches!(c, ColumnCell::External(_)));
    if !has_external {
        return;
    }

    let cell_info: Vec<String> = cells.iter().enumerate().map(|(i, cell)| {
        match cell {
            ColumnCell::Terminal(id) => {
                let hidden = terminal_manager.get(*id).map(|t| t.hidden).unwrap_or(false);
                format!("[{}]Term({})h={}{}",
                    i, id.0,
                    cached_heights.get(i).unwrap_or(&0),
                    if hidden { " HIDDEN" } else { "" })
            }
            ColumnCell::External(e) => {
                format!("[{}]Ext h={}", i, e.state.current_height())
            }
        }
    }).collect();

    let render_info: Vec<String> = render_data.iter().enumerate().map(|(i, data)| {
        match data {
            CellRenderData::Terminal { id, y, height } => {
                format!("[{}]T{}@y={},h={}", i, id.0, y, height)
            }
            CellRenderData::External { y, height, .. } => {
                format!("[{}]E@y={},h={}", i, y, height)
            }
        }
    }).collect();

    tracing::info!(
        scroll = scroll_offset,
        focused = ?focused_index,
        screen_h = screen_height,
        cells = %cell_info.join(" "),
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
pub fn render_terminal(
    frame: &mut GlesFrame<'_, '_>,
    terminal_manager: &TerminalManager,
    id: TerminalId,
    y: i32,
    height: i32,
    is_focused: bool,
    screen_size: Size<i32, Physical>,
    damage: Rectangle<i32, Physical>,
) {
    let Some(terminal) = terminal_manager.get(id) else { return };

    if terminal.hidden {
        return;
    }

    let Some(texture) = terminal.get_texture() else { return };

    // Only render if visible
    if y + height <= 0 || y >= screen_size.h {
        return;
    }

    frame.render_texture_at(
        texture,
        Point::from((0, y)),
        1,
        1.0,
        Transform::Flipped180,
        &[damage],
        &[],
        1.0,
    ).ok();

    // Draw focus indicator
    if is_focused && y >= 0 {
        let border_height = 2;
        let focus_damage = Rectangle::new(
            (0, y).into(),
            (screen_size.w, border_height).into(),
        );
        frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_damage]).ok();
    }
}

/// Render an external window cell with title bar
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
) {
    let title_bar_y = y + height - TITLE_BAR_HEIGHT as i32;

    // Draw focus indicator at the top of the title bar
    if is_focused && title_bar_y >= 0 && title_bar_y < screen_size.h {
        let border_height = 2;
        let focus_damage = Rectangle::new(
            (0, title_bar_y + TITLE_BAR_HEIGHT as i32 - border_height).into(),
            (screen_size.w, border_height).into(),
        );
        frame.clear(Color32F::new(0.0, 0.8, 0.0, 1.0), &[focus_damage]).ok();
    }

    // Render title bar
    if let Some(tex) = title_bar_texture {
        frame.render_texture_at(
            tex,
            Point::from((0, title_bar_y)),
            1,
            1.0,
            Transform::Flipped180,
            &[damage],
            &[],
            1.0,
        ).ok();
    }

    // Render external window elements
    for element in elements {
        let geo = element.geometry(scale);
        let src = element.src();

        let dest = Rectangle::new(
            Point::from((geo.loc.x, geo.loc.y + y)),
            geo.size,
        );

        let flipped_src = Rectangle::new(
            Point::from((src.loc.x, src.loc.y + src.size.h)),
            Size::from((src.size.w, -src.size.h)),
        );

        element.draw(frame, flipped_src, dest, &[damage], &[]).ok();
    }
}
