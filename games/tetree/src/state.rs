use std::time::Duration;

use engine::embedded::DEFAULT_PREVIEW_SIZE;
use engine::input::InputFrame;
use engine::surface::SurfaceSize;
use engine::timeline::StateSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GameDisplay {
    /// The coordinates used to lay out this frame, including its recorded pointer.
    pub viewport: SurfaceSize,
    pub dig_camera: DigCameraController,
}

impl Default for GameDisplay {
    fn default() -> Self {
        Self {
            viewport: DEFAULT_PREVIEW_SIZE,
            dig_camera: DigCameraController::new_with_config(true, DigCameraConfig::default()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SettingsState {
    #[serde(rename = "settings")]
    pub values: PlayerSettings,
    #[serde(rename = "settings_open")]
    pub open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "SavedGameState")]
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
    pub input: Option<InputFrame>,
    #[serde(flatten)]
    pub settings: SettingsState,
}

#[derive(Deserialize)]
struct SavedGameState {
    view: GameView,
    tetris: TetrisCore,
    skilltree: SkillTreeRuntime,
    round_timer: RoundTimer,
    round_timer_disabled: bool,
    display: SavedDisplay,
    #[serde(with = "crate::serde_duration")]
    gravity_interval: Duration,
    #[serde(with = "crate::serde_duration")]
    gravity_elapsed: Duration,
    input: Option<InputFrame>,
    settings: Option<PlayerSettings>,
    settings_open: Option<bool>,
    settings_overlay: Option<PlayerSettings>,
}

#[derive(Deserialize)]
struct SavedDisplay {
    viewport: SurfaceSize,
    dig_camera: DigCameraController,
    screen_shake_percent: Option<u8>,
    show_round_timer: Option<bool>,
}

impl From<SavedGameState> for GameState {
    fn from(saved: SavedGameState) -> Self {
        let open = saved
            .settings_open
            .unwrap_or(saved.settings_overlay.is_some());
        // Older snapshots without an open Settings menu only saved the applied display preferences.
        let values = saved
            .settings
            .or(saved.settings_overlay)
            .unwrap_or_else(|| {
                let mut defaults = PlayerSettings::default();
                if let Some(percent) = saved.display.screen_shake_percent {
                    defaults.video.screen_shake_percent = percent;
                }
                if let Some(show) = saved.display.show_round_timer {
                    defaults.gameplay.show_round_timer = show;
                }
                defaults
            });
        Self {
            view: saved.view,
            tetris: saved.tetris,
            skilltree: saved.skilltree,
            round_timer: saved.round_timer,
            round_timer_disabled: saved.round_timer_disabled,
            display: GameDisplay {
                viewport: saved.display.viewport,
                dig_camera: saved.display.dig_camera,
            },
            gravity_interval: saved.gravity_interval,
            gravity_elapsed: saved.gravity_elapsed,
            input: saved.input,
            settings: SettingsState {
                values: values.sanitized(),
                open,
            },
        }
    }
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

    pub fn world_offset_y_px(&self) -> i32 {
        let settings = &self.settings.values;
        if settings.accessibility.reduce_motion {
            return 0;
        }
        let shake = f32::from(settings.video.clamped_screen_shake()) / 100.0;
        (self.display.dig_camera.offset_y_px() * shake).round() as i32
    }

    pub fn pointer(&self) -> Option<(u32, u32)> {
        self.input
            .as_ref()?
            .mouse_pos
            .filter(|&(x, y)| x < self.display.viewport.width && y < self.display.viewport.height)
    }

    pub fn tetris(&self) -> &TetrisCore {
        &self.tetris
    }
}
