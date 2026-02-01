use engine::GameLogic;

use crate::tetris_core::{Piece, RotationDir, TetrisCore, Vec2i};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TetrisAction {
    MoveLeft,
    MoveRight,
    MoveDown,
    RotateCw,
    RotateCcw,
    HardDrop,
    Noop,
}

#[derive(Debug, Clone)]
pub struct TetrisGame {
    seed: u64,
    available_pieces: Vec<Piece>,
}

impl TetrisGame {
    pub fn new(seed: u64, available_pieces: Vec<Piece>) -> Self {
        Self {
            seed,
            available_pieces,
        }
    }

    pub fn standard(seed: u64) -> Self {
        Self::new(seed, Piece::all())
    }
}

impl GameLogic for TetrisGame {
    type State = TetrisCore;
    type Input = TetrisAction;

    fn initial_state(&self) -> Self::State {
        let mut core = TetrisCore::new(self.seed);
        core.set_available_pieces(self.available_pieces.clone());
        core.initialize_game();
        core
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        let mut next = state.clone();
        apply_action(&mut next, input);
        next
    }
}

fn apply_action(core: &mut TetrisCore, action: TetrisAction) {
    match action {
        TetrisAction::MoveLeft => {
            core.move_piece(Vec2i::new(-1, 0));
        }
        TetrisAction::MoveRight => {
            core.move_piece(Vec2i::new(1, 0));
        }
        TetrisAction::MoveDown => {
            core.move_piece_down();
        }
        TetrisAction::RotateCw => {
            core.rotate_piece(RotationDir::Cw);
        }
        TetrisAction::RotateCcw => {
            core.rotate_piece(RotationDir::Ccw);
        }
        TetrisAction::HardDrop => {
            core.hard_drop();
        }
        TetrisAction::Noop => {}
    }
}
