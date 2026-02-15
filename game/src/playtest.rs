use engine::GameLogic;

use crate::state::GameState;
use crate::tetris_core::{Piece, RotationDir, TetrisCore, Vec2i};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Noop,
    MoveLeft,
    MoveRight,
    SoftDrop,
    RotateCw,
    RotateCcw,
    Rotate180,
    HardDrop,
    Hold,
}

#[derive(Debug, Clone)]
pub struct TetrisLogic {
    seed: u64,
    available_pieces: Vec<Piece>,
    gravity_enabled: bool,
    score_bonus_per_line: u32,
    bottomwell_enabled: bool,
}

impl TetrisLogic {
    pub fn new(seed: u64, available_pieces: Vec<Piece>) -> Self {
        Self {
            seed,
            available_pieces,
            gravity_enabled: false,
            score_bonus_per_line: 0,
            bottomwell_enabled: false,
        }
    }

    pub fn with_gravity(mut self, enabled: bool) -> Self {
        self.gravity_enabled = enabled;
        self
    }

    pub fn with_score_bonus_per_line(mut self, bonus: u32) -> Self {
        self.score_bonus_per_line = bonus;
        self
    }

    pub fn with_bottomwell(mut self, enabled: bool) -> Self {
        self.bottomwell_enabled = enabled;
        self
    }
}

impl GameLogic for TetrisLogic {
    type State = GameState;
    type Input = InputAction;

    fn initial_state(&self) -> Self::State {
        let mut core = TetrisCore::new(self.seed);
        core.set_available_pieces(self.available_pieces.clone());
        core.set_bottomwell_enabled(self.bottomwell_enabled);
        core.initialize_game();
        GameState::new(core)
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        let mut next = state.clone();
        let prev_lines = state.tetris.lines_cleared();
        let mut apply_gravity = self.gravity_enabled;

        match input {
            InputAction::Noop => {}
            InputAction::MoveLeft => {
                next.tetris.move_piece(Vec2i::new(-1, 0));
            }
            InputAction::MoveRight => {
                next.tetris.move_piece(Vec2i::new(1, 0));
            }
            InputAction::SoftDrop => {
                next.tetris.move_piece_down();
            }
            InputAction::RotateCw => {
                next.tetris.rotate_piece(RotationDir::Cw);
            }
            InputAction::RotateCcw => {
                next.tetris.rotate_piece(RotationDir::Ccw);
            }
            InputAction::Rotate180 => {
                next.tetris.rotate_piece(RotationDir::Half);
            }
            InputAction::HardDrop => {
                next.tetris.hard_drop();
                apply_gravity = false;
            }
            InputAction::Hold => {
                next.tetris.hold_piece();
            }
        }

        if apply_gravity {
            next.tetris.move_piece_down();
        }

        let delta_lines = next.tetris.lines_cleared().saturating_sub(prev_lines);
        if delta_lines > 0 && self.score_bonus_per_line > 0 {
            let bonus = self.score_bonus_per_line.saturating_mul(delta_lines);
            next.tetris.add_score(bonus);
        }

        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tetris_core::{BOARD_HEIGHT, BOARD_WIDTH};

    #[test]
    fn score_bonus_is_added_per_line_cleared() {
        // Fill the bottom row except for two cells. Place an O piece at y=1 so hard drop distance is 0,
        // completing the row and clearing exactly 1 line.
        let mut core = TetrisCore::new(0);
        core.set_available_pieces(vec![Piece::O]);
        core.initialize_game();
        let mut state = GameState::new(core);

        for x in 0..BOARD_WIDTH {
            if x == 4 || x == 5 {
                continue;
            }
            state.tetris.set_cell(x, 0, 1);
        }
        // Sanity: row is not yet full.
        assert_eq!(
            state.tetris.snapshot().board[0]
                .iter()
                .filter(|&&c| c != 0)
                .count(),
            BOARD_WIDTH - 2
        );

        state
            .tetris
            .set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);

        let logic_no_bonus = TetrisLogic::new(0, vec![Piece::O]);
        let next_no_bonus = logic_no_bonus.step(&state, InputAction::HardDrop);
        assert_eq!(next_no_bonus.tetris.lines_cleared(), 1);
        assert_eq!(
            next_no_bonus.tetris.score(),
            100,
            "expected base 1-line clear score"
        );

        let logic_bonus = TetrisLogic::new(0, vec![Piece::O]).with_score_bonus_per_line(100);
        let next_bonus = logic_bonus.step(&state, InputAction::HardDrop);
        assert_eq!(next_bonus.tetris.lines_cleared(), 1);
        assert_eq!(
            next_bonus.tetris.score(),
            200,
            "expected +100 score bonus applied for the cleared line"
        );

        // Ensure the cleared line actually moved the board (bottom row is no longer all filled).
        let bottom_filled = next_bonus
            .tetris
            .snapshot()
            .board
            .get(0)
            .map(|r| r.iter().all(|&c| c != 0))
            .unwrap_or(false);
        assert!(!bottom_filled);

        // This isn't strictly required, but catching regressions where the piece didn't lock.
        assert!(next_bonus.tetris.current_piece().is_some());
        assert_eq!(
            next_bonus.tetris.current_piece_pos(),
            Vec2i::new(4, BOARD_HEIGHT as i32)
        );
    }
}
