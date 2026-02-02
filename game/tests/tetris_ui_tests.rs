use engine::render::{color_for_cell, CELL_SIZE};

use game::tetris_core::{Piece, TetrisCore, Vec2i, BOARD_HEIGHT};
use game::tetris_ui::{draw_pause_menu, draw_tetris};

#[test]
fn draw_tetris_renders_hold_panel_outside_board_area() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let layout = draw_tetris(&mut frame, width, height, &core);
    assert!(layout.hold_panel.w > 0 && layout.hold_panel.h > 0);
    assert!(layout.hold_panel.x < width && layout.hold_panel.y < height);

    let idx = ((layout.hold_panel.y * width + layout.hold_panel.x) * 4) as usize;
    let mut pixel = [0u8; 4];
    pixel.copy_from_slice(&frame[idx..idx + 4]);

    let bg = color_for_cell(0);
    assert_ne!(pixel, bg, "expected hold panel border to differ from background");
}

#[test]
fn draw_tetris_renders_ghost_piece_at_hard_drop_position() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 10), 0);

    assert_eq!(core.ghost_piece_pos(), Some(Vec2i::new(4, 1)));

    let layout = draw_tetris(&mut frame, width, height, &core);

    let bg = color_for_cell(0);
    let piece_color = color_for_cell(2);

    // Sample a pixel inside the bottom-left ghost cell at board coords (4, 0).
    let ghost_cell_x = 4u32;
    let ghost_cell_y_from_bottom = 0u32;
    let ghost_inverted_y = (BOARD_HEIGHT as u32 - 1) - ghost_cell_y_from_bottom;
    let ghost_px = layout.board.x + ghost_cell_x * CELL_SIZE + 1;
    let ghost_py = layout.board.y + ghost_inverted_y * CELL_SIZE + 1;
    let ghost_idx = ((ghost_py * width + ghost_px) * 4) as usize;
    let mut ghost_pixel = [0u8; 4];
    ghost_pixel.copy_from_slice(&frame[ghost_idx..ghost_idx + 4]);

    assert_ne!(ghost_pixel, bg, "ghost cell should be drawn over background");
    assert_ne!(
        ghost_pixel, piece_color,
        "ghost cell should differ from the solid piece color"
    );

    // Active piece should be drawn at its current position (board coords (4, 10)).
    let active_cell_y_from_bottom = 10u32;
    let active_inverted_y = (BOARD_HEIGHT as u32 - 1) - active_cell_y_from_bottom;
    let active_px = layout.board.x + ghost_cell_x * CELL_SIZE + 1;
    let active_py = layout.board.y + active_inverted_y * CELL_SIZE + 1;
    let active_idx = ((active_py * width + active_px) * 4) as usize;
    assert_eq!(
        &frame[active_idx..active_idx + 4],
        &piece_color,
        "active piece should render over the ghost"
    );
}


#[test]
fn draw_tetris_renders_pause_button_in_bounds() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let layout = draw_tetris(&mut frame, width, height, &core);
    assert!(layout.pause_button.w > 0 && layout.pause_button.h > 0);
    assert!(layout.pause_button.x < width && layout.pause_button.y < height);

    let idx = ((layout.pause_button.y * width + layout.pause_button.x) * 4) as usize;
    let mut pixel = [0u8; 4];
    pixel.copy_from_slice(&frame[idx..idx + 4]);

    let bg = color_for_cell(0);
    assert_ne!(
        pixel, bg,
        "expected pause button border to differ from background"
    );
}

#[test]
fn draw_pause_menu_draws_a_panel_and_resume_button() {
    let width = 800u32;
    let height = 600u32;

    let bg = color_for_cell(0);
    let mut frame = vec![0u8; (width * height * 4) as usize];
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    let layout = draw_pause_menu(&mut frame, width, height);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert!(layout.resume_button.w > 0 && layout.resume_button.h > 0);

    let idx = ((layout.panel.y * width + layout.panel.x) * 4) as usize;
    assert_ne!(
        &frame[idx..idx + 4],
        &bg,
        "expected pause menu panel border to differ from background"
    );
}
