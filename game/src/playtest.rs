use engine::GameLogic;

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
}

impl TetrisLogic {
    pub fn new(seed: u64, available_pieces: Vec<Piece>) -> Self {
        Self {
            seed,
            available_pieces,
            gravity_enabled: false,
        }
    }

    pub fn with_gravity(mut self, enabled: bool) -> Self {
        self.gravity_enabled = enabled;
        self
    }
}

impl GameLogic for TetrisLogic {
    type State = TetrisCore;
    type Input = InputAction;

    fn initial_state(&self) -> Self::State {
        let mut core = TetrisCore::new(self.seed);
        core.set_available_pieces(self.available_pieces.clone());
        core.initialize_game();
        core
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        let mut next = state.clone();
        let mut apply_gravity = self.gravity_enabled;

        match input {
            InputAction::Noop => {}
            InputAction::MoveLeft => {
                next.move_piece(Vec2i::new(-1, 0));
            }
            InputAction::MoveRight => {
                next.move_piece(Vec2i::new(1, 0));
            }
            InputAction::SoftDrop => {
                next.move_piece_down();
            }
            InputAction::RotateCw => {
                next.rotate_piece(RotationDir::Cw);
            }
            InputAction::RotateCcw => {
                next.rotate_piece(RotationDir::Ccw);
            }
            InputAction::Rotate180 => {
                next.rotate_piece(RotationDir::Half);
            }
            InputAction::HardDrop => {
                next.hard_drop();
                apply_gravity = false;
            }
            InputAction::Hold => {
                next.hold_piece();
            }
        }

        if apply_gravity {
            next.move_piece_down();
        }

        next
    }
}
