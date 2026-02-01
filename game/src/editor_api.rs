use engine::agent::{AgentCommand, AgentHost, AgentResponse};
use engine::editor::{
    EditorAction, EditorGrid, EditorManifest, EditorPaletteEntry, EditorSnapshot, EditorStat,
    EditorTimeline, GridOrigin,
};

use crate::playtest::{InputAction, TetrisLogic};
use crate::tetris_core::{Piece, TetrisCore};

#[derive(Debug)]
pub enum EditorApiError {
    UnknownActionId(String),
}

pub struct EditorSession {
    host: AgentHost<TetrisLogic>,
}

impl EditorSession {
    pub fn new(seed: u64) -> Self {
        let game = TetrisLogic::new(seed, Piece::all());
        Self {
            host: AgentHost::new(game),
        }
    }

    pub fn manifest(&self) -> EditorManifest {
        EditorManifest {
            title: "Tetree (Tetris)".to_string(),
            actions: vec![
                EditorAction {
                    id: "moveLeft".to_string(),
                    label: "Left".to_string(),
                },
                EditorAction {
                    id: "moveRight".to_string(),
                    label: "Right".to_string(),
                },
                EditorAction {
                    id: "softDrop".to_string(),
                    label: "Down".to_string(),
                },
                EditorAction {
                    id: "rotateCw".to_string(),
                    label: "Rotate CW".to_string(),
                },
                EditorAction {
                    id: "rotateCcw".to_string(),
                    label: "Rotate CCW".to_string(),
                },
                EditorAction {
                    id: "rotate180".to_string(),
                    label: "Rotate 180".to_string(),
                },
                EditorAction {
                    id: "hardDrop".to_string(),
                    label: "Hard Drop".to_string(),
                },
                EditorAction {
                    id: "hold".to_string(),
                    label: "Hold".to_string(),
                },
                EditorAction {
                    id: "noop".to_string(),
                    label: "Noop".to_string(),
                },
            ],
        }
    }

    pub fn timeline(&self) -> EditorTimeline {
        let runner = self.host.runner();
        let tm = runner.timemachine();
        EditorTimeline {
            frame: runner.frame(),
            history_len: runner.history().len(),
            can_rewind: tm.can_rewind(),
            can_forward: tm.can_forward(),
        }
    }

    pub fn state(&mut self) -> EditorSnapshot {
        snapshot_from_response(self.host.handle(AgentCommand::GetState))
    }

    pub fn step(&mut self, action_id: &str) -> Result<EditorSnapshot, EditorApiError> {
        let action = action_from_id(action_id)
            .ok_or_else(|| EditorApiError::UnknownActionId(action_id.to_string()))?;
        Ok(snapshot_from_response(
            self.host.handle(AgentCommand::Step(action)),
        ))
    }

    pub fn rewind(&mut self, frames: usize) -> EditorSnapshot {
        snapshot_from_response(self.host.handle(AgentCommand::Rewind { frames }))
    }

    pub fn forward(&mut self, frames: usize) -> EditorSnapshot {
        snapshot_from_response(self.host.handle(AgentCommand::Forward { frames }))
    }

    pub fn reset(&mut self) -> EditorSnapshot {
        snapshot_from_response(self.host.handle(AgentCommand::Reset))
    }
}

fn action_from_id(id: &str) -> Option<InputAction> {
    match id {
        "moveLeft" => Some(InputAction::MoveLeft),
        "moveRight" => Some(InputAction::MoveRight),
        "softDrop" => Some(InputAction::SoftDrop),
        "rotateCw" => Some(InputAction::RotateCw),
        "rotateCcw" => Some(InputAction::RotateCcw),
        "rotate180" => Some(InputAction::Rotate180),
        "hardDrop" => Some(InputAction::HardDrop),
        "hold" => Some(InputAction::Hold),
        "noop" => Some(InputAction::Noop),
        _ => None,
    }
}

fn snapshot_from_response(response: AgentResponse<TetrisCore>) -> EditorSnapshot {
    match response {
        AgentResponse::State { frame, state } => snapshot_from_state(frame, &state),
        AgentResponse::History { .. } => {
            unreachable!("history responses are not exposed via the editor API")
        }
    }
}

fn snapshot_from_state(frame: usize, state: &TetrisCore) -> EditorSnapshot {
    let pos = state.current_piece_pos();

    let stats = vec![
        stat("linesCleared", state.lines_cleared()),
        stat_opt("currentPiece", state.current_piece().map(piece_label)),
        stat_opt("nextPiece", state.next_piece().map(piece_label)),
        stat("posX", pos.x),
        stat("posY", pos.y),
        stat("rotation", state.current_piece_rotation()),
        stat_opt("heldPiece", state.held_piece().map(piece_label)),
        stat("canHold", state.can_hold()),
    ];

    let grid = EditorGrid {
        origin: GridOrigin::BottomLeft,
        cells: state.board_with_active_piece(),
        palette: Some(default_tetris_palette()),
    };

    EditorSnapshot {
        frame,
        stats,
        grid: Some(grid),
    }
}

fn stat(label: impl Into<String>, value: impl ToString) -> EditorStat {
    EditorStat {
        label: label.into(),
        value: value.to_string(),
    }
}

fn stat_opt(label: impl Into<String>, value: Option<String>) -> EditorStat {
    EditorStat {
        label: label.into(),
        value: value.unwrap_or_else(|| "-".to_string()),
    }
}

fn piece_label(piece: Piece) -> String {
    match piece {
        Piece::I => "I",
        Piece::O => "O",
        Piece::T => "T",
        Piece::S => "S",
        Piece::Z => "Z",
        Piece::J => "J",
        Piece::L => "L",
    }
    .to_string()
}

fn default_tetris_palette() -> Vec<EditorPaletteEntry> {
    // Keep the palette small and stable: 0 = background, 1..7 are the 7 tetrominoes.
    let mut entries = Vec::with_capacity(8);
    for value in 0u8..=7u8 {
        entries.push(EditorPaletteEntry {
            value,
            rgba: engine::render::color_for_cell(value),
            label: None,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_advances_frame() {
        let mut session = EditorSession::new(0);
        let initial = session.state();

        let stepped = session.step("moveLeft").expect("valid action");
        assert_eq!(stepped.frame, initial.frame + 1);
    }

    #[test]
    fn timeline_reports_history_len() {
        let mut session = EditorSession::new(0);
        assert_eq!(session.timeline().history_len, 1);

        session.step("noop").unwrap();
        session.step("noop").unwrap();

        let timeline = session.timeline();
        assert_eq!(timeline.frame, 2);
        assert_eq!(timeline.history_len, 3);
        assert!(timeline.can_rewind);
        assert!(!timeline.can_forward);
    }

    #[test]
    fn unknown_action_is_rejected() {
        let mut session = EditorSession::new(0);
        let err = session.step("doesNotExist").unwrap_err();
        match err {
            EditorApiError::UnknownActionId(id) => assert_eq!(id, "doesNotExist"),
        }
    }
}

