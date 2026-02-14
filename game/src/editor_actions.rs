use engine::editor::{EditorAction, EditorManifest};

use crate::playtest::InputAction;

const EDITOR_TITLE: &str = "Tetree (Tetris)";

const ACTION_SPECS: &[(&str, &str, InputAction)] = &[
    ("moveLeft", "Left", InputAction::MoveLeft),
    ("moveRight", "Right", InputAction::MoveRight),
    ("softDrop", "Down", InputAction::SoftDrop),
    ("rotateCw", "Rotate CW", InputAction::RotateCw),
    ("rotateCcw", "Rotate CCW", InputAction::RotateCcw),
    ("rotate180", "Rotate 180", InputAction::Rotate180),
    ("hardDrop", "Hard Drop", InputAction::HardDrop),
    ("hold", "Hold", InputAction::Hold),
    ("noop", "Noop", InputAction::Noop),
];

pub fn default_manifest() -> EditorManifest {
    EditorManifest {
        title: EDITOR_TITLE.to_string(),
        actions: ACTION_SPECS
            .iter()
            .map(|(id, label, _)| EditorAction {
                id: (*id).to_string(),
                label: (*label).to_string(),
            })
            .collect(),
    }
}

pub fn action_from_id(id: &str) -> Option<InputAction> {
    ACTION_SPECS
        .iter()
        .find_map(|(action_id, _, action)| (*action_id == id).then_some(*action))
}
