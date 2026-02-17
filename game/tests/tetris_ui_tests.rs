use engine::graphics::CpuRenderer;
use engine::render::{CELL_SIZE, color_for_cell};
use engine::surface::SurfaceSize;
use engine::ui;
use engine::ui_tree::{UiInput, UiTree};

use game::skilltree::{
    SkillEffect, SkillNodeDef, SkillTreeDef, SkillTreeProgress, SkillTreeRuntime,
};
use game::tetris_core::{
    BOARD_HEIGHT, BOARD_WIDTH, CELL_GARBAGE, DEFAULT_BOTTOMWELL_ROWS, DepthWallDef, Piece,
    TetrisCore, Vec2i,
};
use game::tetris_ui::{
    MAIN_MENU_TITLE, SkillTreeLayout, draw_game_over_menu, draw_main_menu, draw_main_menu_with_ui,
    draw_pause_menu, draw_skilltree, draw_skilltree_runtime_with_ui, draw_tetris,
    draw_tetris_hud_with_ui, draw_tetris_world, draw_tetris_world_with_camera_offset,
};
use game::ui_ids::{UI_CANVAS, UI_TETRIS_PAUSE};

fn pixel_at(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let idx = ((y * width + x) * 4) as usize;
    [frame[idx], frame[idx + 1], frame[idx + 2], frame[idx + 3]]
}

fn count_color_in_rect(frame: &[u8], width: u32, rect: ui::Rect, color: [u8; 4]) -> usize {
    let mut matches = 0usize;
    let x_end = rect.x.saturating_add(rect.w);
    let y_end = rect.y.saturating_add(rect.h);
    for y in rect.y..y_end {
        for x in rect.x..x_end {
            if pixel_at(frame, width, x, y) == color {
                matches += 1;
            }
        }
    }
    matches
}

fn piece_edge_color(piece_fill: [u8; 4]) -> [u8; 4] {
    [
        ((piece_fill[0] as u16 * 11) / 20) as u8,
        ((piece_fill[1] as u16 * 11) / 20) as u8,
        ((piece_fill[2] as u16 * 11) / 20) as u8,
        piece_fill[3],
    ]
}

fn is_piece_fill_or_edge(px: [u8; 4], piece_fill: [u8; 4]) -> bool {
    px == piece_fill || px == piece_edge_color(piece_fill)
}

fn orthogonal_routing_fixture_runtime() -> SkillTreeRuntime {
    let mut runtime = SkillTreeRuntime::from_defaults();
    runtime.def = SkillTreeDef {
        version: 1,
        nodes: vec![
            SkillNodeDef {
                id: "src".to_string(),
                name: "SRC".to_string(),
                pos: Vec2i::new(0, 2),
                shape: vec![Vec2i::new(0, 0)],
                color: 2,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            },
            SkillNodeDef {
                id: "block".to_string(),
                name: "BLOCK".to_string(),
                pos: Vec2i::new(2, 2),
                shape: vec![Vec2i::new(0, 0)],
                color: 1,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            },
            SkillNodeDef {
                id: "dst".to_string(),
                name: "DST".to_string(),
                pos: Vec2i::new(4, 2),
                shape: vec![Vec2i::new(0, 0)],
                color: 3,
                cost: 0,
                requires: vec!["src".to_string()],
                effect: SkillEffect::None,
            },
        ],
    };
    runtime.progress = SkillTreeProgress {
        version: 1,
        money: 0,
        unlocked: vec!["src".to_string()],
    };
    runtime.rebuild_caches();
    runtime
}

fn direct_attachment_fixture_runtime() -> SkillTreeRuntime {
    let mut runtime = SkillTreeRuntime::from_defaults();
    runtime.def = SkillTreeDef {
        version: 1,
        nodes: vec![
            SkillNodeDef {
                id: "src".to_string(),
                name: "SRC".to_string(),
                pos: Vec2i::new(0, 0),
                shape: vec![Vec2i::new(0, 0)],
                color: 2,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            },
            SkillNodeDef {
                id: "dst".to_string(),
                name: "DST".to_string(),
                pos: Vec2i::new(3, 0),
                shape: vec![Vec2i::new(0, 0)],
                color: 3,
                cost: 0,
                requires: vec!["src".to_string()],
                effect: SkillEffect::None,
            },
        ],
    };
    runtime.progress = SkillTreeProgress {
        version: 1,
        money: 0,
        unlocked: vec!["src".to_string()],
    };
    runtime.rebuild_caches();
    runtime
}

