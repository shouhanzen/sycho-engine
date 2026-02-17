use std::collections::HashSet;

use game::tetris_core::{
    BOARD_HEIGHT, BOARD_WIDTH, CELL_DIRT, CELL_EMPTY, CELL_GARBAGE, CELL_GLASS, CELL_MOSS,
    CELL_MOSS_SEED, CELL_SAND, CELL_STONE,
    DEFAULT_BOTTOMWELL_ROWS, GravityAdvanceResult, LINE_CLEAR_DELAY_MS_DEFAULT,
    LOCK_DELAY_MAX_MS_DEFAULT, LOCK_DELAY_MS_DEFAULT, NEXT_QUEUE_LEN, Piece, RotationDir,
    TetrisCore, Vec2i,
};

fn grounded_o_piece_core() -> TetrisCore {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    core
}

#[test]
fn initializes_board_and_spawns_piece() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    assert_eq!(core.board().len(), BOARD_HEIGHT);
    for row in core.board() {
        assert_eq!(row.len(), BOARD_WIDTH);
        assert!(row.iter().all(|&cell| cell == 0));
    }

    assert!(core.current_piece().is_some());
    assert!(core.next_piece().is_some());
    assert_eq!(core.next_queue().len(), NEXT_QUEUE_LEN);
    assert!(core.held_piece().is_none());
    assert!(core.can_hold());
    assert_eq!(core.lines_cleared(), 0);
    assert_eq!(core.score(), 0);
    assert!(!core.snapshot().game_over);
    assert_eq!(core.current_piece_pos(), Vec2i::new(4, BOARD_HEIGHT as i32));
    assert_eq!(core.current_piece_rotation(), 0);
}

#[test]
fn hard_drop_advances_next_piece() {
    let mut core = TetrisCore::new(123);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let before_current = core.current_piece();
    let before_next = core.next_piece();
    let before_queue = core.next_queue().to_vec();
    assert!(before_current.is_some());
    assert!(before_next.is_some());
    assert!(before_queue.len() >= 2);

    let drop_distance = core.hard_drop();
    assert!(drop_distance >= 0);

    assert_eq!(core.current_piece(), before_next);
    assert!(core.next_piece().is_some());
    assert_eq!(core.next_queue()[0], before_queue[1]);
}

#[test]
fn hold_stores_piece_and_is_only_usable_once_per_spawn() {
    let mut core = TetrisCore::new(7);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let before_current = core.current_piece().unwrap();
    let before_queue = core.next_queue().to_vec();

    assert!(core.hold_piece());
    assert_eq!(core.held_piece(), Some(before_current));
    assert_eq!(core.current_piece(), Some(before_queue[0]));
    assert!(!core.can_hold());
    assert_eq!(core.next_queue()[0], before_queue[1]);

    // Can't hold again until a new piece spawns.
    let snap_before_second_hold = core.snapshot();
    assert!(!core.hold_piece());
    assert_eq!(core.snapshot(), snap_before_second_hold);
}

#[test]
fn hold_swaps_with_held_piece_after_spawn_without_consuming_queue() {
    let mut core = TetrisCore::new(999);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    let initial_current = core.current_piece().unwrap();
    assert!(core.hold_piece());
    assert_eq!(core.held_piece(), Some(initial_current));

    // Spawn a new piece (hard drop locks + spawns in this simplified core).
    core.hard_drop();
    assert!(core.can_hold());

    let before_swap_current = core.current_piece().unwrap();
    let before_swap_queue = core.next_queue().to_vec();

    assert!(core.hold_piece());
    assert_eq!(core.current_piece(), Some(initial_current));
    assert_eq!(core.held_piece(), Some(before_swap_current));
    assert!(!core.can_hold());
    assert_eq!(core.next_queue(), before_swap_queue);
}

#[test]
fn weighted_pool_draws_from_all_available_pieces() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());

    let mut seen = HashSet::new();
    for _ in 0..200 {
        seen.insert(core.draw_piece());
    }
    assert_eq!(seen.len(), Piece::all().len());
}

#[test]
fn moss_seed_piece_can_be_drawn_from_default_pool() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());

    let mut saw_moss_seed = false;
    for _ in 0..200 {
        if core.draw_piece() == Piece::MossSeed {
            saw_moss_seed = true;
            break;
        }
    }

    assert!(saw_moss_seed, "expected moss seed to be draw-able from pool");
}

