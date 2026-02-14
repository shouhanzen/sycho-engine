use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameView {
    MainMenu,
    Tetris { paused: bool },
    SkillTree,
    GameOver,
}

impl Default for GameView {
    fn default() -> Self {
        Self::MainMenu
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameViewEvent {
    StartGame,
    OpenSkillTree,
    OpenSkillTreeEditor,
    Back,
    TogglePause,
    GameOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameViewEffect {
    None,
    ResetTetris,
}

impl GameView {
    /// Pure transition function for the game "view" state machine.
    ///
    /// Side-effects (like actually resetting a `HeadlessRunner`) are reported via `GameViewEffect`
    /// so callers can stay deterministic + easy to test.
    pub fn handle(self, event: GameViewEvent) -> (GameView, GameViewEffect) {
        match (self, event) {
            (GameView::MainMenu, GameViewEvent::StartGame) => (
                GameView::Tetris { paused: false },
                GameViewEffect::ResetTetris,
            ),
            (GameView::MainMenu, GameViewEvent::OpenSkillTreeEditor) => {
                (GameView::SkillTree, GameViewEffect::None)
            }

            (GameView::SkillTree, GameViewEvent::Back) => {
                (GameView::MainMenu, GameViewEffect::None)
            }
            (GameView::SkillTree, GameViewEvent::StartGame) => (
                GameView::Tetris { paused: false },
                GameViewEffect::ResetTetris,
            ),

            (GameView::Tetris { paused }, GameViewEvent::TogglePause) => {
                (GameView::Tetris { paused: !paused }, GameViewEffect::None)
            }
            (GameView::Tetris { .. }, GameViewEvent::GameOver) => {
                (GameView::GameOver, GameViewEffect::None)
            }

            (GameView::GameOver, GameViewEvent::StartGame) => (
                GameView::Tetris { paused: false },
                GameViewEffect::ResetTetris,
            ),
            (GameView::GameOver, GameViewEvent::OpenSkillTree) => {
                (GameView::SkillTree, GameViewEffect::None)
            }
            (GameView::GameOver, GameViewEvent::Back) => (GameView::MainMenu, GameViewEffect::None),

            // Ignore irrelevant events in the current state.
            (state, _) => (state, GameViewEffect::None),
        }
    }

    pub fn is_tetris(self) -> bool {
        matches!(self, GameView::Tetris { .. })
    }

    pub fn is_tetris_playing(self) -> bool {
        matches!(self, GameView::Tetris { paused: false })
    }

    pub fn is_tetris_paused(self) -> bool {
        matches!(self, GameView::Tetris { paused: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_is_main_menu() {
        assert_eq!(GameView::default(), GameView::MainMenu);
    }

    #[test]
    fn start_game_from_main_menu_enters_tetris_and_requests_reset() {
        assert_eq!(
            GameView::MainMenu.handle(GameViewEvent::StartGame),
            (
                GameView::Tetris { paused: false },
                GameViewEffect::ResetTetris
            )
        );
    }

    #[test]
    fn open_skilltree_from_main_menu_is_ignored() {
        assert_eq!(
            GameView::MainMenu.handle(GameViewEvent::OpenSkillTree),
            (GameView::MainMenu, GameViewEffect::None)
        );
    }

    #[test]
    fn open_skilltree_editor_from_main_menu_enters_skilltree() {
        assert_eq!(
            GameView::MainMenu.handle(GameViewEvent::OpenSkillTreeEditor),
            (GameView::SkillTree, GameViewEffect::None)
        );
    }

    #[test]
    fn toggle_pause_in_tetris_flips_paused_flag() {
        assert_eq!(
            GameView::Tetris { paused: false }.handle(GameViewEvent::TogglePause),
            (GameView::Tetris { paused: true }, GameViewEffect::None)
        );
        assert_eq!(
            GameView::Tetris { paused: true }.handle(GameViewEvent::TogglePause),
            (GameView::Tetris { paused: false }, GameViewEffect::None)
        );
    }

    #[test]
    fn back_from_skilltree_returns_to_main_menu() {
        assert_eq!(
            GameView::SkillTree.handle(GameViewEvent::Back),
            (GameView::MainMenu, GameViewEffect::None)
        );
    }

    #[test]
    fn game_over_event_in_tetris_enters_game_over() {
        assert_eq!(
            GameView::Tetris { paused: false }.handle(GameViewEvent::GameOver),
            (GameView::GameOver, GameViewEffect::None)
        );
    }

    #[test]
    fn game_over_menu_can_restart_or_open_skilltree() {
        assert_eq!(
            GameView::GameOver.handle(GameViewEvent::StartGame),
            (
                GameView::Tetris { paused: false },
                GameViewEffect::ResetTetris
            )
        );
        assert_eq!(
            GameView::GameOver.handle(GameViewEvent::OpenSkillTree),
            (GameView::SkillTree, GameViewEffect::None)
        );
    }

    #[test]
    fn back_from_game_over_returns_to_main_menu() {
        assert_eq!(
            GameView::GameOver.handle(GameViewEvent::Back),
            (GameView::MainMenu, GameViewEffect::None)
        );
    }
}