fn skilltree_world_center_px(
    layout: SkillTreeLayout,
    runtime: &SkillTreeRuntime,
    world: Vec2i,
) -> (u32, u32) {
    let grid_cell = runtime.camera.cell_px.round().clamp(8.0, 64.0) as i32;
    let default_cam_min_x = -(layout.grid_cols as i32) / 2;
    let default_cam_min_y = 0i32;
    let cam_min_x = default_cam_min_x as f32 + runtime.camera.pan.x;
    let cam_min_y = default_cam_min_y as f32 + runtime.camera.pan.y;
    let grid_cam_min_x = cam_min_x.floor() as i32;
    let grid_cam_min_y = cam_min_y.floor() as i32;
    let frac_x = cam_min_x - grid_cam_min_x as f32;
    let frac_y = cam_min_y - grid_cam_min_y as f32;
    let grid_pan_px_x = -((frac_x * grid_cell as f32).round() as i32);
    let grid_pan_px_y = (frac_y * grid_cell as f32).round() as i32;
    let col = world.x - grid_cam_min_x;
    let row_from_bottom = world.y - grid_cam_min_y;
    let row_from_top = layout.grid_rows as i32 - 1 - row_from_bottom;
    let x = layout.grid_origin_x as i32 + col * grid_cell + grid_pan_px_x + grid_cell / 2;
    let y = layout.grid_origin_y as i32 + row_from_top * grid_cell + grid_pan_px_y + grid_cell / 2;
    (x.max(0) as u32, y.max(0) as u32)
}

fn quarter_step_toward(
    from: (u32, u32),
    to: (u32, u32),
    numerator: i32,
    denominator: i32,
) -> (u32, u32) {
    let fx = from.0 as i32;
    let fy = from.1 as i32;
    let tx = to.0 as i32;
    let ty = to.1 as i32;
    let x = fx.saturating_add((tx.saturating_sub(fx)).saturating_mul(numerator) / denominator);
    let y = fy.saturating_add((ty.saturating_sub(fy)).saturating_mul(numerator) / denominator);
    (x.max(0) as u32, y.max(0) as u32)
}

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
    assert_ne!(
        pixel, bg,
        "expected hold panel border to differ from background"
    );
}

#[test]
fn draw_tetris_world_background_is_deterministic_for_same_seed_and_depth() {
    let width = 800u32;
    let height = 600u32;

    let mut frame_a = vec![0u8; (width * height * 4) as usize];
    let mut frame_b = vec![0u8; (width * height * 4) as usize];

    let mut core_a = TetrisCore::new(1337);
    core_a.set_available_pieces(Piece::all());
    core_a.initialize_game();

    let mut core_b = TetrisCore::new(1337);
    core_b.set_available_pieces(Piece::all());
    core_b.initialize_game();

    let mut gfx_a = CpuRenderer::new(&mut frame_a, SurfaceSize::new(width, height));
    let mut gfx_b = CpuRenderer::new(&mut frame_b, SurfaceSize::new(width, height));
    let _ = draw_tetris_world(&mut gfx_a, width, height, &core_a);
    let _ = draw_tetris_world(&mut gfx_b, width, height, &core_b);

    assert_eq!(
        frame_a, frame_b,
        "same seed + same depth should produce identical world rendering"
    );
}