#[test]
fn valid_position_rejects_out_of_bounds() {
    let mut core = TetrisCore::new(1);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_current_piece_for_test(Piece::T, Vec2i::new(4, 5), 0);

    assert!(core.is_valid_position(Vec2i::new(4, 5), 0));
    assert!(!core.is_valid_position(Vec2i::new(-1, 5), 0));
    assert!(!core.is_valid_position(Vec2i::new(15, 5), 0));
    assert!(!core.is_valid_position(Vec2i::new(4, -1), 0));
}

#[test]
fn clear_lines_removes_full_rows() {
    let mut core = TetrisCore::new(7);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    for x in 0..BOARD_WIDTH {
        core.set_cell(x, 0, 1);
    }

    let cleared = core.clear_lines();
    assert_eq!(cleared, 1);
    assert_eq!(core.lines_cleared(), 1);
    assert_eq!(core.score(), 100);

    let top_row = &core.board()[BOARD_HEIGHT - 1];
    assert!(top_row.iter().all(|&cell| cell == 0));
}

#[test]
fn background_depth_rows_advances_after_clearing_bottomwell_garbage() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());
    core.set_bottomwell_enabled(true);
    core.initialize_game();

    let depth_after_init = core.background_depth_rows();
    assert_eq!(depth_after_init, 0);

    let target_y = 0;
    for x in 0..BOARD_WIDTH {
        core.set_cell(x, target_y, CELL_GARBAGE);
    }
    let cleared = core.clear_lines();
    assert_eq!(cleared, 1);
    assert_eq!(core.background_depth_rows(), depth_after_init + 1);
}

#[test]
fn background_depth_rows_does_not_advance_on_non_bottomwell_clear() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());
    core.set_bottomwell_enabled(true);
    core.initialize_game();

    let depth_after_init = core.background_depth_rows();
    assert_eq!(depth_after_init, 0);

    let target_y = DEFAULT_BOTTOMWELL_ROWS;
    for x in 0..BOARD_WIDTH {
        core.set_cell(x, target_y, 1);
    }
    let cleared = core.clear_lines();
    assert_eq!(cleared, 1);
    assert_eq!(core.background_depth_rows(), depth_after_init);
}

#[test]
fn background_depth_rows_remains_zero_without_bottomwell_reveals() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());
    core.initialize_game();

    for x in 0..BOARD_WIDTH {
        core.set_cell(x, 0, 1);
    }
    let _ = core.clear_lines();

    assert_eq!(core.background_depth_rows(), 0);
}

#[test]
fn hard_drop_awards_score_for_drop_distance() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    // From y=10, an O piece can hard-drop to y=1 on an empty board (distance = 9).
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 10), 0);
    let drop_distance = core.hard_drop();

    assert_eq!(drop_distance, 9);
    assert_eq!(core.score(), 18);
}

#[test]
fn wood_spear_tip_breaks_dirt_during_hard_drop() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_current_piece_for_test(Piece::I, Vec2i::new(4, 6), 1);
    core.set_cell(5, 1, 4);

    let drop_distance = core.hard_drop();

    assert_eq!(drop_distance, 4, "wood spear should break dirt and keep falling");
    assert_eq!(core.board()[1][5], 1, "wood should occupy the broken dirt lane");
}

#[test]
fn wood_spear_tip_does_not_break_stone() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_current_piece_for_test(Piece::I, Vec2i::new(4, 6), 1);
    core.set_cell(5, 1, 2);

    let drop_distance = core.hard_drop();

    assert_eq!(
        drop_distance, 2,
        "wood spear should stop when its tip hits stone"
    );
    assert_eq!(core.board()[1][5], 2, "stone remains intact");
}

#[test]
fn wood_spear_tip_facing_down_pierces_multiple_dirt_tiles() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(Piece::all());
    core.initialize_game();
    core.set_current_piece_for_test(Piece::I, Vec2i::new(4, 7), 1);
    core.set_cell(5, 2, CELL_DIRT);
    core.set_cell(5, 1, CELL_DIRT);

    let drop_distance = core.hard_drop();

    assert!(
        drop_distance > 0,
        "wood spear should continue after piercing stacked dirt"
    );
    assert_ne!(
        core.board()[2][5],
        CELL_DIRT,
        "upper dirt tile should be crushed"
    );
    assert_eq!(
        core.board()[1][5],
        1,
        "wood spear should pierce through and occupy the lane"
    );
}

