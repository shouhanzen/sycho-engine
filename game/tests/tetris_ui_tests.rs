use engine::render::{color_for_cell, CELL_SIZE};
use engine::graphics::CpuRenderer;
use engine::surface::SurfaceSize;

use game::tetris_core::{Piece, TetrisCore, Vec2i, BOARD_HEIGHT};
use game::tetris_ui::{
    draw_game_over_menu, draw_main_menu, draw_main_menu_with_cursor, draw_pause_menu, draw_skilltree, draw_tetris,
    MAIN_MENU_TITLE,
};

#[test]
fn draw_tetris_renders_hold_panel_outside_board_area() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris(&mut gfx, width, height, &core);
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

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris(&mut gfx, width, height, &core);

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

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris(&mut gfx, width, height, &core);
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

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_pause_menu(&mut gfx, width, height);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert!(layout.resume_button.w > 0 && layout.resume_button.h > 0);
    assert!(layout.end_run_button.w > 0 && layout.end_run_button.h > 0);

    let idx = ((layout.panel.y * width + layout.panel.x) * 4) as usize;
    assert_ne!(
        &frame[idx..idx + 4],
        &bg,
        "expected pause menu panel border to differ from background"
    );
}

#[test]
fn draw_main_menu_draws_a_panel_and_buttons() {
    let width = 800u32;
    let height = 600u32;

    let bg = color_for_cell(0);
    let mut frame = vec![0u8; (width * height * 4) as usize];
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_main_menu(&mut gfx, width, height);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert!(layout.start_button.w > 0 && layout.start_button.h > 0);
    assert!(layout.quit_button.w > 0 && layout.quit_button.h > 0);

    let idx = ((layout.panel.y * width + layout.panel.x) * 4) as usize;
    assert_eq!(
        &frame[idx..idx + 4],
        &bg,
        "expected main menu to be non-modal (no panel drawn at the safe-region corner)"
    );

    // Buttons should draw over the cleared background.
    let btn_idx = ((layout.start_button.y * width + layout.start_button.x) * 4) as usize;
    assert_ne!(
        &frame[btn_idx..btn_idx + 4],
        &bg,
        "expected main menu start button to draw over the background"
    );

    // Title should draw some pixels above the buttons.
    let mut found_title_px = false;
    for y in layout.panel.y..layout.start_button.y.min(height) {
        let row_start = ((y * width) * 4) as usize;
        for x in 0..width {
            let i = row_start + (x as usize) * 4;
            if i + 4 <= frame.len() && &frame[i..i + 4] != &bg {
                found_title_px = true;
                break;
            }
        }
        if found_title_px {
            break;
        }
    }
    assert!(
        found_title_px,
        "expected main menu title to draw pixels above the buttons"
    );
}

#[test]
fn draw_main_menu_brightens_start_button_on_hover() {
    let width = 800u32;
    let height = 600u32;

    let bg = color_for_cell(0);

    let mut frame_normal = vec![0u8; (width * height * 4) as usize];
    for px in frame_normal.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    let mut gfx_normal = CpuRenderer::new(&mut frame_normal, SurfaceSize::new(width, height));
    let layout = draw_main_menu(&mut gfx_normal, width, height);

    let hover_x = layout.start_button.x.saturating_add(2);
    let hover_y = layout.start_button.y.saturating_add(2);
    let idx = ((hover_y * width + hover_x) * 4) as usize;
    let mut normal_px = [0u8; 4];
    normal_px.copy_from_slice(&frame_normal[idx..idx + 4]);

    let mut frame_hover = vec![0u8; (width * height * 4) as usize];
    for px in frame_hover.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }
    let mut gfx_hover = CpuRenderer::new(&mut frame_hover, SurfaceSize::new(width, height));
    let _layout_hover = draw_main_menu_with_cursor(&mut gfx_hover, width, height, Some((hover_x, hover_y)));
    let mut hover_px = [0u8; 4];
    hover_px.copy_from_slice(&frame_hover[idx..idx + 4]);

    assert!(
        hover_px[0] > normal_px[0] && hover_px[1] > normal_px[1] && hover_px[2] > normal_px[2],
        "expected hovered button fill to be brighter than normal"
    );
}

