pub const CELL_SIZE: u32 = 24;

const COLOR_BACKGROUND: [u8; 4] = [0, 0, 0, 255];
const COLOR_BOARD_OUTLINE: [u8; 4] = [28, 28, 38, 255];
const COLOR_GRID_DOT: [u8; 4] = [18, 18, 24, 255];
const FILLED_EDGE_STROKE_PX: u32 = 3;
const INTERNAL_SEAM_STROKE_PX: u32 = 3;
const FILLED_EDGE_STROKE_NUM: u16 = 11;
const FILLED_EDGE_STROKE_DEN: u16 = 20;
const COLOR_WOOD_I4: [u8; 4] = [143, 92, 53, 255];
const COLOR_STONE_O: [u8; 4] = [126, 124, 120, 255];
const COLOR_GLASS_I3: [u8; 4] = [150, 208, 232, 255];
const COLOR_DIRT_I2: [u8; 4] = [113, 86, 61, 255];
const COLOR_Z: [u8; 4] = [220, 20, 60, 255];
const COLOR_J: [u8; 4] = [30, 144, 255, 255];
const COLOR_L: [u8; 4] = [255, 140, 0, 255];

// Bottomwell earth cell colors
const COLOR_GARBAGE: [u8; 4] = [90, 80, 70, 255]; // dark brown/grey
const COLOR_STONE: [u8; 4] = [140, 135, 130, 255]; // grey stone
const COLOR_ORE: [u8; 4] = [200, 120, 50, 255]; // copper/ore orange
const COLOR_COIN: [u8; 4] = [255, 230, 50, 255]; // bright gold
const COLOR_BOTTOMWELL_GRASS: [u8; 4] = [94, 152, 72, 255];
const COLOR_MOSS: [u8; 4] = [72, 128, 64, 255];
const COLOR_MOSS_SEED: [u8; 4] = [118, 188, 98, 255];
const COLOR_SAND: [u8; 4] = [206, 180, 109, 255];

pub fn color_for_cell(cell: u8) -> [u8; 4] {
    match cell {
        0 => COLOR_BACKGROUND,
        1 => COLOR_WOOD_I4,
        2 => COLOR_STONE_O,
        3 => COLOR_GLASS_I3,
        4 => COLOR_DIRT_I2,
        5 => COLOR_Z,
        6 => COLOR_J,
        7 => COLOR_L,
        8 => COLOR_GARBAGE,
        9 => COLOR_STONE,
        10 => COLOR_ORE,
        11 => COLOR_COIN,
        12 => COLOR_BOTTOMWELL_GRASS,
        13 => COLOR_MOSS,
        14 => COLOR_MOSS_SEED,
        15 => COLOR_SAND,
        _ => [255, 255, 255, 255],
    }
}

/// Clip a screen-space rect to a viewport, returning the visible intersection.
pub fn clip_rect_to_viewport(
    rect: crate::ui::Rect,
    viewport: crate::ui::Rect,
) -> Option<crate::ui::Rect> {
    clip_rect_i32_to_viewport(rect.x as i32, rect.y as i32, rect.w, rect.h, viewport)
}

/// Clip an `i32`-positioned rect to a viewport, returning the visible intersection.
///
/// This is useful when world-space content can be temporarily translated outside the
/// viewport bounds (for example camera impulses) but must still render with fixed
/// viewport clipping.
pub fn clip_rect_i32_to_viewport(
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    viewport: crate::ui::Rect,
) -> Option<crate::ui::Rect> {
    if w == 0 || h == 0 || viewport.w == 0 || viewport.h == 0 {
        return None;
    }

    let vx0 = viewport.x.min(i32::MAX as u32) as i32;
    let vy0 = viewport.y.min(i32::MAX as u32) as i32;
    let vx1 = viewport.x.saturating_add(viewport.w).min(i32::MAX as u32) as i32;
    let vy1 = viewport.y.saturating_add(viewport.h).min(i32::MAX as u32) as i32;

    let w_i32 = w.min(i32::MAX as u32) as i32;
    let h_i32 = h.min(i32::MAX as u32) as i32;
    let x1 = x.saturating_add(w_i32);
    let y1 = y.saturating_add(h_i32);

    let clipped_x0 = x.max(vx0);
    let clipped_y0 = y.max(vy0);
    let clipped_x1 = x1.min(vx1);
    let clipped_y1 = y1.min(vy1);
    if clipped_x1 <= clipped_x0 || clipped_y1 <= clipped_y0 {
        return None;
    }

    Some(crate::ui::Rect::new(
        clipped_x0 as u32,
        clipped_y0 as u32,
        (clipped_x1 - clipped_x0) as u32,
        (clipped_y1 - clipped_y0) as u32,
    ))
}