#[test]
fn moss_seed_spreads_one_tile_per_turn_up_to_bfs_distance_three() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    // Seed at (4, 1); create a dirt lane out to distance 4 to verify cap at 3.
    core.set_cell(4, 1, CELL_MOSS_SEED);
    core.set_cell(3, 1, CELL_DIRT);
    core.set_cell(2, 1, CELL_DIRT);
    core.set_cell(1, 1, CELL_DIRT);
    core.set_cell(0, 1, CELL_DIRT);

    core.advance_material_turn();
    assert_eq!(core.board()[1][3], CELL_MOSS);
    assert_eq!(core.board()[1][2], CELL_DIRT);
    assert_eq!(core.board()[1][1], CELL_DIRT);
    assert_eq!(core.board()[1][0], CELL_DIRT);

    core.advance_material_turn();
    assert_eq!(core.board()[1][2], CELL_MOSS);
    assert_eq!(core.board()[1][1], CELL_DIRT);
    assert_eq!(core.board()[1][0], CELL_DIRT);

    core.advance_material_turn();
    assert_eq!(core.board()[1][1], CELL_MOSS);
    assert_eq!(core.board()[1][0], CELL_DIRT);

    // Distance-4 tile stays dirt forever because growth radius is capped at 3.
    core.advance_material_turn();
    core.advance_material_turn();
    assert_eq!(core.board()[1][0], CELL_DIRT);
}

#[test]
fn moss_does_not_spread_through_non_dirt_barrier() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    // Stone barrier at x=3 blocks propagation from seed at x=4 to dirt at x=2.
    core.set_cell(4, 1, CELL_MOSS_SEED);
    core.set_cell(3, 1, 2);
    core.set_cell(2, 1, CELL_DIRT);

    for _ in 0..4 {
        core.advance_material_turn();
    }

    assert_eq!(
        core.board()[1][2],
        CELL_DIRT,
        "moss should not cross non-dirt barrier tiles"
    );
}

#[test]
fn hard_drop_breaks_moss_for_non_spear_piece() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 6), 0);
    core.set_cell(4, 1, CELL_MOSS);

    let drop_distance = core.hard_drop();

    assert!(drop_distance > 0, "piece should continue dropping after breaking moss");
    assert_eq!(
        core.board()[1][4],
        2,
        "O piece should occupy the lane after breaking moss"
    );
}

#[test]
fn hard_drop_crushes_only_contacts_under_threshold() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 6), 0);
    core.set_cell(4, 1, CELL_MOSS);
    core.set_cell(5, 1, CELL_STONE);

    core.hard_drop();

    assert_eq!(
        core.board()[1][4],
        CELL_EMPTY,
        "moss contact should crush when under threshold"
    );
    assert_eq!(
        core.board()[1][5],
        CELL_STONE,
        "stone support should still block and remain"
    );
}

#[test]
fn sand_t_piece_locks_as_true_t_shape() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::T]);
    core.initialize_game();

    core.set_current_piece_for_test(Piece::T, Vec2i::new(4, 3), 0);
    core.hard_drop();

    // True T: 3-wide base at y=0 and center cap at y=1.
    assert_eq!(core.board()[0][3], CELL_SAND);
    assert_eq!(core.board()[0][4], CELL_SAND);
    assert_eq!(core.board()[0][5], CELL_SAND);
    assert_eq!(core.board()[1][4], CELL_SAND);
}

#[test]
fn glass_piece_shatters_when_spear_tip_breaks_it() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::Glass, Piece::I]);
    core.initialize_game();

    // Lock a glass I3 on the floor.
    core.set_current_piece_for_test(Piece::Glass, Vec2i::new(4, 3), 0);
    core.hard_drop();
    assert_eq!(core.board()[0][3], CELL_GLASS);
    assert_eq!(core.board()[0][4], CELL_GLASS);
    assert_eq!(core.board()[0][5], CELL_GLASS);

    // Spear tip should shatter the full glass piece.
    core.set_current_piece_for_test(Piece::I, Vec2i::new(4, 7), 1);
    core.hard_drop();
    assert_eq!(core.glass_shatter_count(), 1);
    assert_eq!(core.board()[0][3], 0);
    assert_eq!(core.board()[0][4], 0);
    assert_eq!(core.board()[0][5], 1);
}

#[test]
fn sand_cell_falls_down_one_step_per_material_turn() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    core.set_cell(4, 4, CELL_SAND);
    core.advance_material_turn();

    assert_eq!(core.board()[4][4], 0);
    assert_eq!(core.board()[3][4], CELL_SAND);
}

#[test]
fn sand_cell_stays_put_when_blocked_below() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    core.set_cell(4, 4, CELL_SAND);
    core.set_cell(4, 3, 2);
    core.set_cell(5, 3, 2);
    core.advance_material_turn();

    assert_eq!(core.board()[4][4], CELL_SAND);
    assert_eq!(core.board()[3][3], 0);
}