#[test]
fn main_menu_title_is_untitled() {
    assert_eq!(MAIN_MENU_TITLE, "UNTITLED");
}

#[test]
fn draw_main_menu_clears_the_tetris_board_underneath() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let bg = color_for_cell(0);
    let piece_color = color_for_cell(2);

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    // Paint a bright, easy-to-detect cell at both the bottom and top of the board so we can
    // sample a location that is outside the centered main menu panel.
    core.set_cell(0, 0, 2);
    core.set_cell(0, BOARD_HEIGHT - 1, 2);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let tetris_layout = draw_tetris(&mut gfx, width, height, &core);

    // Sample a pixel inside the bottom-left painted board cell.
    let cell_x = 0u32;
    let cell_y_from_bottom = 0u32;
    let inverted_y = (BOARD_HEIGHT as u32 - 1) - cell_y_from_bottom;
    let sample_px = tetris_layout.board.x + cell_x * CELL_SIZE + 1;
    let sample_py = tetris_layout.board.y + inverted_y * CELL_SIZE + 1;
    let idx = ((sample_py * width + sample_px) * 4) as usize;
    assert_eq!(
        &frame[idx..idx + 4],
        &piece_color,
        "expected the painted board cell pixel to be present before opening main menu"
    );

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_main_menu(&mut gfx, width, height);
    assert_eq!(
        &frame[idx..idx + 4],
        &bg,
        "expected main menu scene to clear the tetris board/background"
    );
}

#[test]
fn draw_game_over_menu_draws_a_panel_and_buttons() {
    let width = 800u32;
    let height = 600u32;

    let bg = color_for_cell(0);
    let mut frame = vec![0u8; (width * height * 4) as usize];
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&bg);
    }

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_game_over_menu(&mut gfx, width, height);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert!(layout.restart_button.w > 0 && layout.restart_button.h > 0);
    assert!(layout.skilltree_button.w > 0 && layout.skilltree_button.h > 0);
    assert!(layout.quit_button.w > 0 && layout.quit_button.h > 0);

    let idx = ((layout.panel.y * width + layout.panel.x) * 4) as usize;
    assert_ne!(
        &frame[idx..idx + 4],
        &bg,
        "expected game over panel border to differ from background"
    );
}

#[test]
fn draw_skilltree_draws_a_panel_and_start_new_game_button() {
    let width = 800u32;
    let height = 600u32;

    let bg = color_for_cell(0);
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Draw a known Tetris piece first so we can assert the skilltree scene clears it.
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 10), 0);
    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let tetris_layout = draw_tetris(&mut gfx, width, height, &core);

    let piece_color = color_for_cell(2);
    let cell_x = 4u32;
    let cell_y_from_bottom = 10u32;
    let inverted_y = (BOARD_HEIGHT as u32 - 1) - cell_y_from_bottom;
    let px = tetris_layout.board.x + cell_x * CELL_SIZE + 1;
    let py = tetris_layout.board.y + inverted_y * CELL_SIZE + 1;
    let idx = ((py * width + px) * 4) as usize;
    assert_eq!(
        &frame[idx..idx + 4],
        &piece_color,
        "expected the active piece pixel to be present before opening skilltree"
    );

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_skilltree(&mut gfx, width, height);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert!(layout.start_new_game_button.w > 0 && layout.start_new_game_button.h > 0);

    assert_eq!(
        &frame[idx..idx + 4],
        &bg,
        "expected skilltree scene to clear the tetris board/background"
    );

    let start_idx = ((layout.start_new_game_button.y * width + layout.start_new_game_button.x) * 4) as usize;
    assert_ne!(
        &frame[start_idx..start_idx + 4],
        &bg,
        "expected the skilltree start button to draw over the background"
    );
}
