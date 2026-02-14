use engine::GameLogic;
use game::agent::{TetrisAction, TetrisGame};
use game::tetris_core::{BOARD_HEIGHT, Piece, Vec2i};

#[test]
fn tetris_game_initializes_with_piece() {
    let game = TetrisGame::new(0, vec![Piece::O]);
    let state = game.initial_state();

    assert_eq!(state.tetris.current_piece(), Some(Piece::O));
    assert_eq!(state.tetris.next_piece(), Some(Piece::O));
    assert_eq!(
        state.tetris.current_piece_pos(),
        Vec2i::new(4, BOARD_HEIGHT as i32)
    );
}

#[test]
fn tetris_game_move_left_updates_position() {
    let game = TetrisGame::new(0, vec![Piece::O]);
    let state = game.initial_state();

    let moved = game.step(&state, TetrisAction::MoveLeft);
    assert_eq!(
        moved.tetris.current_piece_pos().x,
        state.tetris.current_piece_pos().x - 1
    );
}

#[test]
fn tetris_game_rotate_updates_rotation() {
    let game = TetrisGame::new(0, vec![Piece::O]);
    let state = game.initial_state();

    let rotated = game.step(&state, TetrisAction::RotateCw);
    assert_eq!(rotated.tetris.current_piece_rotation(), 1);
}
