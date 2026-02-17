use engine::GameLogic;

use crate::state::GameState;
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
pub struct BlockGame {
    seed: u64,
    available_pieces: Vec<Piece>,
}

impl BlockGame {
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

impl GameLogic for BlockGame {
    type State = GameState;
    type Input = TetrisAction;

    fn initial_state(&self) -> Self::State {
        let mut core = TetrisCore::new(self.seed);
        core.set_available_pieces(self.available_pieces.clone());
        core.initialize_game();
        GameState::new(core)
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        let mut next = state.clone();
        apply_action(&mut next.tetris, input);
        next
    }
}

pub type BlockAction = TetrisAction;
pub type TetrisGame = BlockGame;

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
    core.advance_material_turn();
}
