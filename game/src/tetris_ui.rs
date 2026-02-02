use engine::render::{color_for_cell, draw_board, CELL_SIZE};

use crate::debug::draw_text;

use crate::tetris_core::{Piece, TetrisCore, Vec2i};

const COLOR_PANEL_BG: [u8; 4] = [16, 16, 22, 255];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];
const COLOR_PANEL_BORDER_DISABLED: [u8; 4] = [28, 28, 38, 255];

const PAUSE_BUTTON_SIZE: u32 = 44;
const PAUSE_BUTTON_MARGIN: u32 = 12;
const COLOR_PAUSE_ICON: [u8; 4] = [235, 235, 245, 255];

const COLOR_PAUSE_MENU_TEXT: [u8; 4] = [235, 235, 245, 255];
const COLOR_PAUSE_MENU_DIM: [u8; 4] = [0, 0, 0, 255];
const PAUSE_MENU_DIM_ALPHA: u8 = 170;
const COLOR_PAUSE_MENU_BG: [u8; 4] = [10, 10, 14, 255];
const COLOR_PAUSE_MENU_BORDER: [u8; 4] = [40, 40, 55, 255];

const PANEL_MARGIN: u32 = 16;
const PANEL_PADDING: u32 = 12;

const PREVIEW_GRID: u32 = 4;
const PREVIEW_CELL: u32 = 16;
const PREVIEW_SIZE: u32 = PREVIEW_GRID * PREVIEW_CELL;
const PREVIEW_GAP_Y: u32 = 10;

const GHOST_ALPHA: u8 = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x.saturating_add(self.w) && py >= self.y && py < self.y.saturating_add(self.h)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiLayout {
    pub board: Rect,
    pub hold_panel: Rect,
    pub next_panel: Rect,
    pub pause_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuLayout {
    pub panel: Rect,
    pub resume_button: Rect,
}

pub fn compute_layout(width: u32, height: u32, board_w: u32, board_h: u32, next_len: usize) -> UiLayout {
    let board_pixel_width = board_w.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_h.saturating_mul(CELL_SIZE);
    let board_x = width.saturating_sub(board_pixel_width) / 2;
    let board_y = height.saturating_sub(board_pixel_height) / 2;

    let board = Rect {
        x: board_x,
        y: board_y,
        w: board_pixel_width,
        h: board_pixel_height,
    };

    let panel_w = (PREVIEW_SIZE + PANEL_PADDING * 2).min(width);
    let hold_h = (PREVIEW_SIZE + PANEL_PADDING * 2).min(height);

    // Next panel height depends on queue length.
    let next_h_content = (next_len as u32)
        .saturating_mul(PREVIEW_SIZE)
        .saturating_add((next_len as u32).saturating_sub(1).saturating_mul(PREVIEW_GAP_Y));
    let next_h = (next_h_content + PANEL_PADDING * 2).min(height);

    // Prefer hold on the left of the board, next on the right. If there isn't space,
    // fall back to the opposite side.
    let space_left = board_x;
    let space_right = width.saturating_sub(board_x.saturating_add(board_pixel_width));

    let mut hold_x = 0;
    if space_left >= panel_w.saturating_add(PANEL_MARGIN) {
        hold_x = board_x.saturating_sub(PANEL_MARGIN + panel_w);
    } else if space_right >= panel_w.saturating_add(PANEL_MARGIN) {
        hold_x = board_x.saturating_add(board_pixel_width + PANEL_MARGIN);
    }

    let mut next_x = 0;
    if space_right >= panel_w.saturating_add(PANEL_MARGIN) {
        next_x = board_x.saturating_add(board_pixel_width + PANEL_MARGIN);
    } else if space_left >= panel_w.saturating_add(PANEL_MARGIN) {
        next_x = board_x.saturating_sub(PANEL_MARGIN + panel_w);
    }

    let hold_panel = Rect {
        x: hold_x,
        y: board_y,
        w: panel_w,
        h: hold_h,
    };

    let next_panel = Rect {
        x: next_x,
        y: board_y,
        w: panel_w,
        h: next_h.min(board_pixel_height),
    };

    let pause_size = PAUSE_BUTTON_SIZE.min(width).min(height);
    let pause_button = Rect {
        x: width.saturating_sub(PAUSE_BUTTON_MARGIN.saturating_add(pause_size)),
        y: PAUSE_BUTTON_MARGIN.min(height.saturating_sub(pause_size)),
        w: pause_size,
        h: pause_size,
    };

    UiLayout {
        board,
        hold_panel,
        next_panel,
        pause_button,
    }
}

pub fn draw_tetris(
    frame: &mut [u8],
    width: u32,
    height: u32,
    state: &TetrisCore,
) -> UiLayout {
    let board = state.board();
    draw_board(frame, width, height, board);

    let board_h = board.len() as u32;
    let board_w = board.first().map(|r| r.len()).unwrap_or(0) as u32;
    let layout = compute_layout(width, height, board_w, board_h, state.next_queue().len());

    draw_ghost_and_active_piece(frame, width, height, layout.board, board_w, board_h, state);

    draw_hold_panel(
        frame,
        width,
        height,
        layout.hold_panel,
        state.held_piece(),
        state.can_hold(),
    );
    draw_next_panel(frame, width, height, layout.next_panel, state.next_queue());

    draw_pause_button(frame, width, height, layout.pause_button);

    layout
}

fn draw_pause_button(frame: &mut [u8], width: u32, height: u32, rect: Rect) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    if rect.x >= width || rect.y >= height {
        return;
    }

    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG);
    draw_rect_outline(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        COLOR_PANEL_BORDER,
    );

    // Draw a simple pause icon: two vertical bars.
    let bar_w = (rect.w / 6).max(3).min(rect.w);
    let bar_h = (rect.h * 2 / 3).max(6).min(rect.h);
    let gap = (rect.w / 5).max(4);

    let icon_total_w = bar_w.saturating_mul(2).saturating_add(gap);
    let icon_x0 = rect.x + rect.w.saturating_sub(icon_total_w) / 2;
    let icon_y0 = rect.y + rect.h.saturating_sub(bar_h) / 2;

    fill_rect(frame, width, height, icon_x0, icon_y0, bar_w, bar_h, COLOR_PAUSE_ICON);
    fill_rect(
        frame,
        width,
        height,
        icon_x0.saturating_add(bar_w + gap),
        icon_y0,
        bar_w,
        bar_h,
        COLOR_PAUSE_ICON,
    );
}

