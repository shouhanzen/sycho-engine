use game::tetris_core::{Piece, TetrisCore, Vec2i};

#[test]
fn board_with_active_piece_overlays_current_piece() {
    let mut core = TetrisCore::new(7);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();
    core.set_current_piece_for_test(Piece::O, Vec2i::new(4, 5), 0);

    let overlay = core.board_with_active_piece();

    assert_eq!(overlay[5][4], 2);
    assert_eq!(overlay[5][5], 2);
    assert_eq!(overlay[4][4], 2);
    assert_eq!(overlay[4][5], 2);

    assert_eq!(core.board()[5][4], 0);
    assert_eq!(core.board()[5][5], 0);
    assert_eq!(core.board()[4][4], 0);
    assert_eq!(core.board()[4][5], 0);
}