pub fn draw_board(gfx: &mut dyn crate::graphics::Renderer2d, board: &[Vec<u8>]) {
    let size = gfx.size();
    let width = size.width;
    let height = size.height;

    gfx.fill_rect(crate::ui::Rect::from_size(width, height), COLOR_BACKGROUND);

    draw_board_cells(gfx, board);
}

/// Draw board outline, grid dots, and filled cells **without** clearing the frame first.
///
/// Use this when you want to composite the board on top of a previously-drawn background
/// layer (e.g. tile background).  The full-clearing `draw_board` is still available for
/// callers that don't need layered compositing.
pub fn draw_board_cells(gfx: &mut dyn crate::graphics::Renderer2d, board: &[Vec<u8>]) {
    if board.is_empty() {
        return;
    }

    let size = gfx.size();
    let width = size.width;
    let height = size.height;

    let board_height = board.len() as u32;
    let board_width = board[0].len() as u32;

    let board_pixel_width = board_width.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_height.saturating_mul(CELL_SIZE);
    let offset_x = width.saturating_sub(board_pixel_width) / 2;
    let offset_y = height.saturating_sub(board_pixel_height) / 2;

    draw_board_cells_in_rect(
        gfx,
        board,
        crate::ui::Rect::new(offset_x, offset_y, board_pixel_width, board_pixel_height),
    );
}

/// Draw board outline, grid dots, and filled cells at an explicit board rect origin.
///
/// `board_rect.w` / `board_rect.h` are currently ignored and board dimensions are derived from
/// `board` and `CELL_SIZE`. This keeps rendering deterministic while exposing world-offset control.
pub fn draw_board_cells_in_rect(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    board_rect: crate::ui::Rect,
) {
    if board.is_empty() {
        return;
    }

    let size = gfx.size();
    let width = size.width;
    let height = size.height;

    let board_height = board.len() as u32;
    let board_width = board[0].len() as u32;

    let board_pixel_width = board_width.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_height.saturating_mul(CELL_SIZE);
    let offset_x = board_rect.x;
    let offset_y = board_rect.y;
    let screen_rect = crate::ui::Rect::from_size(width, height);

    draw_board_outline(
        gfx,
        offset_x,
        offset_y,
        board_pixel_width,
        board_pixel_height,
    );

    for (y, row) in board.iter().enumerate() {
        for (x, &cell) in row.iter().enumerate() {
            let pixel_x = offset_x + x as u32 * CELL_SIZE;
            let inverted_y = board_height.saturating_sub(1).saturating_sub(y as u32);
            let pixel_y = offset_y + inverted_y * CELL_SIZE;

            if pixel_x.saturating_add(CELL_SIZE) > width
                || pixel_y.saturating_add(CELL_SIZE) > height
            {
                continue;
            }

            if cell == 0 {
                // A subtle dot in the center of each empty cell helps reveal the grid without
                // distracting from the pieces.
                let dot_size = 2u32;
                let dot_x = pixel_x + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                let dot_y = pixel_y + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                gfx.fill_rect(
                    crate::ui::Rect::new(dot_x, dot_y, dot_size, dot_size),
                    COLOR_GRID_DOT,
                );
            } else {
                let color = color_for_cell(cell);
                gfx.fill_rect(
                    crate::ui::Rect::new(pixel_x, pixel_y, CELL_SIZE, CELL_SIZE),
                    color,
                );
                draw_exposed_cell_edges(gfx, board, None, x, y, pixel_x, pixel_y, screen_rect);
            }
        }
    }

    draw_inside_corner_bridges(gfx, board, offset_x, offset_y, screen_rect);
}

/// Draw board content translated to `board_rect`, clipped to `clip_rect`.
///
/// This keeps `clip_rect` anchored as the board viewport while allowing camera-style
/// content offsets inside that viewport.
pub fn draw_board_cells_in_rect_clipped(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    board_rect: crate::ui::Rect,
    clip_rect: crate::ui::Rect,
) {
    draw_board_cells_in_rect_clipped_with_owners(gfx, board, None, board_rect, clip_rect);
}

