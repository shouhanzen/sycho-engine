use engine::graphics::CpuRenderer;
use engine::render::{
    CELL_SIZE, clip_rect_i32_to_viewport, clip_rect_to_viewport, color_for_cell, draw_board,
    draw_board_cells, draw_board_cells_in_rect, draw_board_cells_in_rect_clipped,
    draw_board_cells_in_rect_clipped_with_owners,
};
use engine::surface::SurfaceSize;
use engine::ui::Rect;

#[test]
fn color_mapping_is_stable() {
    assert_eq!(color_for_cell(0), [0, 0, 0, 255]);
    assert_eq!(color_for_cell(1), [0, 229, 255, 255]);
    assert_eq!(color_for_cell(2), [255, 215, 0, 255]);
    assert_eq!(color_for_cell(3), [150, 208, 232, 255]);
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

    let sample_x = 6u32;
    let sample_y = height - 7;
    let sample_index = ((sample_y * width + sample_x) * 4) as usize;
    let pixel = &frame[sample_index..sample_index + 4];
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
    let expected_x = offset_x + 6;
    let expected_y = offset_y + board_pixel_height - 7;
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
    let x = base_rect.x + 6;
    let y_base = base_rect.y + 6;
    let y_shifted = shifted_rect.y + 6;
    let idx_a = ((y_base * width + x) * 4) as usize;
    let idx_b_same = ((y_base * width + x) * 4) as usize;
    let idx_b_shifted = ((y_shifted * width + x) * 4) as usize;

    assert_eq!(&frame_a[idx_a..idx_a + 4], &piece);
    assert_ne!(&frame_b[idx_b_same..idx_b_same + 4], &piece);
    assert_eq!(&frame_b[idx_b_shifted..idx_b_shifted + 4], &piece);
}

#[test]
fn clip_rect_to_viewport_returns_intersection() {
    let rect = Rect::new(12, 20, 40, 30);
    let viewport = Rect::new(24, 10, 20, 20);
    let clipped = clip_rect_to_viewport(rect, viewport);
    assert_eq!(clipped, Some(Rect::new(24, 20, 20, 10)));
}

#[test]
fn clip_rect_i32_to_viewport_handles_negative_origin() {
    let viewport = Rect::new(10, 10, 20, 20);
    let clipped = clip_rect_i32_to_viewport(-8, 18, 20, 8, viewport);
    assert_eq!(clipped, Some(Rect::new(10, 18, 2, 8)));
}

#[test]
fn draw_board_cells_in_rect_clipped_keeps_content_inside_viewport() {
    let width = 6 * CELL_SIZE;
    let height = 8 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board = vec![vec![1u8]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, CELL_SIZE, CELL_SIZE);
    let shifted_content = Rect::new(
        viewport.x,
        viewport.y + CELL_SIZE.saturating_sub(4),
        CELL_SIZE,
        CELL_SIZE,
    );

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped(&mut gfx, &board, shifted_content, viewport);

    let piece = color_for_cell(1);
    let stroke = [
        ((piece[0] as u16 * 11) / 20) as u8,
        ((piece[1] as u16 * 11) / 20) as u8,
        ((piece[2] as u16 * 11) / 20) as u8,
        piece[3],
    ];
    let x = viewport.x + 1;
    let y_visible = viewport.y + viewport.h - 1;
    let y_outside = viewport.y + viewport.h;
    let idx_visible = ((y_visible * width + x) * 4) as usize;
    let idx_outside = ((y_outside * width + x) * 4) as usize;

    assert_eq!(&frame[idx_visible..idx_visible + 4], &stroke);
    assert_ne!(&frame[idx_outside..idx_outside + 4], &stroke);

    // Outline should remain anchored to the fixed viewport.
    let outline_color = [28u8, 28u8, 38u8, 255u8];
    let outline_x = viewport.x - 1;
    let outline_y = viewport.y + 1;
    let outline_idx = ((outline_y * width + outline_x) * 4) as usize;
    assert_eq!(&frame[outline_idx..outline_idx + 4], &outline_color);
}