pub fn draw_pause_menu(frame: &mut [u8], width: u32, height: u32) -> PauseMenuLayout {
    // Dim the entire game view.
    blend_rect(
        frame,
        width,
        height,
        0,
        0,
        width,
        height,
        COLOR_PAUSE_MENU_DIM,
        PAUSE_MENU_DIM_ALPHA,
    );

    let margin = 32u32;
    let pad = 18u32;

    let panel_w = 360u32.min(width.saturating_sub(margin.saturating_mul(2)));
    let panel_h = 200u32.min(height.saturating_sub(margin.saturating_mul(2)));
    if panel_w == 0 || panel_h == 0 {
        return PauseMenuLayout::default();
    }

    let panel = Rect {
        x: width.saturating_sub(panel_w) / 2,
        y: height.saturating_sub(panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    fill_rect(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BG,
    );
    draw_rect_outline(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BORDER,
    );

    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad),
        "PAUSED",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 24),
        "ESC TO RESUME",
        COLOR_PAUSE_MENU_TEXT,
    );

    let button_h = 44u32.min(panel.h.saturating_sub(pad.saturating_mul(2)));
    let button_w = 240u32.min(panel.w.saturating_sub(pad.saturating_mul(2)));
    let resume_button = Rect {
        x: panel.x.saturating_add(panel.w.saturating_sub(button_w) / 2),
        y: panel.y.saturating_add(panel.h.saturating_sub(pad.saturating_add(button_h))),
        w: button_w,
        h: button_h,
    };

    fill_rect(
        frame,
        width,
        height,
        resume_button.x,
        resume_button.y,
        resume_button.w,
        resume_button.h,
        COLOR_PANEL_BG,
    );
    draw_rect_outline(
        frame,
        width,
        height,
        resume_button.x,
        resume_button.y,
        resume_button.w,
        resume_button.h,
        COLOR_PANEL_BORDER,
    );
    draw_text(
        frame,
        width,
        height,
        resume_button.x.saturating_add(16),
        resume_button.y.saturating_add(resume_button.h / 2).saturating_sub(6),
        "RESUME",
        COLOR_PAUSE_MENU_TEXT,
    );

    PauseMenuLayout { panel, resume_button }
}

#[derive(Debug, Clone, Copy)]
enum PieceDrawStyle {
    Solid,
    Ghost,
}

fn draw_ghost_and_active_piece(
    frame: &mut [u8],
    width: u32,
    height: u32,
    board_rect: Rect,
    board_w: u32,
    board_h: u32,
    state: &TetrisCore,
) {
    let Some(piece) = state.current_piece() else {
        return;
    };

    let rotation = state.current_piece_rotation();

    // Ghost should render behind the active piece.
    if let Some(ghost_pos) = state.ghost_piece_pos() {
        draw_piece_on_board(
            frame,
            width,
            height,
            board_rect,
            board_w,
            board_h,
            piece,
            ghost_pos,
            rotation,
            PieceDrawStyle::Ghost,
        );
    }

    draw_piece_on_board(
        frame,
        width,
        height,
        board_rect,
        board_w,
        board_h,
        piece,
        state.current_piece_pos(),
        rotation,
        PieceDrawStyle::Solid,
    );
}

