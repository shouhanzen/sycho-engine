use engine::render::{color_for_cell, draw_board, CELL_SIZE};

#[test]
fn color_mapping_is_stable() {
    assert_eq!(color_for_cell(0), [10, 10, 14, 255]);
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

    draw_board(&mut frame, width, height, &board);

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

    draw_board(&mut frame, width, height, &board);

    let board_pixel_width = board_width as u32 * CELL_SIZE;
    let board_pixel_height = board_height as u32 * CELL_SIZE;
    let offset_x = (width - board_pixel_width) / 2;
    let offset_y = (height - board_pixel_height) / 2;

    // The bottom-left pixel of the board's bottom-left cell should be the piece color,
    // and the window's true bottom-left pixel should remain background.
    let expected_x = offset_x;
    let expected_y = offset_y + board_pixel_height - 1;
    let expected_index = ((expected_y * width + expected_x) * 4) as usize;
    assert_eq!(&frame[expected_index..expected_index + 4], &[0, 229, 255, 255]);

    let window_bottom_left_index = ((height - 1) * width * 4) as usize;
    assert_eq!(
        &frame[window_bottom_left_index..window_bottom_left_index + 4],
        &[10, 10, 14, 255]
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

    draw_board(&mut frame, width, height, &board);

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

