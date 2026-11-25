use std::time::Duration;

use engine::app::InputFrame;
use engine::surface::SurfaceSize;
use engine::timeline::StateSchema;
use serde::{Deserialize, Serialize};
use winit::event::{MouseButton, VirtualKeyCode};

use crate::round_timer::RoundTimer;
use crate::settings::PlayerSettings;
use crate::skilltree::SkillTreeRuntime;
use crate::tetris_core::TetrisCore;
use crate::view::GameView;

pub const DEFAULT_ROUND_LIMIT: Duration = Duration::from_secs(20);
pub const DEFAULT_GRAVITY_INTERVAL: Duration = Duration::from_millis(500);
pub const TIMELINE_SCHEMA: StateSchema = StateSchema {
    game: "tetree",
    version: 1,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedInput {
    pub pointer: Option<(u32, u32)>,
    pub viewport: (u32, u32),
    pub buttons_down: Vec<MouseButton>,
    pub buttons_pressed: Vec<MouseButton>,
    pub buttons_released: Vec<MouseButton>,
    pub keys_down: Vec<VirtualKeyCode>,
    pub keys_pressed: Vec<VirtualKeyCode>,
    pub keys_released: Vec<VirtualKeyCode>,
    pub scroll: (f32, f32),
    pub focused: bool,
}

impl RecordedInput {
    pub fn from_frame(input: &InputFrame, size: SurfaceSize) -> Self {
        use std::collections::HashSet;
        fn keys(set: &HashSet<VirtualKeyCode>) -> Vec<VirtualKeyCode> {
            let mut keys: Vec<_> = set.iter().copied().collect();
            keys.sort_unstable();
            keys
        }
        fn buttons(set: &HashSet<MouseButton>) -> Vec<MouseButton> {
            let mut buttons: Vec<_> = set.iter().copied().collect();
            buttons.sort_unstable_by_key(|button| match button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                MouseButton::Other(id) => u32::from(*id) + 3,
            });
            buttons
        }

        Self {
            pointer: input.mouse_pos,
            viewport: (size.width, size.height),
            buttons_down: buttons(&input.mouse_buttons_down),
            buttons_pressed: buttons(&input.mouse_buttons_pressed),
            buttons_released: buttons(&input.mouse_buttons_released),
            keys_down: keys(&input.keys_down),
            keys_pressed: keys(&input.keys_pressed),
            keys_released: keys(&input.keys_released),
            scroll: (input.scroll_x, input.scroll_y),
            focused: input.window_focused,
        }
    }

    pub fn pointer_at(&self, size: SurfaceSize) -> Option<(u32, u32)> {
        let (x, y) = self.pointer?;
        let (width, height) = self.viewport;
        (x < width && y < height && size.width > 0 && size.height > 0).then(|| {
            (
                (u64::from(x) * u64::from(size.width) / u64::from(width)) as u32,
                (u64::from(y) * u64::from(size.height) / u64::from(height)) as u32,
            )
        })
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<RecordedInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_overlay: Option<PlayerSettings>,
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
            input: None,
            settings_overlay: None,
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

    pub fn use_timeline_progress(&mut self) {
        self.skilltree.use_timeline_progress();
        self.tetris.use_timeline_progress();
    }
}