/// Draw board content translated to `board_rect`, clipped to `clip_rect`, with optional
/// per-cell piece ownership for interior seam rendering.
///
/// When `board_owners` is provided, interior seams are drawn between adjacent non-empty cells
/// that belong to different piece IDs.
pub fn draw_board_cells_in_rect_clipped_with_owners(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    board_owners: Option<&[Vec<Option<u32>>]>,
    board_rect: crate::ui::Rect,
    clip_rect: crate::ui::Rect,
) {
    if board.is_empty() || clip_rect.w == 0 || clip_rect.h == 0 {
        return;
    }

    let board_height = board.len() as u32;
    let board_width = board[0].len() as u32;
    let board_pixel_width = board_width.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_height.saturating_mul(CELL_SIZE);
    let offset_x = board_rect.x;
    let offset_y = board_rect.y;

    let outline_w = clip_rect.w.min(board_pixel_width);
    let outline_h = clip_rect.h.min(board_pixel_height);
    if outline_w > 0 && outline_h > 0 {
        draw_board_outline(gfx, clip_rect.x, clip_rect.y, outline_w, outline_h);
    }

    for (y, row) in board.iter().enumerate() {
        for (x, &cell) in row.iter().enumerate() {
            let pixel_x = offset_x + x as u32 * CELL_SIZE;
            let inverted_y = board_height.saturating_sub(1).saturating_sub(y as u32);
            let pixel_y = offset_y + inverted_y * CELL_SIZE;

            if cell == 0 {
                // A subtle dot in the center of each empty cell helps reveal the grid without
                // distracting from the pieces.
                let dot_size = 2u32;
                let dot_x = pixel_x + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                let dot_y = pixel_y + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                let dot_rect = crate::ui::Rect::new(dot_x, dot_y, dot_size, dot_size);
                if let Some(clipped_dot) = clip_rect_to_viewport(dot_rect, clip_rect) {
                    gfx.fill_rect(clipped_dot, COLOR_GRID_DOT);
                }
            } else {
                let cell_rect = crate::ui::Rect::new(pixel_x, pixel_y, CELL_SIZE, CELL_SIZE);
                let Some(clipped_cell_rect) = clip_rect_to_viewport(cell_rect, clip_rect) else {
                    continue;
                };
                let color = color_for_cell(cell);
                gfx.fill_rect(clipped_cell_rect, color);
                draw_exposed_cell_edges(gfx, board, board_owners, x, y, pixel_x, pixel_y, clip_rect);
            }
        }
    }

    draw_interior_seam_corner_bridges(gfx, board, board_owners, offset_x, offset_y, clip_rect);
    draw_inside_corner_bridges(gfx, board, offset_x, offset_y, clip_rect);
}

