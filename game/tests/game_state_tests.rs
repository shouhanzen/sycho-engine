use std::time::Duration;

use game::state::GameState;
use game::tetris_core::{Piece, TetrisCore};
use game::view::GameView;

#[test]
fn game_state_round_trip_preserves_state() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    let mut state = GameState::new(core);
    state.view = GameView::SkillTree;
    state
        .round_timer
        .tick_if_running(Duration::from_secs(3), true);
    state.gravity_interval = Duration::from_millis(400);
    state.gravity_elapsed = Duration::from_millis(120);
    state.skilltree.progress.money = 25;

    let json = serde_json::to_string(&state).expect("serialize game state");
    let restored: GameState = serde_json::from_str(&json).expect("deserialize game state");

    assert_eq!(restored.view, state.view);
    assert_eq!(restored.tetris.snapshot(), state.tetris.snapshot());
    assert_eq!(restored.round_timer, state.round_timer);
    assert_eq!(restored.gravity_interval, state.gravity_interval);
    assert_eq!(restored.gravity_elapsed, state.gravity_elapsed);
    assert_eq!(
        restored.skilltree.to_snapshot(),
        state.skilltree.to_snapshot()
    );
    assert!(restored.skilltree.is_unlocked("start"));
}