#[test]
fn draw_tetris_world_background_scrolls_when_depth_increases() {
    if std::env::var("ROLLOUT_DISABLE_TILE_BG")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
    {
        return;
    }

    let width = 800u32;
    let height = 600u32;

    let mut frame_depth0 = vec![0u8; (width * height * 4) as usize];
    let mut frame_depth1 = vec![0u8; (width * height * 4) as usize];

    let mut core_depth0 = TetrisCore::new(2026);
    core_depth0.set_available_pieces(Piece::all());
    core_depth0.set_bottomwell_enabled(true);
    core_depth0.initialize_game();

    let mut core_depth1 = TetrisCore::new(2026);
    core_depth1.set_available_pieces(Piece::all());
    core_depth1.set_bottomwell_enabled(true);
    core_depth1.initialize_game();

    let target_y = 0;
    for x in 0..BOARD_WIDTH {
        core_depth1.set_cell(x, target_y, CELL_GARBAGE);
    }
    assert_eq!(core_depth1.clear_lines(), 1);
    assert!(
        core_depth1.background_depth_rows() > core_depth0.background_depth_rows(),
        "expected canonical background depth to increase after reveal"
    );

    let mut gfx_depth0 = CpuRenderer::new(&mut frame_depth0, SurfaceSize::new(width, height));
    let mut gfx_depth1 = CpuRenderer::new(&mut frame_depth1, SurfaceSize::new(width, height));
    let _ = draw_tetris_world(&mut gfx_depth0, width, height, &core_depth0);
    let _ = draw_tetris_world(&mut gfx_depth1, width, height, &core_depth1);

    assert_ne!(
        frame_depth0, frame_depth1,
        "increasing depth should change visible background tiles"
    );
}

#[test]
fn draw_tetris_world_zero_camera_offset_matches_wrapper_output() {
    let width = 800u32;
    let height = 600u32;

    let mut frame_a = vec![0u8; (width * height * 4) as usize];
    let mut frame_b = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(7);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let mut gfx_a = CpuRenderer::new(&mut frame_a, SurfaceSize::new(width, height));
    let mut gfx_b = CpuRenderer::new(&mut frame_b, SurfaceSize::new(width, height));
    let _ = draw_tetris_world(&mut gfx_a, width, height, &core);
    let _ = draw_tetris_world_with_camera_offset(&mut gfx_b, width, height, &core, 0);

    assert_eq!(
        frame_a, frame_b,
        "zero world camera offset should preserve deterministic render output"
    );
}

#[test]
fn draw_tetris_world_nonzero_camera_offset_shifts_world_pixels() {
    let width = 800u32;
    let height = 600u32;
    let mut frame_base = vec![0u8; (width * height * 4) as usize];
    let mut frame_shifted = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(11);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_cell(0, BOARD_HEIGHT - 1, 1);

    let mut gfx_base = CpuRenderer::new(&mut frame_base, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx_base, width, height, &core);

    let offset_y = CELL_SIZE as i32;
    let mut gfx_shifted = CpuRenderer::new(&mut frame_shifted, SurfaceSize::new(width, height));
    let _ = draw_tetris_world_with_camera_offset(&mut gfx_shifted, width, height, &core, offset_y);

    let piece = color_for_cell(1);
    let sample_x = layout.board.x + CELL_SIZE / 2;
    let sample_y_base = layout.board.y + 1;
    let sample_y_shifted = sample_y_base + CELL_SIZE;

    let idx_base = ((sample_y_base * width + sample_x) * 4) as usize;
    let idx_shifted_same = ((sample_y_base * width + sample_x) * 4) as usize;
    let idx_shifted_offset = ((sample_y_shifted * width + sample_x) * 4) as usize;

    let mut px_base = [0u8; 4];
    px_base.copy_from_slice(&frame_base[idx_base..idx_base + 4]);
    assert!(
        is_piece_fill_or_edge(px_base, piece),
        "baseline should draw board cell at unshifted position"
    );
    assert_ne!(
        &frame_shifted[idx_shifted_same..idx_shifted_same + 4],
        &piece,
        "shifted frame should not keep board cell at original position"
    );
    let mut px_shifted = [0u8; 4];
    px_shifted.copy_from_slice(&frame_shifted[idx_shifted_offset..idx_shifted_offset + 4]);
    assert!(
        is_piece_fill_or_edge(px_shifted, piece),
        "shifted frame should draw board cell at offset position"
    );
}