fn draw_exposed_cell_edges(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    board_owners: Option<&[Vec<Option<u32>>]>,
    x: usize,
    y: usize,
    pixel_x: u32,
    pixel_y: u32,
    clip_rect: crate::ui::Rect,
) {
    if board.is_empty() {
        return;
    }

    let cell = board[y][x];
    if cell == 0 {
        return;
    }

    let board_h = board.len();
    let board_w = board[0].len();
    let stroke_color = edge_stroke_color_for_cell(cell);
    let owner_here = owner_at(board_owners, x, y);

    let left_kind = if x == 0 {
        EdgeKind::Exterior
    } else {
        classify_edge(board, board_owners, cell, owner_here, x - 1, y)
    };
    let right_kind = if x + 1 >= board_w {
        EdgeKind::Exterior
    } else {
        classify_edge(board, board_owners, cell, owner_here, x + 1, y)
    };
    let up_kind = if y + 1 >= board_h {
        EdgeKind::Exterior
    } else {
        classify_edge(board, board_owners, cell, owner_here, x, y + 1)
    };
    let down_kind = if y == 0 {
        EdgeKind::Exterior
    } else {
        classify_edge(board, board_owners, cell, owner_here, x, y - 1)
    };

    if let Some(stroke_px) = stroke_px_for_edge(left_kind, EdgeDir::Left) {
        let edge_rect = crate::ui::Rect::new(pixel_x, pixel_y, stroke_px, CELL_SIZE);
        if let Some(clipped) = clip_rect_to_viewport(edge_rect, clip_rect) {
            gfx.fill_rect(clipped, stroke_color);
        }
    }
    if let Some(stroke_px) = stroke_px_for_edge(right_kind, EdgeDir::Right) {
        let edge_rect = crate::ui::Rect::new(
            pixel_x.saturating_add(CELL_SIZE.saturating_sub(stroke_px)),
            pixel_y,
            stroke_px,
            CELL_SIZE,
        );
        if let Some(clipped) = clip_rect_to_viewport(edge_rect, clip_rect) {
            gfx.fill_rect(clipped, stroke_color);
        }
    }
    if let Some(stroke_px) = stroke_px_for_edge(up_kind, EdgeDir::Up) {
        let edge_rect = crate::ui::Rect::new(pixel_x, pixel_y, CELL_SIZE, stroke_px);
        if let Some(clipped) = clip_rect_to_viewport(edge_rect, clip_rect) {
            gfx.fill_rect(clipped, stroke_color);
        }
    }
    if let Some(stroke_px) = stroke_px_for_edge(down_kind, EdgeDir::Down) {
        let edge_rect = crate::ui::Rect::new(
            pixel_x,
            pixel_y.saturating_add(CELL_SIZE.saturating_sub(stroke_px)),
            CELL_SIZE,
            stroke_px,
        );
        if let Some(clipped) = clip_rect_to_viewport(edge_rect, clip_rect) {
            gfx.fill_rect(clipped, stroke_color);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
    Closed,
    Exterior,
    Interior,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EdgeDir {
    Left,
    Right,
    Up,
    Down,
}

fn stroke_px_for_edge(kind: EdgeKind, dir: EdgeDir) -> Option<u32> {
    match kind {
        EdgeKind::Closed => None,
        EdgeKind::Exterior => Some(FILLED_EDGE_STROKE_PX.min(CELL_SIZE)),
        // Draw interior seams once (canonical directions only) so total seam width is 3px.
        EdgeKind::Interior => match dir {
            EdgeDir::Right | EdgeDir::Up => Some(INTERNAL_SEAM_STROKE_PX.min(CELL_SIZE)),
            EdgeDir::Left | EdgeDir::Down => None,
        },
    }
}

fn classify_edge(
    board: &[Vec<u8>],
    board_owners: Option<&[Vec<Option<u32>>]>,
    cell_here: u8,
    owner_here: Option<u32>,
    nx: usize,
    ny: usize,
) -> EdgeKind {
    let neighbor_cell = board[ny][nx];
    if neighbor_cell == 0 {
        return EdgeKind::Exterior;
    }

    // When owner metadata is unavailable, preserve classic behavior: same-type neighbors merge.
    let Some(_owners) = board_owners else {
        return EdgeKind::Closed;
    };

    let owner_neighbor = owner_at(board_owners, nx, ny);
    match (owner_here, owner_neighbor) {
        (Some(lhs), Some(rhs)) => {
            if lhs != rhs {
                EdgeKind::Interior
            } else {
                EdgeKind::Closed
            }
        }
        // Mixed ownership indicates a piece-vs-environment boundary.
        (Some(_), None) | (None, Some(_)) => EdgeKind::Interior,
        // Unowned terrain/legacy cells only split when tile types differ.
        (None, None) => {
            if neighbor_cell != cell_here {
                EdgeKind::Interior
            } else {
                EdgeKind::Closed
            }
        }
    }
}

fn owner_at(board_owners: Option<&[Vec<Option<u32>>]>, x: usize, y: usize) -> Option<u32> {
    let owners = board_owners?;
    owners.get(y)?.get(x).copied().flatten()
}

fn draw_interior_seam_corner_bridges(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    board_owners: Option<&[Vec<Option<u32>>]>,
    offset_x: u32,
    offset_y: u32,
    clip_rect: crate::ui::Rect,
) {
    let Some(_owners) = board_owners else {
        return;
    };
    if board.is_empty() {
        return;
    }

    let board_h = board.len();
    let board_w = board[0].len();

    for (y, row) in board.iter().enumerate() {
        for (x, &cell) in row.iter().enumerate() {
            if cell == 0 {
                continue;
            }

            let owner_here = owner_at(board_owners, x, y);
            let right_kind = if x + 1 >= board_w {
                EdgeKind::Exterior
            } else {
                classify_edge(board, board_owners, cell, owner_here, x + 1, y)
            };
            let up_kind = if y + 1 >= board_h {
                EdgeKind::Exterior
            } else {
                classify_edge(board, board_owners, cell, owner_here, x, y + 1)
            };
            let seam_px = INTERNAL_SEAM_STROKE_PX.min(CELL_SIZE);
            let seam_i32 = seam_px.min(i32::MAX as u32) as i32;
            let inverted_y = board_h.saturating_sub(1).saturating_sub(y);
            let pixel_x = offset_x.saturating_add(x as u32 * CELL_SIZE);
            let pixel_y = offset_y.saturating_add(inverted_y as u32 * CELL_SIZE);
            let px = pixel_x.min(i32::MAX as u32) as i32;
            let py = pixel_y.min(i32::MAX as u32) as i32;
            let cell_size_i32 = CELL_SIZE.min(i32::MAX as u32) as i32;
            let stroke_color = edge_stroke_color_for_cell(cell);

            if right_kind == EdgeKind::Interior && up_kind == EdgeKind::Interior {
                fill_rect_i32_clipped(
                    gfx,
                    px.saturating_add(cell_size_i32),
                    py.saturating_sub(seam_i32),
                    seam_px,
                    seam_px,
                    clip_rect,
                    stroke_color,
                );
            }
        }
    }
}

fn draw_inside_corner_bridges(
    gfx: &mut dyn crate::graphics::Renderer2d,
    board: &[Vec<u8>],
    offset_x: u32,
    offset_y: u32,
    clip_rect: crate::ui::Rect,
) {
    if board.is_empty() {
        return;
    }

    let board_h = board.len();
    let board_w = board[0].len();
    let stroke_px = FILLED_EDGE_STROKE_PX.min(CELL_SIZE);
    let stroke_i32 = stroke_px.min(i32::MAX as u32) as i32;

    for (y, row) in board.iter().enumerate() {
        for (x, &cell) in row.iter().enumerate() {
            if cell != 0 {
                continue;
            }

            let inverted_y = board_h.saturating_sub(1).saturating_sub(y);
            let pixel_x = offset_x.saturating_add(x as u32 * CELL_SIZE);
            let pixel_y = offset_y.saturating_add(inverted_y as u32 * CELL_SIZE);
            let px = pixel_x.min(i32::MAX as u32) as i32;
            let py = pixel_y.min(i32::MAX as u32) as i32;
            let cell_size_i32 = CELL_SIZE.min(i32::MAX as u32) as i32;

            // Top-left concave corner: left and up neighbors are filled.
            if x > 0 && y + 1 < board_h {
                let left = board[y][x - 1];
                let up = board[y + 1][x];
                if left != 0 && up != 0 {
                    let owner = board[y + 1][x - 1];
                    if owner != 0 {
                        let corner = edge_stroke_color_for_cell(owner);
                        fill_rect_i32_clipped(
                            gfx,
                            px.saturating_sub(stroke_i32),
                            py.saturating_sub(stroke_i32),
                            stroke_px,
                            stroke_px,
                            clip_rect,
                            corner,
                        );
                    }
                }
            }

            // Top-right concave corner: right and up neighbors are filled.
            if x + 1 < board_w && y + 1 < board_h {
                let right = board[y][x + 1];
                let up = board[y + 1][x];
                if right != 0 && up != 0 {
                    let owner = board[y + 1][x + 1];
                    if owner != 0 {
                        let corner = edge_stroke_color_for_cell(owner);
                        fill_rect_i32_clipped(
                            gfx,
                            px.saturating_add(cell_size_i32),
                            py.saturating_sub(stroke_i32),
                            stroke_px,
                            stroke_px,
                            clip_rect,
                            corner,
                        );
                    }
                }
            }

            // Bottom-left concave corner: left and down neighbors are filled.
            if x > 0 && y > 0 {
                let left = board[y][x - 1];
                let down = board[y - 1][x];
                if left != 0 && down != 0 {
                    let owner = board[y - 1][x - 1];
                    if owner != 0 {
                        let corner = edge_stroke_color_for_cell(owner);
                        fill_rect_i32_clipped(
                            gfx,
                            px.saturating_sub(stroke_i32),
                            py.saturating_add(cell_size_i32),
                            stroke_px,
                            stroke_px,
                            clip_rect,
                            corner,
                        );
                    }
                }
            }

            // Bottom-right concave corner: right and down neighbors are filled.
            if x + 1 < board_w && y > 0 {
                let right = board[y][x + 1];
                let down = board[y - 1][x];
                if right != 0 && down != 0 {
                    let owner = board[y - 1][x + 1];
                    if owner != 0 {
                        let corner = edge_stroke_color_for_cell(owner);
                        fill_rect_i32_clipped(
                            gfx,
                            px.saturating_add(cell_size_i32),
                            py.saturating_add(cell_size_i32),
                            stroke_px,
                            stroke_px,
                            clip_rect,
                            corner,
                        );
                    }
                }
            }
        }
    }
}

fn fill_rect_i32_clipped(
    gfx: &mut dyn crate::graphics::Renderer2d,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    clip_rect: crate::ui::Rect,
    color: [u8; 4],
) {
    if let Some(clipped) = clip_rect_i32_to_viewport(x, y, w, h, clip_rect) {
        gfx.fill_rect(clipped, color);
    }
}

fn edge_stroke_color_for_cell(cell: u8) -> [u8; 4] {
    let base = color_for_cell(cell);
    [
        ((base[0] as u16 * FILLED_EDGE_STROKE_NUM) / FILLED_EDGE_STROKE_DEN) as u8,
        ((base[1] as u16 * FILLED_EDGE_STROKE_NUM) / FILLED_EDGE_STROKE_DEN) as u8,
        ((base[2] as u16 * FILLED_EDGE_STROKE_NUM) / FILLED_EDGE_STROKE_DEN) as u8,
        base[3],
    ]
}

fn draw_board_outline(
    gfx: &mut dyn crate::graphics::Renderer2d,
    offset_x: u32,
    offset_y: u32,
    board_pixel_width: u32,
    board_pixel_height: u32,
) {
    let size = gfx.size();
    let width = size.width;
    let height = size.height;

    // Draw the outline *outside* the board so we don't overwrite piece pixels at the edges.
    // If the board touches the buffer edge, that side is simply clipped (not drawn).

    // Top border
    if offset_y > 0 {
        gfx.fill_rect(
            crate::ui::Rect::new(offset_x, offset_y - 1, board_pixel_width, 1),
            COLOR_BOARD_OUTLINE,
        );
    }

    // Bottom border
    if offset_y + board_pixel_height < height {
        gfx.fill_rect(
            crate::ui::Rect::new(
                offset_x,
                offset_y + board_pixel_height,
                board_pixel_width,
                1,
            ),
            COLOR_BOARD_OUTLINE,
        );
    }

    // Left border
    if offset_x > 0 {
        gfx.fill_rect(
            crate::ui::Rect::new(offset_x - 1, offset_y, 1, board_pixel_height),
            COLOR_BOARD_OUTLINE,
        );
    }

    // Right border
    if offset_x + board_pixel_width < width {
        gfx.fill_rect(
            crate::ui::Rect::new(
                offset_x + board_pixel_width,
                offset_y,
                1,
                board_pixel_height,
            ),
            COLOR_BOARD_OUTLINE,
        );
    }

    // Corners (only if both adjacent sides exist)
    if offset_x > 0 && offset_y > 0 {
        gfx.fill_rect(
            crate::ui::Rect::new(offset_x - 1, offset_y - 1, 1, 1),
            COLOR_BOARD_OUTLINE,
        );
    }
    if offset_x + board_pixel_width < width && offset_y > 0 {
        gfx.fill_rect(
            crate::ui::Rect::new(offset_x + board_pixel_width, offset_y - 1, 1, 1),
            COLOR_BOARD_OUTLINE,
        );
    }
    if offset_x > 0 && offset_y + board_pixel_height < height {
        gfx.fill_rect(
            crate::ui::Rect::new(offset_x - 1, offset_y + board_pixel_height, 1, 1),
            COLOR_BOARD_OUTLINE,
        );
    }
    if offset_x + board_pixel_width < width && offset_y + board_pixel_height < height {
        gfx.fill_rect(
            crate::ui::Rect::new(
                offset_x + board_pixel_width,
                offset_y + board_pixel_height,
                1,
                1,
            ),
            COLOR_BOARD_OUTLINE,
        );
    }
}
