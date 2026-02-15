use engine::graphics::CpuRenderer;
use engine::render::{
    CELL_SIZE, color_for_cell, draw_board, draw_board_cells, draw_board_cells_in_rect,
};
use engine::surface::SurfaceSize;
use engine::ui::Rect;

#[test]
fn color_mapping_is_stable() {
    assert_eq!(color_for_cell(0), [0, 0, 0, 255]);
    assert_eq!(color_for_cell(1), [0, 229, 255, 255]);
    assert_eq!(color_for_cell(2), [255, 215, 0, 255]);
}

#[test]
fn draw_board_renders_bottom_row_at_buffer_bottom() {
    let board_width = 2usize;
    let board_height = 2usize;

    let width = board_width as u32 * CELL_SIZE;
    let height = board_height as u32 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut board = vec![vec![0u8; board_width]; board_height];
    // row 0 is the *bottom* row in this representation
    board[0][0] = 1;

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board(&mut gfx, &board);

    let bottom_left_index = ((height - 1) * width * 4) as usize;
    let pixel = &frame[bottom_left_index..bottom_left_index + 4];
    assert_eq!(pixel, &[0, 229, 255, 255]);
}

#[test]
fn draw_board_centers_board_in_larger_buffer() {
    let width = 1920u32;
    let height = 1080u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board_width = 10usize;
    let board_height = 20usize;
    let mut board = vec![vec![0u8; board_width]; board_height];
    board[0][0] = 1;

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board(&mut gfx, &board);

    let board_pixel_width = board_width as u32 * CELL_SIZE;
    let board_pixel_height = board_height as u32 * CELL_SIZE;
    let offset_x = (width - board_pixel_width) / 2;
    let offset_y = (height - board_pixel_height) / 2;

    // The bottom-left pixel of the board's bottom-left cell should be the piece color,
    // and the window's true bottom-left pixel should remain background.
    let expected_x = offset_x;
    let expected_y = offset_y + board_pixel_height - 1;
    let expected_index = ((expected_y * width + expected_x) * 4) as usize;
    assert_eq!(
        &frame[expected_index..expected_index + 4],
        &[0, 229, 255, 255]
    );

    let window_bottom_left_index = ((height - 1) * width * 4) as usize;
    assert_eq!(
        &frame[window_bottom_left_index..window_bottom_left_index + 4],
        &[0, 0, 0, 255]
    );
}

#[test]
fn draw_board_draws_faint_outline_and_grid_dots() {
    let width = 1920u32;
    let height = 1080u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board_width = 10usize;
    let board_height = 20usize;
    let mut board = vec![vec![0u8; board_width]; board_height];
    board[0][0] = 1;

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board(&mut gfx, &board);

    let board_pixel_width = board_width as u32 * CELL_SIZE;
    let board_pixel_height = board_height as u32 * CELL_SIZE;
    let offset_x = (width - board_pixel_width) / 2;
    let offset_y = (height - board_pixel_height) / 2;

    // Outline is drawn just outside the board area when there is padding.
    let outline_color = [28u8, 28u8, 38u8, 255u8];
    let outline_x = offset_x - 1;
    let outline_y = offset_y + board_pixel_height / 2;
    let outline_index = ((outline_y * width + outline_x) * 4) as usize;
    assert_eq!(&frame[outline_index..outline_index + 4], &outline_color);

    // Grid dot in the center of an empty cell (x=1, bottom row y=0) should be visible.
    let dot_color = [18u8, 18u8, 24u8, 255u8];
    let cell_x = 1u32;
    let cell_y_from_bottom = 0u32;
    let inverted_y = (board_height as u32 - 1) - cell_y_from_bottom;
    let cell_px = offset_x + cell_x * CELL_SIZE;
    let cell_py = offset_y + inverted_y * CELL_SIZE;
    let dot_x = cell_px + (CELL_SIZE / 2);
    let dot_y = cell_py + (CELL_SIZE / 2);
    let dot_index = ((dot_y * width + dot_x) * 4) as usize;
    assert_eq!(&frame[dot_index..dot_index + 4], &dot_color);
}

#[test]
fn draw_board_cells_in_rect_matches_centered_wrapper_when_using_center_rect() {
    let width = 640u32;
    let height = 480u32;
    let mut frame_centered = vec![0u8; (width * height * 4) as usize];
    let mut frame_explicit = vec![0u8; (width * height * 4) as usize];

    let board_width = 10usize;
    let board_height = 20usize;
    let mut board = vec![vec![0u8; board_width]; board_height];
    board[0][0] = 1;
    board[10][5] = 3;

    let board_pixel_width = board_width as u32 * CELL_SIZE;
    let board_pixel_height = board_height as u32 * CELL_SIZE;
    let centered_x = (width - board_pixel_width) / 2;
    let centered_y = (height - board_pixel_height) / 2;

    {
        let mut gfx = CpuRenderer::new(&mut frame_centered, SurfaceSize::new(width, height));
        draw_board_cells(&mut gfx, &board);
    }
    {
        let mut gfx = CpuRenderer::new(&mut frame_explicit, SurfaceSize::new(width, height));
        draw_board_cells_in_rect(
            &mut gfx,
            &board,
            Rect::new(
                centered_x,
                centered_y,
                board_pixel_width,
                board_pixel_height,
            ),
        );
    }

    assert_eq!(
        frame_centered, frame_explicit,
        "explicit centered rect should preserve old draw_board_cells output"
    );
}

#[test]
fn draw_board_cells_in_rect_applies_origin_offset() {
    let width = 6 * CELL_SIZE;
    let height = 8 * CELL_SIZE;
    let mut frame_a = vec![0u8; (width * height * 4) as usize];
    let mut frame_b = vec![0u8; (width * height * 4) as usize];

    let board = vec![vec![1u8]];
    let base_rect = Rect::new(CELL_SIZE, CELL_SIZE, CELL_SIZE, CELL_SIZE);
    let shifted_rect = Rect::new(CELL_SIZE, CELL_SIZE * 3, CELL_SIZE, CELL_SIZE);

    {
        let mut gfx = CpuRenderer::new(&mut frame_a, SurfaceSize::new(width, height));
        draw_board_cells_in_rect(&mut gfx, &board, base_rect);
    }
    {
        let mut gfx = CpuRenderer::new(&mut frame_b, SurfaceSize::new(width, height));
        draw_board_cells_in_rect(&mut gfx, &board, shifted_rect);
    }

    let piece = color_for_cell(1);
    let x = base_rect.x + 1;
    let y_base = base_rect.y + 1;
    let y_shifted = shifted_rect.y + 1;
    let idx_a = ((y_base * width + x) * 4) as usize;
    let idx_b_same = ((y_base * width + x) * 4) as usize;
    let idx_b_shifted = ((y_shifted * width + x) * 4) as usize;

    assert_eq!(&frame_a[idx_a..idx_a + 4], &piece);
    assert_ne!(&frame_b[idx_b_same..idx_b_same + 4], &piece);
    assert_eq!(&frame_b[idx_b_shifted..idx_b_shifted + 4], &piece);
}