#[test]
fn draw_tetris_world_large_positive_offset_clamps_to_bottom_margin() {
    let width = 800u32;
    let height = 600u32;
    let mut frame_shifted = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(23);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_cell(0, BOARD_HEIGHT - 1, 1);

    let mut frame_base = vec![0u8; (width * height * 4) as usize];
    let mut gfx_base = CpuRenderer::new(&mut frame_base, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx_base, width, height, &core);

    let max_down = height.saturating_sub(layout.board.y.saturating_add(layout.board.h));
    let mut gfx_shifted = CpuRenderer::new(&mut frame_shifted, SurfaceSize::new(width, height));
    let _ = draw_tetris_world_with_camera_offset(&mut gfx_shifted, width, height, &core, 10_000);

    let piece = color_for_cell(1);
    let sample_x = layout.board.x + 1;
    let sample_y_base = layout.board.y + 1;
    let sample_y_clamped = sample_y_base + max_down;
    let idx = ((sample_y_clamped * width + sample_x) * 4) as usize;

    let mut px = [0u8; 4];
    px.copy_from_slice(&frame_shifted[idx..idx + 4]);
    assert!(
        is_piece_fill_or_edge(px, piece),
        "large positive camera offsets should clamp so board stays fully visible"
    );
}

#[test]
fn draw_tetris_world_large_negative_offset_clamps_to_top_margin() {
    let width = 800u32;
    let height = 600u32;
    let mut frame_shifted = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(29);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_cell(0, 0, 1);

    let mut frame_base = vec![0u8; (width * height * 4) as usize];
    let mut gfx_base = CpuRenderer::new(&mut frame_base, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx_base, width, height, &core);

    let max_up = layout.board.y;
    let mut gfx_shifted = CpuRenderer::new(&mut frame_shifted, SurfaceSize::new(width, height));
    let _ = draw_tetris_world_with_camera_offset(&mut gfx_shifted, width, height, &core, -10_000);

    let piece = color_for_cell(1);
    let sample_x = layout.board.x + 1;
    let sample_y_base = layout.board.y + (BOARD_HEIGHT as u32 - 1) * CELL_SIZE + 1;
    let sample_y_clamped = sample_y_base.saturating_sub(max_up);
    let idx = ((sample_y_clamped * width + sample_x) * 4) as usize;

    let mut px = [0u8; 4];
    px.copy_from_slice(&frame_shifted[idx..idx + 4]);
    assert!(
        is_piece_fill_or_edge(px, piece),
        "large negative camera offsets should clamp so board stays fully visible"
    );
}

#[test]
fn draw_tetris_world_camera_offset_clips_world_layers_to_board_viewport() {
    let width = 800u32;
    let height = 600u32;
    let mut frame_shifted = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(31);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    for y in 0..BOARD_HEIGHT {
        core.set_cell(0, y, 1);
    }

    let mut frame_base = vec![0u8; (width * height * 4) as usize];
    let mut gfx_base = CpuRenderer::new(&mut frame_base, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx_base, width, height, &core);

    let mut gfx_shifted = CpuRenderer::new(&mut frame_shifted, SurfaceSize::new(width, height));
    let _ = draw_tetris_world_with_camera_offset(
        &mut gfx_shifted,
        width,
        height,
        &core,
        CELL_SIZE as i32 * 2,
    );

    let piece = color_for_cell(1);
    let sample_x = layout.board.x + 1;
    let inside_y = layout.board.y + (layout.board.h / 2);
    let inside_px = pixel_at(&frame_shifted, width, sample_x, inside_y);
    assert!(
        is_piece_fill_or_edge(inside_px, piece),
        "sanity check: shifted world should still render board content in the viewport"
    );

    let above_y = layout.board.y.saturating_sub(1);
    let below_y = layout.board.y.saturating_add(layout.board.h);
    assert_ne!(
        pixel_at(&frame_shifted, width, sample_x, above_y),
        piece,
        "world pixels should be clipped above the fixed board viewport"
    );
    if below_y < height {
        assert_ne!(
            pixel_at(&frame_shifted, width, sample_x, below_y),
            piece,
            "world pixels should be clipped below the fixed board viewport"
        );
    }
}