#[test]
fn draw_board_cells_in_rect_clipped_draws_only_exposed_edges_for_filled_cells() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Bottom row has two adjacent filled cells.
    let board = vec![vec![1u8, 1u8]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped(&mut gfx, &board, viewport, viewport);

    let fill = color_for_cell(1);
    let stroke = [
        ((fill[0] as u16 * 11) / 20) as u8,
        ((fill[1] as u16 * 11) / 20) as u8,
        ((fill[2] as u16 * 11) / 20) as u8,
        fill[3],
    ];

    // Outer perimeter edges should be stroked.
    assert_eq!(pixel_at(&frame, width, viewport.x, viewport.y + 2), stroke);
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + viewport.w.saturating_sub(1),
            viewport.y + 2
        ),
        stroke
    );
    assert_eq!(pixel_at(&frame, width, viewport.x + 4, viewport.y), stroke);
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + 4,
            viewport.y + CELL_SIZE.saturating_sub(1)
        ),
        stroke
    );

    // Interior seam between adjacent filled cells should stay fill-colored.
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + CELL_SIZE.saturating_sub(1),
            viewport.y + 8
        ),
        fill
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE, viewport.y + 8),
        fill
    );
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_draws_seam_between_pieces() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board = vec![vec![1u8, 1u8]];
    let owners = vec![vec![Some(101u32), Some(202u32)]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let fill = color_for_cell(1);
    let stroke = [
        ((fill[0] as u16 * 11) / 20) as u8,
        ((fill[1] as u16 * 11) / 20) as u8,
        ((fill[2] as u16 * 11) / 20) as u8,
        fill[3],
    ];

    // Interior seam should be 3px total, drawn once from the canonical side.
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + CELL_SIZE.saturating_sub(1),
            viewport.y + 8
        ),
        stroke
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE, viewport.y + 8),
        fill
    );
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_keeps_same_piece_interior_unstroked() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board = vec![vec![1u8, 1u8]];
    let owners = vec![vec![Some(777u32), Some(777u32)]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let fill = color_for_cell(1);

    // Adjacent cells owned by the same piece should still look merged.
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + CELL_SIZE.saturating_sub(1),
            viewport.y + 8
        ),
        fill
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE, viewport.y + 8),
        fill
    );
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_draws_seam_for_mixed_owner_neighbors() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Left cell belongs to a piece; right cell is non-empty terrain with no owner.
    let board = vec![vec![1u8, 9u8]];
    let owners = vec![vec![Some(333u32), None]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let left_fill = color_for_cell(1);
    let left_stroke = [
        ((left_fill[0] as u16 * 11) / 20) as u8,
        ((left_fill[1] as u16 * 11) / 20) as u8,
        ((left_fill[2] as u16 * 11) / 20) as u8,
        left_fill[3],
    ];
    let right_fill = color_for_cell(9);

    // Seam is drawn once from the canonical side.
    assert_eq!(
        pixel_at(
            &frame,
            width,
            viewport.x + CELL_SIZE.saturating_sub(1),
            viewport.y + 8
        ),
        left_stroke
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE, viewport.y + 8),
        right_fill
    );
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_uses_three_px_internal_seams() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let board = vec![vec![1u8, 9u8]];
    let owners = vec![vec![Some(1u32), None]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let left_fill = color_for_cell(1);
    let left_stroke = [
        ((left_fill[0] as u16 * 11) / 20) as u8,
        ((left_fill[1] as u16 * 11) / 20) as u8,
        ((left_fill[2] as u16 * 11) / 20) as u8,
        left_fill[3],
    ];

    let y = viewport.y + 8;
    // Exactly 3 px at the right side of the left cell should be stroked.
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE - 3, y),
        left_stroke
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE - 2, y),
        left_stroke
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE - 1, y),
        left_stroke
    );
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE - 4, y),
        left_fill
    );
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_bridges_interior_seam_corners() {
    let width = 8 * CELL_SIZE;
    let height = 8 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // 2x2 filled board where seams form an L around the center junction:
    // - vertical interior seam between bottom-left (owner 1) and bottom-right (owner 2)
    // - horizontal interior seam between bottom-right (owner 2) and top-right (owner 1)
    let board = vec![vec![1u8, 1u8], vec![1u8, 1u8]];
    let owners = vec![vec![Some(1u32), Some(2u32)], vec![Some(1u32), Some(1u32)]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, 2 * CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let fill = color_for_cell(1);
    let stroke = [
        ((fill[0] as u16 * 11) / 20) as u8,
        ((fill[1] as u16 * 11) / 20) as u8,
        ((fill[2] as u16 * 11) / 20) as u8,
        fill[3],
    ];

    // Center junction pixel should be bridged by seam color (not fill).
    let cx = viewport.x + CELL_SIZE;
    let cy = viewport.y + CELL_SIZE;
    assert_eq!(pixel_at(&frame, width, cx, cy), stroke);
}

#[test]
fn draw_board_cells_in_rect_clipped_with_owners_keeps_unowned_same_type_neighbors_merged() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Two unowned cells of the same type should remain merged (no seam noise).
    let board = vec![vec![9u8, 9u8]];
    let owners = vec![vec![None, None]];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 2 * CELL_SIZE, CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped_with_owners(&mut gfx, &board, Some(&owners), viewport, viewport);

    let fill = color_for_cell(9);
    let y = viewport.y + 8;
    assert_eq!(
        pixel_at(&frame, width, viewport.x + CELL_SIZE - 1, y),
        fill
    );
    assert_eq!(pixel_at(&frame, width, viewport.x + CELL_SIZE, y), fill);
}

