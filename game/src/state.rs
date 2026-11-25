use std::time::Duration;

use engine::app::InputFrame;
use engine::embedded::DEFAULT_PREVIEW_SIZE;
use engine::surface::SurfaceSize;
use engine::timeline::StateSchema;
use serde::{Deserialize, Serialize};
use winit::event::{MouseButton, VirtualKeyCode};

use crate::headful::dig_camera::{DigCameraConfig, DigCameraController};
use crate::round_timer::RoundTimer;
use crate::settings::PlayerSettings;
use crate::skilltree::SkillTreeRuntime;
use crate::tetris_core::TetrisCore;
use crate::view::GameView;

pub const DEFAULT_ROUND_LIMIT: Duration = Duration::from_secs(20);
pub const DEFAULT_GRAVITY_INTERVAL: Duration = Duration::from_millis(500);
pub const TIMELINE_SCHEMA: StateSchema = StateSchema {
    game: "tetree",
    version: 2,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordedInput {
    pub pointer: Option<(u32, u32)>,
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
    pub fn from_frame(input: &InputFrame) -> Self {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GameDisplay {
    pub viewport: SurfaceSize,
    pub dig_camera: DigCameraController,
    pub screen_shake_percent: u8,
    pub show_round_timer: bool,
}

impl Default for GameDisplay {
    fn default() -> Self {
        Self {
            viewport: DEFAULT_PREVIEW_SIZE,
            dig_camera: DigCameraController::new_with_config(true, DigCameraConfig::default()),
            screen_shake_percent: 100,
            show_round_timer: true,
        }
    }
}

impl GameDisplay {
    pub fn world_offset_y_px(self) -> i32 {
        (self.dig_camera.offset_y_px() * (f32::from(self.screen_shake_percent) / 100.0)).round()
            as i32
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "SavedSettings")]
pub struct SettingsState {
    #[serde(rename = "settings")]
    pub values: PlayerSettings,
    #[serde(rename = "settings_open")]
    pub open: bool,
    #[serde(skip)]
    legacy_display_only: bool,
}

#[derive(Deserialize)]
struct SavedSettings {
    settings: Option<PlayerSettings>,
    settings_open: Option<bool>,
    settings_overlay: Option<PlayerSettings>,
}

impl From<SavedSettings> for SettingsState {
    fn from(saved: SavedSettings) -> Self {
        let open = saved
            .settings_open
            .unwrap_or(saved.settings_overlay.is_some());
        let values = saved.settings.or(saved.settings_overlay);
        Self {
            open,
            legacy_display_only: values.is_none(),
            values: values.unwrap_or_default().sanitized(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameState {
    pub view: GameView,
    pub tetris: TetrisCore,
    pub skilltree: SkillTreeRuntime,
    pub round_timer: RoundTimer,
    pub round_timer_disabled: bool,
    pub display: GameDisplay,
    #[serde(with = "crate::serde_duration")]
    pub gravity_interval: Duration,
    #[serde(with = "crate::serde_duration")]
    pub gravity_elapsed: Duration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<RecordedInput>,
    #[serde(flatten)]
    pub settings: SettingsState,
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
            round_timer_disabled: false,
            display: GameDisplay::default(),
            gravity_interval,
            gravity_elapsed: Duration::ZERO,
            input: None,
            settings: SettingsState::default(),
        }
    }

    pub fn restore_legacy_display_preferences(&mut self) {
        if std::mem::take(&mut self.settings.legacy_display_only) {
            self.settings.values.video.screen_shake_percent = self.display.screen_shake_percent;
            self.settings.values.gameplay.show_round_timer = self.display.show_round_timer;
        }
    }

    pub fn sync_display_preferences(&mut self) {
        let settings = &self.settings.values;
        self.display.screen_shake_percent = if settings.accessibility.reduce_motion {
            0
        } else {
            settings.video.clamped_screen_shake()
        };
        self.display.show_round_timer = settings.gameplay.show_round_timer;
    }

    pub fn pointer(&self) -> Option<(u32, u32)> {
        self.input
            .as_ref()?
            .pointer
            .filter(|&(x, y)| x < self.display.viewport.width && y < self.display.viewport.height)
    }

    pub fn tetris(&self) -> &TetrisCore {
        &self.tetris
    }
}