#[test]
fn draw_tetris_world_camera_offset_keeps_top_edge_background_persistent() {
    if std::env::var("ROLLOUT_DISABLE_TILE_BG")
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
    {
        return;
    }

    let width = 800u32;
    let height = 600u32;
    let mut frame_shifted = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(37);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let mut gfx_shifted = CpuRenderer::new(&mut frame_shifted, SurfaceSize::new(width, height));
    let layout = draw_tetris_world_with_camera_offset(
        &mut gfx_shifted,
        width,
        height,
        &core,
        (CELL_SIZE as i32) / 2,
    );

    let sample = pixel_at(
        &frame_shifted,
        width,
        layout.board.x + 1,
        layout.board.y + 1,
    );
    assert_ne!(
        sample,
        color_for_cell(0),
        "top edge should remain background-filled under partial downward camera offsets"
    );
}

#[test]
fn draw_tetris_world_grassline_stays_behind_bottomwell_cells() {
    let env_truthy = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false)
    };
    if env_truthy("ROLLOUT_DISABLE_TILE_BG")
        || env_truthy("ROLLOUT_DISABLE_LAYER_NEAR")
        || env_truthy("ROLLOUT_BG_FORCE_LEGACY_UNDERGROUND_START")
        || std::env::var("ROLLOUT_FORCE_BIOME").is_ok()
    {
        return;
    }

    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(41);
    core.set_available_pieces(vec![Piece::O]);
    core.set_bottomwell_enabled(true);
    core.initialize_game();

    // At run start, the Overworld->Dirt separator aligns with board row y=2.
    let surface_boundary_y = 2usize;
    core.set_cell(0, surface_boundary_y, 1);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx, width, height, &core);

    let sample_x = layout.board.x + 1;
    let board_h = core.board().len() as u32;
    let inverted_y = board_h.saturating_sub(1).saturating_sub(surface_boundary_y as u32);
    let sample_y = layout.board.y + inverted_y * CELL_SIZE + CELL_SIZE / 2;
    let sample = pixel_at(&frame, width, sample_x, sample_y);

    let filled_cell = color_for_cell(1);
    let filled_edge = piece_edge_color(filled_cell);
    let grassline = [94, 152, 72, 255];
    assert!(
        sample == filled_cell || sample == filled_edge,
        "boundary pixel should come from locked-board rendering (fill or edge), not grass overlay"
    );
    assert_ne!(
        sample, grassline,
        "boundary pixel should not show grassline over placed/locked cells"
    );
}

fn make_depth_locked_core_for_ui(seed: u64, hp: u32) -> TetrisCore {
    let mut core = TetrisCore::new(seed);
    core.set_available_pieces(Piece::all());
    core.set_bottomwell_enabled(true);
    core.set_depth_wall_defs(vec![DepthWallDef {
        id: "ui_test_wall".to_string(),
        depth_trigger: (DEFAULT_BOTTOMWELL_ROWS as u64).saturating_add(1),
        hp,
        biome_from: "dirt".to_string(),
        biome_to: "stone".to_string(),
    }]);
    core.initialize_game();
    for x in 0..BOARD_WIDTH {
        core.set_cell(x, 0, CELL_GARBAGE);
    }
    assert_eq!(core.clear_lines(), 1, "setup should activate the wall");
    assert!(
        core.depth_progress_paused(),
        "setup should leave depth progression paused"
    );
    core
}