#[test]
fn draw_board_cells_in_rect_clipped_bridges_concave_inside_corners() {
    let width = 8 * CELL_SIZE;
    let height = 6 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Around empty cell (x=1, y=1), right and up are filled (1), and the
    // diagonal owner cell at (x=2, y=2) is a different type (2). The bridge
    // should use that owner's stroke color.
    let board = vec![
        vec![0u8, 0u8, 0u8],
        vec![0u8, 0u8, 1u8],
        vec![0u8, 1u8, 2u8],
    ];
    let viewport = Rect::new(CELL_SIZE, CELL_SIZE, 3 * CELL_SIZE, 3 * CELL_SIZE);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect_clipped(&mut gfx, &board, viewport, viewport);

    let fill = color_for_cell(2);
    let stroke = [
        ((fill[0] as u16 * 11) / 20) as u8,
        ((fill[1] as u16 * 11) / 20) as u8,
        ((fill[2] as u16 * 11) / 20) as u8,
        fill[3],
    ];

    // Sample inside the top-right concave bridge around empty cell (1,1).
    let bridge_x = viewport.x + (2 * CELL_SIZE) + 1;
    let bridge_y = viewport.y + CELL_SIZE.saturating_sub(2);
    assert_eq!(pixel_at(&frame, width, bridge_x, bridge_y), stroke);
}

#[test]
fn draw_board_cells_renders_grass_cells_by_tile_type() {
    let width = 6 * CELL_SIZE;
    let height = 8 * CELL_SIZE;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    // Bottom row grass, next row garbage.
    let board = vec![vec![12u8], vec![8u8], vec![0u8], vec![0u8]];
    let board_rect = Rect::new(
        CELL_SIZE,
        CELL_SIZE,
        CELL_SIZE,
        board.len() as u32 * CELL_SIZE,
    );

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_board_cells_in_rect(&mut gfx, &board, board_rect);

    let grass = [94u8, 152u8, 72u8, 255u8];
    let garbage = color_for_cell(8);

    // y=0 is bottom row in board space.
    let grass_row_inverted_y = (board.len() as u32 - 1).saturating_sub(0);
    let grass_row_px = board_rect.x + CELL_SIZE / 2;
    let grass_row_py = board_rect.y + grass_row_inverted_y * CELL_SIZE + CELL_SIZE / 2;
    assert_eq!(pixel_at(&frame, width, grass_row_px, grass_row_py), grass);

    let garbage_row_inverted_y = (board.len() as u32 - 1).saturating_sub(1);
    let garbage_row_px = board_rect.x + CELL_SIZE / 2;
    let garbage_row_py = board_rect.y + garbage_row_inverted_y * CELL_SIZE + CELL_SIZE / 2;
    assert_eq!(
        pixel_at(&frame, width, garbage_row_px, garbage_row_py),
        garbage
    );
}

fn pixel_at(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [frame[idx], frame[idx + 1], frame[idx + 2], frame[idx + 3]]
}
