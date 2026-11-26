use std::time::Duration;

use engine::GameLogic;

use crate::state::GameState;
use crate::tetris_core::{BottomwellRunMods, DepthWallDef, Piece, RotationDir, TetrisCore, Vec2i};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Noop,
    MoveLeft,
    MoveRight,
    SoftDrop,
    GravityTick { dt_ms: u32 },
    RotateCw,
    RotateCcw,
    Rotate180,
    HardDrop,
    Hold,
}

fn duration_to_ms_u32(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
}

#[derive(Debug, Clone)]
pub struct TetrisLogic {
    seed: u64,
    available_pieces: Vec<Piece>,
    gravity_enabled: bool,
    score_bonus_per_line: u32,
    bottomwell_enabled: bool,
    bottomwell_run_mods: BottomwellRunMods,
    depth_wall_defs_override: Option<Vec<DepthWallDef>>,
    depth_wall_damage_tuning: Option<(u32, u32)>,
}

impl TetrisLogic {
    pub fn new(seed: u64, available_pieces: Vec<Piece>) -> Self {
        Self {
            seed,
            available_pieces,
            gravity_enabled: false,
            score_bonus_per_line: 0,
            bottomwell_enabled: false,
            bottomwell_run_mods: BottomwellRunMods::default(),
            depth_wall_defs_override: None,
            depth_wall_damage_tuning: None,
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

    pub fn with_bottomwell_run_mods(mut self, mods: BottomwellRunMods) -> Self {
        self.bottomwell_run_mods = mods;
        self
    }

    pub fn with_depth_wall_defs(mut self, defs: Vec<DepthWallDef>) -> Self {
        self.depth_wall_defs_override = Some(defs);
        self
    }

    pub fn with_depth_wall_damage_tuning(
        mut self,
        per_line_damage: u32,
        multi_bonus_percent: u32,
    ) -> Self {
        self.depth_wall_damage_tuning = Some((per_line_damage, multi_bonus_percent));
        self
    }

    pub fn initial_core(&self) -> TetrisCore {
        let mut core = TetrisCore::new(self.seed);
        core.set_available_pieces(self.available_pieces.clone());
        core.set_bottomwell_enabled(self.bottomwell_enabled);
        core.set_bottomwell_run_mods(self.bottomwell_run_mods);
        if let Some(defs) = self.depth_wall_defs_override.as_ref() {
            core.set_depth_wall_defs(defs.clone());
        }
        if let Some((per_line_damage, multi_bonus_percent)) = self.depth_wall_damage_tuning {
            core.set_depth_wall_damage_tuning(per_line_damage, multi_bonus_percent);
        }
        core.initialize_game();
        core
    }
}

impl GameLogic for TetrisLogic {
    type State = GameState;
    type Input = InputAction;

    fn initial_state(&self) -> Self::State {
        GameState::new(self.initial_core())
    }

    fn step(&self, state: &mut Self::State, input: Self::Input) {
        let prev_lines = state.tetris.lines_cleared();
        let mut apply_gravity = self.gravity_enabled;

        match input {
            InputAction::Noop => {}
            InputAction::MoveLeft => {
                state.tetris.move_piece(Vec2i::new(-1, 0));
            }
            InputAction::MoveRight => {
                state.tetris.move_piece(Vec2i::new(1, 0));
            }
            InputAction::SoftDrop => {
                state.tetris.move_piece_down();
                apply_gravity = false;
            }
            InputAction::GravityTick { dt_ms } => {
                state.tetris.advance_with_gravity(dt_ms);
                apply_gravity = false;
            }
            InputAction::RotateCw => {
                state.tetris.rotate_piece(RotationDir::Cw);
            }
            InputAction::RotateCcw => {
                state.tetris.rotate_piece(RotationDir::Ccw);
            }
            InputAction::Rotate180 => {
                state.tetris.rotate_piece(RotationDir::Half);
            }
            InputAction::HardDrop => {
                state.tetris.hard_drop();
                apply_gravity = false;
            }
            InputAction::Hold => {
                state.tetris.hold_piece();
            }
        }

        if apply_gravity {
            state
                .tetris
                .advance_with_gravity(duration_to_ms_u32(state.gravity_interval));
        }

        state.tetris.advance_material_turn();

        let delta_lines = state.tetris.lines_cleared().saturating_sub(prev_lines);
        if delta_lines > 0 && self.score_bonus_per_line > 0 {
            let bonus = self.score_bonus_per_line.saturating_mul(delta_lines);
            state.tetris.add_score(bonus);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tetris_core::{BOARD_HEIGHT, BOARD_WIDTH};

    #[test]
    fn spawns_the_configured_piece_and_moves_it_left() {
        let logic = TetrisLogic::new(0, vec![Piece::O]);
        let mut state = logic.initial_state();
        assert_eq!(state.tetris.current_piece(), Some(Piece::O));
        assert_eq!(state.tetris.next_piece(), Some(Piece::O));
        assert_eq!(
            state.tetris.current_piece_pos(),
            Vec2i::new(4, BOARD_HEIGHT as i32)
        );

        logic.step(&mut state, InputAction::MoveLeft);
        assert_eq!(
            state.tetris.current_piece_pos(),
            Vec2i::new(3, BOARD_HEIGHT as i32)
        );
    }

    #[test]
    fn square_piece_keeps_its_orientation_when_rotated() {
        let logic = TetrisLogic::new(0, vec![Piece::O]);
        let mut state = logic.initial_state();

        logic.step(&mut state, InputAction::RotateCw);
        assert_eq!(state.tetris.current_piece_rotation(), 0);
    }

    #[test]
    fn score_bonus_is_added_per_line_cleared() {
        // Fill the bottom row except for two cells. Place an O piece at y=1 so hard drop distance is 0,
        // completing the row and clearing exactly 1 line.
        let mut core = TetrisCore::new(0);
        core.set_available_pieces(vec![Piece::O]);
        core.initialize_game();
        let mut state = GameState::new(core);
        state.tetris.set_line_clear_delay_ms(0);

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
        // This test verifies score math, not clear-animation timing.
        state.tetris.set_line_clear_delay_ms(0);

        let logic_no_bonus = TetrisLogic::new(0, vec![Piece::O]);
        let mut next_no_bonus = state.clone();
        logic_no_bonus.step(&mut next_no_bonus, InputAction::HardDrop);
        logic_no_bonus.step(&mut next_no_bonus, InputAction::GravityTick { dt_ms: 0 });
        assert_eq!(next_no_bonus.tetris.lines_cleared(), 1);
        assert_eq!(
            next_no_bonus.tetris.score(),
            100,
            "expected base 1-line clear score"
        );

        let logic_bonus = TetrisLogic::new(0, vec![Piece::O]).with_score_bonus_per_line(100);
        let mut next_bonus = state;
        logic_bonus.step(&mut next_bonus, InputAction::HardDrop);
        logic_bonus.step(&mut next_bonus, InputAction::GravityTick { dt_ms: 0 });
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

    #[test]
    fn soft_drop_with_gravity_enabled_moves_exactly_one_row() {
        let logic = TetrisLogic::new(0, vec![Piece::O]).with_gravity(true);
        let mut state = logic.initial_state();
        state
            .tetris
            .set_current_piece_for_test(Piece::O, Vec2i::new(4, 10), 0);

        logic.step(&mut state, InputAction::SoftDrop);
        assert_eq!(
            state.tetris.current_piece_pos(),
            Vec2i::new(4, 9),
            "soft drop should move by one row, not apply an additional gravity move in the same step"
        );
    }

    #[test]
    fn soft_drop_ground_contact_does_not_lock_in_same_step() {
        let logic = TetrisLogic::new(0, vec![Piece::O]).with_gravity(true);
        let mut state = logic.initial_state();
        state
            .tetris
            .set_current_piece_for_test(Piece::O, Vec2i::new(4, 2), 0);

        logic.step(&mut state, InputAction::SoftDrop);
        assert_eq!(
            state.tetris.current_piece_pos(),
            Vec2i::new(4, 1),
            "soft drop should land the piece without locking in the same step"
        );
        assert!(
            state.tetris.current_piece().is_some(),
            "active piece should still be present right after soft-drop ground contact"
        );
        assert_eq!(
            state.tetris.board()[0][4],
            0,
            "piece should not have been locked into the board on the soft-drop contact step"
        );
    }

    #[test]
    fn grounded_horizontal_move_can_stall_without_locking_same_step() {
        let logic = TetrisLogic::new(0, vec![Piece::O]).with_gravity(true);
        let mut state = logic.initial_state();
        state
            .tetris
            .set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);

        logic.step(&mut state, InputAction::MoveRight);
        assert_eq!(
            state.tetris.current_piece_pos(),
            Vec2i::new(5, 1),
            "grounded horizontal adjustment should move the piece for lock-delay stalling"
        );
        assert!(
            state.tetris.current_piece().is_some(),
            "move-right stall should keep the active piece alive after the same-step gravity phase"
        );
        assert_eq!(
            state.tetris.board()[0][5],
            0,
            "piece should not lock into the board on the same step as a valid grounded horizontal move"
        );
    }
}