#[test]
fn draw_tetris_world_depth_wall_overlay_obscures_bottom_rows_when_locked() {
    let width = 800u32;
    let height = 600u32;
    let mut frame_locked = vec![0u8; (width * height * 4) as usize];
    let mut frame_unlocked = vec![0u8; (width * height * 4) as usize];

    let mut locked = make_depth_locked_core_for_ui(57, 18);
    let mut unlocked = TetrisCore::new(57);
    unlocked.set_available_pieces(Piece::all());
    unlocked.initialize_game();

    locked.set_cell(0, 0, 1);
    unlocked.set_cell(0, 0, 1);

    let mut gfx_locked = CpuRenderer::new(&mut frame_locked, SurfaceSize::new(width, height));
    let mut gfx_unlocked = CpuRenderer::new(&mut frame_unlocked, SurfaceSize::new(width, height));
    let layout_locked = draw_tetris_world(&mut gfx_locked, width, height, &locked);
    let layout_unlocked = draw_tetris_world(&mut gfx_unlocked, width, height, &unlocked);

    let x_locked = layout_locked.board.x + CELL_SIZE / 2;
    let y_locked = layout_locked.board.y + (BOARD_HEIGHT as u32 - 1) * CELL_SIZE + CELL_SIZE / 2;
    let x_unlocked = layout_unlocked.board.x + CELL_SIZE / 2;
    let y_unlocked =
        layout_unlocked.board.y + (BOARD_HEIGHT as u32 - 1) * CELL_SIZE + CELL_SIZE / 2;

    let piece = color_for_cell(1);
    let locked_px = pixel_at(&frame_locked, width, x_locked, y_locked);
    let unlocked_px = pixel_at(&frame_unlocked, width, x_unlocked, y_unlocked);
    assert!(
        is_piece_fill_or_edge(unlocked_px, piece),
        "sanity check: unlocked world should still show the bottom board cell"
    );
    assert!(
        !is_piece_fill_or_edge(locked_px, piece),
        "locked world should obscure the bottom rows with the depth wall overlay"
    );
}

#[test]
fn draw_tetris_world_depth_wall_overlay_draws_hp_text() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];
    let mut core = make_depth_locked_core_for_ui(61, 11);

    core.set_cell(0, 0, 1);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx, width, height, &core);
    let board_highlight_count =
        count_color_in_rect(&frame, width, layout.board, [255, 188, 112, 255]);
    assert!(
        board_highlight_count > 0,
        "depth wall overlay should render HP text using the depth-locked highlight color"
    );
}