#[test]
fn sand_settling_can_trigger_line_clear_phase() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_line_clear_delay_ms(0);

    for x in 0..BOARD_WIDTH {
        if x == 4 {
            continue;
        }
        core.set_cell(x, 0, 1);
    }
    core.set_cell(4, 1, CELL_SAND);

    core.advance_material_turn();
    assert!(core.is_line_clear_active());
    assert_eq!(core.line_clear_rows(), &[0]);

    assert_eq!(core.advance_with_gravity(0), GravityAdvanceResult::Locked);
    assert_eq!(core.lines_cleared(), 1);
    assert!(!core.is_line_clear_active());
    assert!(core.current_piece().is_some());
    assert!(core.board()[0].iter().all(|&cell| cell == 0));
}

#[test]
fn piece_does_not_lock_before_threshold() {
    let mut core = grounded_o_piece_core();
    assert_eq!(core.lock_delay_ms(), LOCK_DELAY_MS_DEFAULT);

    let result = core.advance_with_gravity(LOCK_DELAY_MS_DEFAULT - 1);
    assert_eq!(result, GravityAdvanceResult::Grounded);
    assert_eq!(core.current_piece_pos(), Vec2i::new(4, 1));
    assert!(core.current_piece().is_some());
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), LOCK_DELAY_MS_DEFAULT - 1);
    assert!(core.is_grounded_for_lock_delay());
}

#[test]
fn piece_locks_after_cumulative_grounded_delay() {
    let mut core = grounded_o_piece_core();

    assert_eq!(
        core.advance_with_gravity(250),
        GravityAdvanceResult::Grounded
    );
    assert_eq!(
        core.advance_with_gravity(250),
        GravityAdvanceResult::Grounded
    );
    assert_eq!(core.advance_with_gravity(250), GravityAdvanceResult::Locked);

    assert_eq!(core.current_piece_pos(), Vec2i::new(4, BOARD_HEIGHT as i32));
    assert!(core.current_piece().is_some());
    assert_eq!(core.grounded_lock_ms(), 0);
    assert!(!core.is_grounded_for_lock_delay());
    assert_eq!(core.board()[0][4], 2);
    assert_eq!(core.board()[0][5], 2);
}

#[test]
fn successful_ground_moves_reset_delay() {
    let mut move_core = grounded_o_piece_core();
    assert_eq!(
        move_core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(move_core.move_piece(Vec2i::new(1, 0)));
    assert_eq!(move_core.grounded_lock_ms(), 0);
    assert_eq!(move_core.grounded_total_lock_ms(), 400);
    assert_eq!(
        move_core.advance_with_gravity(200),
        GravityAdvanceResult::Grounded
    );
    assert_eq!(
        move_core.advance_with_gravity(300),
        GravityAdvanceResult::Grounded
    );
    assert_eq!(
        move_core.advance_with_gravity(200),
        GravityAdvanceResult::Locked
    );
}

#[test]
fn o_piece_spin_without_relocation_does_not_reset_delay() {
    let mut core = grounded_o_piece_core();
    assert_eq!(
        core.advance_with_gravity(450),
        GravityAdvanceResult::Grounded
    );
    assert!(core.rotate_piece(RotationDir::Cw));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 450);
    assert_eq!(core.advance_with_gravity(50), GravityAdvanceResult::Grounded);
    assert_eq!(core.advance_with_gravity(450), GravityAdvanceResult::Locked);
}

#[test]
fn non_o_spin_resets_lock_delay_when_footprint_changes() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::T]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::T, Vec2i::new(4, 0), 0);

    assert_eq!(
        core.advance_with_gravity(450),
        GravityAdvanceResult::Grounded
    );
    assert!(core.rotate_piece(RotationDir::Cw));
    assert_eq!(core.grounded_lock_ms(), 0);
}

#[test]
fn total_grounded_lock_cap_prevents_infinite_resets() {
    let mut core = grounded_o_piece_core();
    assert!(
        core.lock_delay_max_ms() > core.lock_delay_ms(),
        "expected the total lock cap to be longer than the per-reset lock delay"
    );
    assert_eq!(core.lock_delay_max_ms(), LOCK_DELAY_MAX_MS_DEFAULT);

    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.move_piece(Vec2i::new(1, 0)));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 400);

    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.move_piece(Vec2i::new(-1, 0)));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 800);

    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.move_piece(Vec2i::new(1, 0)));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 1_200);

    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.move_piece(Vec2i::new(-1, 0)));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 1_600);

    // Another reset would normally extend lock forever, but total grounded cap forces lock.
    assert_eq!(core.advance_with_gravity(400), GravityAdvanceResult::Locked);
}

