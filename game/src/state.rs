use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::round_timer::RoundTimer;
use crate::skilltree::SkillTreeRuntime;
use crate::tetris_core::TetrisCore;
use crate::view::GameView;

pub const DEFAULT_ROUND_LIMIT: Duration = Duration::from_secs(20);
pub const DEFAULT_GRAVITY_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameState {
    pub view: GameView,
    pub tetris: TetrisCore,
    pub skilltree: SkillTreeRuntime,
    pub round_timer: RoundTimer,
    #[serde(with = "crate::serde_duration")]
    pub gravity_interval: Duration,
    #[serde(with = "crate::serde_duration")]
    pub gravity_elapsed: Duration,
}

impl GameState {
    pub fn new(tetris: TetrisCore) -> Self {
        Self::with_runtime(
            tetris,
            SkillTreeRuntime::from_defaults(),
            DEFAULT_ROUND_LIMIT,
            DEFAULT_GRAVITY_INTERVAL,
        )
    }

    pub fn with_runtime(
        tetris: TetrisCore,
        skilltree: SkillTreeRuntime,
        round_limit: Duration,
        gravity_interval: Duration,
    ) -> Self {
        Self {
            view: GameView::default(),
            tetris,
            skilltree,
            round_timer: RoundTimer::new(round_limit),
            gravity_interval,
            gravity_elapsed: Duration::ZERO,
        }
    }

    pub fn tetris(&self) -> &TetrisCore {
        &self.tetris
    }

    pub fn tetris_mut(&mut self) -> &mut TetrisCore {
        &mut self.tetris
    }

    pub fn core(&self) -> &TetrisCore {
        &self.tetris
    }

    pub fn core_mut(&mut self) -> &mut TetrisCore {
        &mut self.tetris
    }
}
