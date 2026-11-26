use engine::surface::SurfaceSize;

use crate::skilltree::SkillTreeEditorTool;
use crate::state::GameState;
use crate::tetris_ui::{
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SettingsMenuLayout, SkillTreeLayout,
    UiLayout, skilltree_layout, tetris_layout,
};
use crate::view::GameView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameUiAction {
    StartGame,
    OpenSkillTree,
    OpenSkillTreeEditor,
    OpenSettings,
    CloseSettings,
    ResetSettings,
    ToggleMute,
    ToggleMusic,
    ToggleTimer,
    ToggleAutoPause,
    ToggleReduceMotion,
    Quit,
    Back,
    PauseToggle,
    HoldPiece,
    SkillTreeTool(SkillTreeEditorTool),
    EndRun,
}

pub(crate) enum Menu {
    Main(MainMenuLayout),
    Pause(PauseMenuLayout),
    GameOver(GameOverMenuLayout),
}

pub struct GameUiLayout {
    pub tetris: Option<UiLayout>,
    pub skilltree: SkillTreeLayout,
    pub settings: Option<SettingsMenuLayout>,
    pub(crate) menu: Option<Menu>,
}

impl GameUiLayout {
    pub fn compute_skilltree(state: &GameState, size: SurfaceSize) -> SkillTreeLayout {
        if state.view == GameView::SkillTree {
            skilltree_layout(size.width, size.height, Some(&state.skilltree))
        } else {
            SkillTreeLayout::default()
        }
    }

    pub fn compute(state: &GameState, size: SurfaceSize) -> Self {
        let (width, height) = (size.width, size.height);
        let menu = match state.view {
            GameView::MainMenu => Some(Menu::Main(MainMenuLayout::compute(width, height))),
            GameView::Tetris { paused: true } => {
                Some(Menu::Pause(PauseMenuLayout::compute(width, height)))
            }
            GameView::GameOver => Some(Menu::GameOver(GameOverMenuLayout::compute(width, height))),
            _ => None,
        };
        Self {
            tetris: matches!(state.view, GameView::Tetris { .. } | GameView::GameOver)
                .then(|| tetris_layout(width, height, state.tetris())),
            skilltree: Self::compute_skilltree(state, size),
            settings: state
                .settings
                .open
                .then(|| SettingsMenuLayout::compute(width, height)),
            menu,
        }
    }

    pub fn action_at(&self, state: &GameState, (x, y): (u32, u32)) -> Option<GameUiAction> {
        use GameUiAction::*;
        let hit = |buttons: &[(Rect, GameUiAction)]| {
            buttons
                .iter()
                .find(|(rect, _)| rect.contains(x, y))
                .map(|&(_, action)| action)
        };

        if let Some(layout) = self.settings {
            return hit(&[
                (layout.back_button, CloseSettings),
                (layout.reset_button, ResetSettings),
                (layout.mute_toggle, ToggleMute),
                (layout.music_toggle, ToggleMusic),
                (layout.show_timer_toggle, ToggleTimer),
                (layout.auto_pause_toggle, ToggleAutoPause),
                (layout.reduce_motion_toggle, ToggleReduceMotion),
            ]);
        }
        let menu_action = match self.menu {
            Some(Menu::Main(layout)) => hit(&[
                (layout.start_button, StartGame),
                (layout.skilltree_editor_button, OpenSkillTreeEditor),
                (layout.settings_button, OpenSettings),
                (layout.quit_button, Quit),
            ]),
            Some(Menu::Pause(layout)) => hit(&[
                (layout.resume_button, PauseToggle),
                (layout.settings_button, OpenSettings),
                (layout.end_run_button, EndRun),
            ]),
            Some(Menu::GameOver(layout)) => hit(&[
                (layout.restart_button, StartGame),
                (layout.skilltree_button, OpenSkillTree),
                (layout.quit_button, Quit),
            ]),
            None => None,
        };
        if menu_action.is_some() {
            return menu_action;
        }
        match state.view {
            GameView::SkillTree if state.skilltree.editor.enabled => self
                .skilltree
                .toolbar()
                .into_iter()
                .find(|(_, _, rect)| rect.contains(x, y))
                .map(|(tool, _, _)| SkillTreeTool(tool)),
            GameView::SkillTree => hit(&[(self.skilltree.start_new_game_button, StartGame)]),
            GameView::Tetris { paused } => {
                let layout = self.tetris?;
                if layout.pause_button.contains(x, y) {
                    Some(PauseToggle)
                } else if !paused && state.tetris.can_hold() && layout.hold_panel.contains(x, y) {
                    Some(HoldPiece)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
