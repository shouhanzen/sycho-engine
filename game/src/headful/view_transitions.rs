use crate::state::GameState;
use crate::view::{GameView, GameViewEffect, GameViewEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionResult {
    pub next_view: GameView,
    pub reset_tetris: bool,
}

pub fn apply_view_event(view: GameView, event: GameViewEvent) -> TransitionResult {
    let (next_view, effect) = view.handle(event);
    TransitionResult {
        next_view,
        reset_tetris: matches!(effect, GameViewEffect::ResetTetris),
    }
}

pub fn start_game(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::StartGame)
}

pub fn open_skilltree(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::OpenSkillTree)
}

pub fn open_skilltree_editor(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::OpenSkillTreeEditor)
}

pub fn toggle_pause(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::TogglePause)
}

pub fn back(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::Back)
}

pub fn game_over(view: GameView) -> TransitionResult {
    apply_view_event(view, GameViewEvent::GameOver)
}

pub fn money_earned_from_run(state: &GameState) -> u32 {
    // Simple, deterministic conversion from in-run performance to meta-currency.
    // Tunable later; for now it makes the buy-loop visible quickly.
    let score = state.tetris.score();
    let lines = state.tetris.lines_cleared();
    score / 10 + lines.saturating_mul(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_game_requests_reset_from_menu() {
        let result = start_game(GameView::MainMenu);
        assert!(matches!(
            result.next_view,
            GameView::Tetris { paused: false }
        ));
        assert!(result.reset_tetris);
    }

    #[test]
    fn back_from_game_over_returns_main_menu_without_reset() {
        let result = back(GameView::GameOver);
        assert!(matches!(result.next_view, GameView::MainMenu));
        assert!(!result.reset_tetris);
    }
}