fn draw_piece_on_board(
    frame: &mut [u8],
    width: u32,
    height: u32,
    board_rect: Rect,
    board_w: u32,
    board_h: u32,
    piece: Piece,
    pos: Vec2i,
    rotation: u8,
    style: PieceDrawStyle,
) {
    if board_w == 0 || board_h == 0 {
        return;
    }

    let grid = rotated_piece_grid(piece, rotation);
    let offset = board_offset_for_piece(piece) as i32;
    let color = color_for_cell(cell_for_piece(piece));

    for (gy, row) in grid.iter().enumerate() {
        for (gx, &cell) in row.iter().enumerate() {
            if cell != 1 {
                continue;
            }

            let board_x = pos.x + gx as i32 - offset;
            let board_y = pos.y - gy as i32 + offset;

            if board_x < 0 || board_x >= board_w as i32 {
                continue;
            }
            if board_y < 0 || board_y >= board_h as i32 {
                continue;
            }

            let pixel_x = board_rect.x + (board_x as u32) * CELL_SIZE;
            let inverted_y = (board_h - 1).saturating_sub(board_y as u32);
            let pixel_y = board_rect.y + inverted_y * CELL_SIZE;

            match style {
                PieceDrawStyle::Solid => {
                    fill_rect(frame, width, height, pixel_x, pixel_y, CELL_SIZE, CELL_SIZE, color);
                }
                PieceDrawStyle::Ghost => {
                    blend_rect(frame, width, height, pixel_x, pixel_y, CELL_SIZE, CELL_SIZE, color, GHOST_ALPHA);
                }
            }
        }
    }
}

fn rotated_piece_grid(piece: Piece, rotation: u8) -> Vec<Vec<u8>> {
    let mut grid = base_piece_grid(piece);
    for _ in 0..(rotation % 4) {
        grid = rotate_grid_90(&grid);
    }
    grid
}

fn rotate_grid_90(grid: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let size = grid.len();
    let mut rotated = vec![vec![0; size]; size];

    for y in 0..size {
        for x in 0..size {
            rotated[x][size - 1 - y] = grid[y][x];
        }
    }

    rotated
}

fn board_offset_for_piece(piece: Piece) -> usize {
    let size = base_piece_grid(piece).len();
    if size == 2 { 0 } else { 1 }
}

fn draw_hold_panel(
    frame: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    held_piece: Option<Piece>,
    can_hold: bool,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }

    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG);
    let border = if can_hold {
        COLOR_PANEL_BORDER
    } else {
        COLOR_PANEL_BORDER_DISABLED
    };
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);

    let preview_x = rect.x + PANEL_PADDING;
    let preview_y = rect.y + PANEL_PADDING;
    draw_piece_preview(frame, width, height, preview_x, preview_y, held_piece, can_hold);
}

fn draw_next_panel(frame: &mut [u8], width: u32, height: u32, rect: Rect, next_queue: &[Piece]) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }

    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG);
    draw_rect_outline(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        COLOR_PANEL_BORDER,
    );

    let mut y = rect.y + PANEL_PADDING;
    let x = rect.x + PANEL_PADDING;

    for &piece in next_queue {
        if y.saturating_add(PREVIEW_SIZE) > rect.y.saturating_add(rect.h) {
            break;
        }
        draw_piece_preview(frame, width, height, x, y, Some(piece), true);
        y = y.saturating_add(PREVIEW_SIZE + PREVIEW_GAP_Y);
    }
}

fn draw_piece_preview(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    piece: Option<Piece>,
    enabled: bool,
) {
    if x >= width || y >= height {
        return;
    }

    // Preview background area.
    fill_rect(frame, width, height, x, y, PREVIEW_SIZE, PREVIEW_SIZE, [10, 10, 14, 255]);

    let Some(piece) = piece else {
        return;
    };

    let grid = base_piece_grid(piece);
    let grid_h = grid.len() as u32;
    let grid_w = grid.first().map(|r| r.len()).unwrap_or(0) as u32;

    let offset_x = (PREVIEW_GRID.saturating_sub(grid_w)) / 2;
    let offset_y = (PREVIEW_GRID.saturating_sub(grid_h)) / 2;

    let mut color = color_for_cell(cell_for_piece(piece));
    if !enabled {
        color = dim_color(color, 0.55);
    }

    for (gy, row) in grid.iter().enumerate() {
        for (gx, &cell) in row.iter().enumerate() {
            if cell != 1 {
                continue;
            }

            let px = x + (offset_x + gx as u32) * PREVIEW_CELL;
            let py = y + (offset_y + gy as u32) * PREVIEW_CELL;
            fill_rect(frame, width, height, px, py, PREVIEW_CELL, PREVIEW_CELL, color);
        }
    }
}

