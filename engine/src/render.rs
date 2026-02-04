pub const CELL_SIZE: u32 = 24;

const COLOR_BACKGROUND: [u8; 4] = [0, 0, 0, 255];
const COLOR_BOARD_OUTLINE: [u8; 4] = [28, 28, 38, 255];
const COLOR_GRID_DOT: [u8; 4] = [18, 18, 24, 255];
const COLOR_I: [u8; 4] = [0, 229, 255, 255];
const COLOR_O: [u8; 4] = [255, 215, 0, 255];
const COLOR_T: [u8; 4] = [186, 85, 211, 255];
const COLOR_S: [u8; 4] = [0, 200, 0, 255];
const COLOR_Z: [u8; 4] = [220, 20, 60, 255];
const COLOR_J: [u8; 4] = [30, 144, 255, 255];
const COLOR_L: [u8; 4] = [255, 140, 0, 255];

pub fn color_for_cell(cell: u8) -> [u8; 4] {
    match cell {
        0 => COLOR_BACKGROUND,
        1 => COLOR_I,
        2 => COLOR_O,
        3 => COLOR_T,
        4 => COLOR_S,
        5 => COLOR_Z,
        6 => COLOR_J,
        7 => COLOR_L,
        _ => [255, 255, 255, 255],
    }
}

pub fn draw_board(gfx: &mut dyn crate::graphics::Renderer2d, board: &[Vec<u8>]) {
    let size = gfx.size();
    let width = size.width;
    let height = size.height;

    gfx.fill_rect(crate::ui::Rect::from_size(width, height), COLOR_BACKGROUND);

    if board.is_empty() {
        return;
    }

    let board_height = board.len() as u32;
    let board_width = board[0].len() as u32;

    let board_pixel_width = board_width.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_height.saturating_mul(CELL_SIZE);
    let offset_x = width.saturating_sub(board_pixel_width) / 2;
    let offset_y = height.saturating_sub(board_pixel_height) / 2;

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

            if pixel_x + CELL_SIZE > width || pixel_y + CELL_SIZE > height {
                continue;
            }

            if cell == 0 {
                // A subtle dot in the center of each empty cell helps reveal the grid without
                // distracting from the pieces.
                let dot_size = 2u32;
                let dot_x = pixel_x + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                let dot_y = pixel_y + (CELL_SIZE / 2).saturating_sub(dot_size / 2);
                gfx.fill_rect(crate::ui::Rect::new(dot_x, dot_y, dot_size, dot_size), COLOR_GRID_DOT);
            } else {
                let color = color_for_cell(cell);
                gfx.fill_rect(crate::ui::Rect::new(pixel_x, pixel_y, CELL_SIZE, CELL_SIZE), color);
            }
        }
    }
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
            crate::ui::Rect::new(offset_x, offset_y + board_pixel_height, board_pixel_width, 1),
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
            crate::ui::Rect::new(offset_x + board_pixel_width, offset_y, 1, board_pixel_height),
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
            crate::ui::Rect::new(offset_x + board_pixel_width, offset_y + board_pixel_height, 1, 1),
            COLOR_BOARD_OUTLINE,
        );
    }
}