#[test]
fn failed_adjustments_do_not_reset_delay() {
    let mut core = grounded_o_piece_core();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(0, 1), 0);

    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(!core.move_piece(Vec2i::new(-1, 0)));
    assert_eq!(core.grounded_lock_ms(), 0);
    assert_eq!(core.grounded_total_lock_ms(), 400);
    assert_eq!(core.advance_with_gravity(100), GravityAdvanceResult::Grounded);
    assert_eq!(core.advance_with_gravity(400), GravityAdvanceResult::Locked);
}

#[test]
fn hard_drop_remains_immediate_and_clears_delay_state() {
    let mut core = grounded_o_piece_core();
    assert_eq!(
        core.advance_with_gravity(LOCK_DELAY_MS_DEFAULT - 1),
        GravityAdvanceResult::Grounded
    );

    let drop_distance = core.hard_drop();
    assert_eq!(drop_distance, 0);
    assert_eq!(core.grounded_lock_ms(), 0);
    assert!(!core.is_grounded_for_lock_delay());
    assert_eq!(
        core.advance_with_gravity(1),
        GravityAdvanceResult::Moved,
        "newly spawned piece should start fresh without stale lock-delay progress"
    );
}

#[test]
fn hold_and_spawn_paths_reset_delay_state() {
    let mut core = grounded_o_piece_core();
    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.hold_piece());
    assert_eq!(core.grounded_lock_ms(), 0);
    assert!(!core.is_grounded_for_lock_delay());

    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    assert_eq!(
        core.advance_with_gravity(400),
        GravityAdvanceResult::Grounded
    );
    assert!(core.spawn_new_piece());
    assert_eq!(core.grounded_lock_ms(), 0);
    assert!(!core.is_grounded_for_lock_delay());
}

#[test]
fn line_clear_enters_delay_phase_before_commit() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_line_clear_delay_ms(LINE_CLEAR_DELAY_MS_DEFAULT);

    for x in 0..BOARD_WIDTH {
        if x == 4 || x == 5 {
            continue;
        }
        core.set_cell(x, 0, 1);
    }
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    assert_eq!(core.hard_drop(), 0);

    assert!(core.is_line_clear_active());
    assert_eq!(core.line_clear_rows(), &[0]);
    assert_eq!(core.lines_cleared(), 0);
    assert_eq!(core.score(), 0);
    assert!(core.current_piece().is_none());
    assert!(core.board()[0].iter().all(|&cell| cell != 0));
}

#[test]
fn line_clear_commits_after_delay_and_spawns_piece() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_line_clear_delay_ms(100);

    for x in 0..BOARD_WIDTH {
        if x == 4 || x == 5 {
            continue;
        }
        core.set_cell(x, 0, 1);
    }
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    core.hard_drop();
    assert!(core.is_line_clear_active());

    assert_eq!(
        core.advance_with_gravity(99),
        GravityAdvanceResult::LineClearAnimating
    );
    assert!(core.is_line_clear_active());
    assert_eq!(core.lines_cleared(), 0);
    assert_eq!(core.score(), 0);

    assert_eq!(core.advance_with_gravity(1), GravityAdvanceResult::Locked);
    assert!(!core.is_line_clear_active());
    assert_eq!(core.lines_cleared(), 1);
    assert_eq!(core.score(), 100);
    assert!(core.current_piece().is_some());
    assert_eq!(core.board()[0][4], 2);
    assert_eq!(core.board()[0][5], 2);
    for x in 0..BOARD_WIDTH {
        if x == 4 || x == 5 {
            continue;
        }
        assert_eq!(core.board()[0][x], 0);
    }
}

#[test]
fn inputs_are_ignored_during_line_clear_delay() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O, Piece::T]);
    core.initialize_game();
    core.set_line_clear_delay_ms(120);

    for x in 0..BOARD_WIDTH {
        if x == 4 || x == 5 {
            continue;
        }
        core.set_cell(x, 0, 1);
    }
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    core.hard_drop();
    assert!(core.is_line_clear_active());

    assert!(!core.move_piece(Vec2i::new(1, 0)));
    assert!(!core.move_piece_down());
    assert!(!core.rotate_piece(RotationDir::Cw));
    assert!(!core.hold_piece());
    assert_eq!(core.hard_drop(), 0);
    assert_eq!(
        core.advance_with_gravity(60),
        GravityAdvanceResult::LineClearAnimating
    );
    assert!(core.is_line_clear_active());
}