fn dim_color(mut c: [u8; 4], factor: f32) -> [u8; 4] {
    let f = factor.clamp(0.0, 1.0);
    c[0] = ((c[0] as f32) * f) as u8;
    c[1] = ((c[1] as f32) * f) as u8;
    c[2] = ((c[2] as f32) * f) as u8;
    c
}

fn cell_for_piece(piece: Piece) -> u8 {
    match piece {
        Piece::I => 1,
        Piece::O => 2,
        Piece::T => 3,
        Piece::S => 4,
        Piece::Z => 5,
        Piece::J => 6,
        Piece::L => 7,
    }
}

fn base_piece_grid(piece: Piece) -> Vec<Vec<u8>> {
    match piece {
        Piece::I => vec![
            vec![0, 0, 0, 0],
            vec![1, 1, 1, 1],
            vec![0, 0, 0, 0],
            vec![0, 0, 0, 0],
        ],
        Piece::O => vec![vec![1, 1], vec![1, 1]],
        Piece::T => vec![vec![0, 1, 0], vec![1, 1, 1], vec![0, 0, 0]],
        Piece::S => vec![vec![0, 1, 1], vec![1, 1, 0], vec![0, 0, 0]],
        Piece::Z => vec![vec![1, 1, 0], vec![0, 1, 1], vec![0, 0, 0]],
        Piece::J => vec![vec![1, 0, 0], vec![1, 1, 1], vec![0, 0, 0]],
        Piece::L => vec![vec![0, 0, 1], vec![1, 1, 1], vec![0, 0, 0]],
    }
}

fn fill_rect(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    let max_x = (x + w).min(width);
    let max_y = (y + h).min(height);

    if x >= max_x || y >= max_y {
        return;
    }

    let width = width as usize;
    let height = height as usize;
    let expected_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(4))
        .unwrap_or(0);
    if expected_len == 0 || frame.len() < expected_len {
        return;
    }

    let row_pixels = (max_x - x) as usize;
    let row_bytes = row_pixels.checked_mul(4).unwrap_or(0);
    if row_bytes == 0 {
        return;
    }

    let stride = width.checked_mul(4).unwrap_or(0);
    let mut row_start = (y as usize)
        .checked_mul(stride)
        .and_then(|v| v.checked_add((x as usize).checked_mul(4)?))
        .unwrap_or(0);

    let [r, g, b, a] = color;
    for _ in y..max_y {
        let row_end = row_start + row_bytes;
        let row = &mut frame[row_start..row_end];
        for px in row.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = a;
        }
        row_start += stride;
    }
}

fn blend_rect(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
    alpha: u8,
) {
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        fill_rect(frame, width, height, x, y, w, h, color);
        return;
    }

    let max_x = (x + w).min(width);
    let max_y = (y + h).min(height);
    let a = alpha as u32;
    let inv = 255u32 - a;

    for py in y..max_y {
        for px in x..max_x {
            let idx = ((py * width + px) * 4) as usize;
            if idx + 4 <= frame.len() {
                let r0 = frame[idx] as u32;
                let g0 = frame[idx + 1] as u32;
                let b0 = frame[idx + 2] as u32;

                frame[idx] = ((r0 * inv + (color[0] as u32) * a + 127) / 255) as u8;
                frame[idx + 1] = ((g0 * inv + (color[1] as u32) * a + 127) / 255) as u8;
                frame[idx + 2] = ((b0 * inv + (color[2] as u32) * a + 127) / 255) as u8;
                frame[idx + 3] = 255;
            }
        }
    }
}

fn draw_rect_outline(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    if w == 0 || h == 0 {
        return;
    }

    let x1 = x.saturating_add(w).min(width);
    let y1 = y.saturating_add(h).min(height);
    if x >= x1 || y >= y1 {
        return;
    }

    // Top / bottom
    for px in x..x1 {
        set_pixel(frame, width, height, px, y, color);
        set_pixel(frame, width, height, px, y1.saturating_sub(1), color);
    }

    // Left / right
    for py in y..y1 {
        set_pixel(frame, width, height, x, py, color);
        set_pixel(frame, width, height, x1.saturating_sub(1), py, color);
    }
}

fn set_pixel(frame: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: [u8; 4]) {
    if x >= width || y >= height {
        return;
    }
    let idx = ((y * width + x) * 4) as usize;
    if idx + 4 <= frame.len() {
        frame[idx..idx + 4].copy_from_slice(&color);
    }
}