#[test]
fn world_camera_offset_keeps_hud_layout_and_pause_hit_target_stable() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(19);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout_base = draw_tetris_world_with_camera_offset(&mut gfx, width, height, &core, 0);
    let layout_shifted =
        draw_tetris_world_with_camera_offset(&mut gfx, width, height, &core, CELL_SIZE as i32 * 2);

    assert_eq!(
        layout_base, layout_shifted,
        "world camera offset should not move HUD/input anchor layout"
    );

    let pause_x = layout_base.pause_button.x + layout_base.pause_button.w / 2;
    let pause_y = layout_base.pause_button.y + layout_base.pause_button.h / 2;

    let mut ui_base = UiTree::new();
    ui_base.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_base.add_root(UI_CANVAS);
    let mut gfx_base = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_tetris_hud_with_ui(
        &mut gfx_base,
        width,
        height,
        &core,
        layout_base,
        &mut ui_base,
    );
    let _ = ui_base.process_input(UiInput {
        mouse_pos: Some((pause_x, pause_y)),
        mouse_down: false,
        mouse_up: false,
    });
    assert!(
        ui_base.is_hovered(UI_TETRIS_PAUSE),
        "pause hit target should remain at the same screen-space position"
    );

    let mut ui_shifted = UiTree::new();
    ui_shifted.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_shifted.add_root(UI_CANVAS);
    let mut gfx_shifted = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    draw_tetris_hud_with_ui(
        &mut gfx_shifted,
        width,
        height,
        &core,
        layout_shifted,
        &mut ui_shifted,
    );
    let _ = ui_shifted.process_input(UiInput {
        mouse_pos: Some((pause_x, pause_y)),
        mouse_down: false,
        mouse_up: false,
    });
    assert!(
        ui_shifted.is_hovered(UI_TETRIS_PAUSE),
        "pause hit target hover should match even when world layer is offset"
    );
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

    assert_ne!(
        ghost_pixel, bg,
        "ghost cell should be drawn over background"
    );
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
fn draw_tetris_world_flashes_pending_line_clear_rows() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    // Fill bottom row except O-piece landing columns; hard-drop to trigger delayed clear.
    for x in 0..BOARD_WIDTH {
        if x == 4 || x == 5 {
            continue;
        }
        core.set_cell(x, 0, 1);
    }
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    core.hard_drop();
    assert!(core.is_line_clear_active());

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let layout = draw_tetris_world(&mut gfx, width, height, &core);

    let sample_x = layout.board.x + 1;
    let inverted_y = (BOARD_HEIGHT as u32 - 1) - 0;
    let sample_y = layout.board.y + inverted_y * CELL_SIZE + 1;
    let sample = pixel_at(&frame, width, sample_x, sample_y);
    assert_ne!(
        sample,
        color_for_cell(1),
        "pending clear row should be flash-blended above base block color"
    );
    assert_ne!(
        sample,
        color_for_cell(0),
        "pending clear row should still render as a non-background filled row"
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
    assert!(layout.skilltree_editor_button.w > 0 && layout.skilltree_editor_button.h > 0);
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
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let _ = draw_main_menu_with_ui(&mut gfx_hover, width, height, &mut ui_tree);
    let _ = ui_tree.process_input(UiInput {
        mouse_pos: Some((hover_x, hover_y)),
        mouse_down: false,
        mouse_up: false,
    });
    ui_tree.begin_frame();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let _layout_hover = draw_main_menu_with_ui(&mut gfx_hover, width, height, &mut ui_tree);
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
    let mut painted_px = [0u8; 4];
    painted_px.copy_from_slice(&frame[idx..idx + 4]);
    assert!(
        is_piece_fill_or_edge(painted_px, piece_color),
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

    let start_idx =
        ((layout.start_new_game_button.y * width + layout.start_new_game_button.x) * 4) as usize;
    assert_ne!(
        &frame[start_idx..start_idx + 4],
        &bg,
        "expected the skilltree start button to draw over the background"
    );
}

#[test]
fn draw_skilltree_editor_hides_start_new_game_button() {
    let width = 800u32;
    let height = 600u32;

    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut runtime = SkillTreeRuntime::load_default();
    runtime.editor.enabled = true;

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let layout = draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);
    assert!(layout.panel.w > 0 && layout.panel.h > 0);
    assert_eq!(layout.start_new_game_button.w, 0);
    assert_eq!(layout.start_new_game_button.h, 0);
}

#[test]
fn draw_skilltree_draws_dependency_arrows() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let runtime = direct_attachment_fixture_runtime();
    draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);

    let link_color = [110u8, 110, 150, 255];
    let found = frame.chunks_exact(4).any(|px| px == link_color);
    assert!(found, "expected dependency arrow pixel to be drawn");
}

#[test]
fn draw_skilltree_routes_links_with_orthogonal_segments_around_blockers() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let runtime = orthogonal_routing_fixture_runtime();
    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let layout = draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);

    let link_color = [110u8, 110, 150, 255];
    let mut lane_pixels = Vec::new();
    for y in [1, 3] {
        let (sample_x, sample_y) = skilltree_world_center_px(layout, &runtime, Vec2i::new(2, y));
        if sample_x < width && sample_y < height {
            lane_pixels.push(pixel_at(&frame, width, sample_x, sample_y));
        }
    }
    assert!(
        !lane_pixels.is_empty(),
        "expected at least one detour lane sample inside the viewport"
    );
    assert!(
        lane_pixels.into_iter().any(|px| px == link_color),
        "expected routed link to occupy an orthogonal detour lane around the blocker"
    );
}

