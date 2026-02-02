use std::collections::HashSet;

use game::tetris_core::{Piece, TetrisCore, Vec2i, BOARD_HEIGHT, BOARD_WIDTH, NEXT_QUEUE_LEN};

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
fn bag_draws_unique_pieces_per_cycle() {
    let mut core = TetrisCore::new(42);
    core.set_available_pieces(Piece::all());

    let mut first_bag = HashSet::new();
    for _ in 0..7 {
        first_bag.insert(core.draw_piece());
    }
    assert_eq!(first_bag.len(), 7);

    let mut second_bag = HashSet::new();
    for _ in 0..7 {
        second_bag.insert(core.draw_piece());
    }
    assert_eq!(second_bag.len(), 7);
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