#[test]
fn draw_skilltree_routed_links_render_under_non_default_camera_pan_and_zoom() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut runtime = orthogonal_routing_fixture_runtime();
    runtime.camera.cell_px = 28.0;
    runtime.camera.target_cell_px = 28.0;
    runtime.camera.pan.x = 1.25;
    runtime.camera.pan.y = -0.75;
    runtime.camera.target_pan = runtime.camera.pan;

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let layout = draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);

    let link_color = [110u8, 110, 150, 255];
    let found = frame.chunks_exact(4).any(|px| px == link_color);
    assert!(
        found,
        "expected routed dependency link pixels under pan/zoom"
    );

    let mut lane_pixels = Vec::new();
    for y in [1, 3] {
        let (sample_x, sample_y) = skilltree_world_center_px(layout, &runtime, Vec2i::new(2, y));
        if sample_x < width && sample_y < height {
            lane_pixels.push(pixel_at(&frame, width, sample_x, sample_y));
        }
    }
    assert!(
        !lane_pixels.is_empty(),
        "expected at least one transformed detour lane sample in-bounds"
    );
    assert!(
        lane_pixels.into_iter().any(|px| px == link_color),
        "expected routed detour sample to remain attached after camera transform"
    );
}

#[test]
fn draw_skilltree_links_attach_directly_to_node_edges() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let runtime = direct_attachment_fixture_runtime();
    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    let layout = draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);

    let link_color = [110u8, 110, 150, 255];

    let source_center = skilltree_world_center_px(layout, &runtime, Vec2i::new(0, 0));
    let source_port_center = skilltree_world_center_px(layout, &runtime, Vec2i::new(1, 0));
    let source_attachment = quarter_step_toward(source_center, source_port_center, 3, 4);

    let target_center = skilltree_world_center_px(layout, &runtime, Vec2i::new(3, 0));
    let target_port_center = skilltree_world_center_px(layout, &runtime, Vec2i::new(2, 0));
    let target_attachment = quarter_step_toward(target_center, target_port_center, 3, 4);

    assert!(
        source_attachment.0 < width
            && source_attachment.1 < height
            && target_attachment.0 < width
            && target_attachment.1 < height,
        "expected attachment samples to be visible in viewport"
    );

    assert_eq!(
        pixel_at(&frame, width, source_attachment.0, source_attachment.1),
        link_color,
        "expected source edge sample to include routed link color"
    );
    assert_eq!(
        pixel_at(&frame, width, target_attachment.0, target_attachment.1),
        link_color,
        "expected target edge sample to include routed link color"
    );
}

#[test]
fn draw_skilltree_editor_draws_keyboard_cursor_indicator() {
    let width = 800u32;
    let height = 600u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut runtime = SkillTreeRuntime::load_default();
    runtime.editor.enabled = true;
    runtime.editor.cursor_world = Vec2i::new(0, 0);

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    draw_skilltree_runtime_with_ui(&mut gfx, width, height, &mut ui_tree, &runtime);

    let cursor_color = [255u8, 220, 120, 255];
    let found = frame.chunks_exact(4).any(|px| px == cursor_color);
    assert!(
        found,
        "expected keyboard cursor indicator pixel to be drawn"
    );
}

#[test]
fn draw_skilltree_editor_help_toggle_changes_rendered_overlay() {
    let width = 800u32;
    let height = 600u32;

    let mut runtime = SkillTreeRuntime::load_default();
    runtime.editor.enabled = true;

    let mut frame_compact = vec![0u8; (width * height * 4) as usize];
    let mut gfx_compact = CpuRenderer::new(&mut frame_compact, SurfaceSize::new(width, height));
    let mut ui_tree_compact = UiTree::new();
    ui_tree_compact.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree_compact.add_root(UI_CANVAS);
    draw_skilltree_runtime_with_ui(
        &mut gfx_compact,
        width,
        height,
        &mut ui_tree_compact,
        &runtime,
    );

    runtime.editor.help_expanded = true;
    let mut frame_expanded = vec![0u8; (width * height * 4) as usize];
    let mut gfx_expanded = CpuRenderer::new(&mut frame_expanded, SurfaceSize::new(width, height));
    let mut ui_tree_expanded = UiTree::new();
    ui_tree_expanded.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree_expanded.add_root(UI_CANVAS);
    draw_skilltree_runtime_with_ui(
        &mut gfx_expanded,
        width,
        height,
        &mut ui_tree_expanded,
        &runtime,
    );

    assert_ne!(
        frame_compact, frame_expanded,
        "expanded help should render additional overlay content"
    );
}
