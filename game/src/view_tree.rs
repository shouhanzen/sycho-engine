use engine::ui;
use engine::view_tree::{ButtonNode, RectNode, ViewNode, ViewTree};
use serde::{Deserialize, Serialize};

use crate::state::GameState;
use crate::tetris_ui::MAIN_MENU_TITLE;
use crate::tetris_ui::compute_layout;
use crate::view::GameView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameUiAction {
    StartGame,
    OpenSkillTree,
    OpenSkillTreeEditor,
    Quit,
    PauseToggle,
    HoldPiece,
    SkillTreeToolSelect,
    SkillTreeToolMove,
    SkillTreeToolAddCell,
    SkillTreeToolRemoveCell,
    SkillTreeToolConnect,
    Resume,
    EndRun,
    Restart,
}

pub fn build_menu_view_tree(view: GameView, width: u32, height: u32) -> ViewTree<GameUiAction> {
    let mut tree = ViewTree::new();
    match view {
        GameView::MainMenu => {
            if let Some((start, skilltree, quit)) = main_menu_button_rects(width, height) {
                push_button(&mut tree, 1, start, "START", GameUiAction::StartGame, true);
                push_button(
                    &mut tree,
                    2,
                    skilltree,
                    "SKILLTREE EDITOR",
                    GameUiAction::OpenSkillTreeEditor,
                    true,
                );
                push_button(&mut tree, 3, quit, "QUIT", GameUiAction::Quit, true);
            }
        }
        GameView::Tetris { paused: true } => {
            if let Some((resume, end_run)) = pause_menu_button_rects(width, height) {
                push_button(&mut tree, 10, resume, "RESUME", GameUiAction::Resume, true);
                push_button(
                    &mut tree,
                    11,
                    end_run,
                    "END RUN",
                    GameUiAction::EndRun,
                    true,
                );
            }
        }
        GameView::GameOver => {
            if let Some((restart, skilltree, quit)) = game_over_button_rects(width, height) {
                push_button(
                    &mut tree,
                    20,
                    restart,
                    "RESTART",
                    GameUiAction::Restart,
                    true,
                );
                push_button(
                    &mut tree,
                    21,
                    skilltree,
                    "SKILL TREE",
                    GameUiAction::OpenSkillTree,
                    true,
                );
                push_button(&mut tree, 22, quit, "QUIT", GameUiAction::Quit, true);
            }
        }
        _ => {}
    }
    tree
}

pub fn build_hud_view_tree(state: &GameState, width: u32, height: u32) -> ViewTree<GameUiAction> {
    let mut tree = ViewTree::new();
    if !state.view.is_tetris() {
        return tree;
    }

    let board = state.tetris.board();
    let board_h = board.len() as u32;
    let board_w = board.first().map(|r| r.len()).unwrap_or(0) as u32;
    let layout = compute_layout(
        width,
        height,
        board_w,
        board_h,
        state.tetris.next_queue().len(),
    );

    push_button(
        &mut tree,
        100,
        layout.pause_button,
        "",
        GameUiAction::PauseToggle,
        true,
    );
    push_button(
        &mut tree,
        101,
        layout.hold_panel,
        "",
        GameUiAction::HoldPiece,
        state.tetris.can_hold(),
    );
    tree.push(ViewNode::Rect(RectNode {
        rect: layout.hold_panel,
    }));
    tree.push(ViewNode::Rect(RectNode {
        rect: layout.next_panel,
    }));
    tree
}

pub fn build_skilltree_toolbar_view_tree(
    state: &GameState,
    width: u32,
    height: u32,
) -> ViewTree<GameUiAction> {
    let mut tree = ViewTree::new();
    if !matches!(state.view, GameView::SkillTree) || !state.skilltree.editor.enabled {
        return tree;
    }
    let Some(buttons) = skilltree_toolbar_button_rects(width, height) else {
        return tree;
    };
    let actions = [
        GameUiAction::SkillTreeToolSelect,
        GameUiAction::SkillTreeToolMove,
        GameUiAction::SkillTreeToolAddCell,
        GameUiAction::SkillTreeToolRemoveCell,
        GameUiAction::SkillTreeToolConnect,
    ];
    let labels = ["SELECT", "MOVE", "ADD CELL", "REMOVE CELL", "LINK"];
    for (idx, rect) in buttons.iter().enumerate() {
        push_button(
            &mut tree,
            200 + idx as u32,
            *rect,
            labels[idx],
            actions[idx],
            true,
        );
    }
    tree
}

fn push_button(
    tree: &mut ViewTree<GameUiAction>,
    id: u32,
    rect: ui::Rect,
    label: &str,
    action: GameUiAction,
    enabled: bool,
) {
    tree.push(ViewNode::Button(ButtonNode {
        id,
        rect,
        label: label.to_string(),
        action,
        enabled,
    }));
}

fn main_menu_button_rects(width: u32, height: u32) -> Option<(ui::Rect, ui::Rect, ui::Rect)> {
    let margin = 32u32;
    let pad = 18u32;

    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return None;
    }

    let title = MAIN_MENU_TITLE;
    let title_chars = title.chars().count() as u32;
    let glyph_cols = 4u32;
    let denom = title_chars.saturating_mul(glyph_cols).max(1);
    let max_scale = 12u32;
    let title_scale = (safe.w / denom).clamp(2, max_scale);
    let title_h = (5u32).saturating_mul(title_scale).min(safe.h);

    let content = safe.inset(ui::Insets::all(pad));
    let button_size = ui::Size::new(240, 44).clamp_max(content.size());
    let button_gap = 12u32;
    let title_button_gap = 28u32;
    let stack_h = title_h
        .saturating_add(title_button_gap)
        .saturating_add(button_size.h.saturating_mul(3))
        .saturating_add(button_gap.saturating_mul(2));
    let top_y = content
        .y
        .saturating_add(content.h.saturating_sub(stack_h) / 2);

    let buttons_y = top_y
        .saturating_add(title_h)
        .saturating_add(title_button_gap);
    let start_button = ui::Rect {
        x: content
            .x
            .saturating_add(content.w.saturating_sub(button_size.w) / 2),
        y: buttons_y,
        w: button_size.w,
        h: button_size.h,
    };
    let skilltree_editor_button = ui::Rect {
        x: start_button.x,
        y: start_button
            .y
            .saturating_add(start_button.h)
            .saturating_add(button_gap),
        w: start_button.w,
        h: start_button.h,
    };
    let quit_button = ui::Rect {
        x: start_button.x,
        y: skilltree_editor_button
            .y
            .saturating_add(skilltree_editor_button.h)
            .saturating_add(button_gap),
        w: start_button.w,
        h: start_button.h,
    };
    Some((start_button, skilltree_editor_button, quit_button))
}

fn pause_menu_button_rects(width: u32, height: u32) -> Option<(ui::Rect, ui::Rect)> {
    let margin = 32u32;
    let pad = 18u32;

    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return None;
    }

    let panel_size = ui::Size::new(360, 260).clamp_max(safe.size());
    if panel_size.w == 0 || panel_size.h == 0 {
        return None;
    }

    let panel_ui = safe.place(panel_size, ui::Anchor::Center);
    let content = panel_ui.inset(ui::Insets::all(pad));
    let gap = 12u32;
    let button_size = ui::Size::new(240, 44).clamp_max(content.size());
    let resume_ui = content.place(button_size, ui::Anchor::BottomCenter);
    let resume_button = ui::Rect {
        x: resume_ui.x,
        y: resume_ui.y,
        w: resume_ui.w,
        h: resume_ui.h,
    };
    let end_run_button = ui::Rect {
        x: resume_button.x,
        y: resume_button
            .y
            .saturating_sub(resume_button.h.saturating_add(gap)),
        w: resume_button.w,
        h: resume_button.h,
    };
    Some((resume_button, end_run_button))
}

fn game_over_button_rects(width: u32, height: u32) -> Option<(ui::Rect, ui::Rect, ui::Rect)> {
    let margin = 32u32;
    let pad = 18u32;

    let panel_w = 420u32.min(width.saturating_sub(margin.saturating_mul(2)));
    let panel_h = 280u32.min(height.saturating_sub(margin.saturating_mul(2)));
    if panel_w == 0 || panel_h == 0 {
        return None;
    }

    let panel = ui::Rect {
        x: width.saturating_sub(panel_w) / 2,
        y: height.saturating_sub(panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    let button_h = 44u32.min(panel.h.saturating_sub(pad.saturating_mul(2)));
    let button_w = 240u32.min(panel.w.saturating_sub(pad.saturating_mul(2)));
    let gap = 12u32;
    let buttons_total_h = button_h
        .saturating_mul(3)
        .saturating_add(gap.saturating_mul(2));
    let top_y = panel
        .y
        .saturating_add(panel.h.saturating_sub(pad.saturating_add(buttons_total_h)));

    let restart_button = ui::Rect {
        x: panel.x.saturating_add(panel.w.saturating_sub(button_w) / 2),
        y: top_y,
        w: button_w,
        h: button_h,
    };
    let skilltree_button = ui::Rect {
        x: restart_button.x,
        y: restart_button.y.saturating_add(button_h + gap),
        w: button_w,
        h: button_h,
    };
    let quit_button = ui::Rect {
        x: restart_button.x,
        y: skilltree_button.y.saturating_add(button_h + gap),
        w: button_w,
        h: button_h,
    };
    Some((restart_button, skilltree_button, quit_button))
}

fn skilltree_toolbar_button_rects(width: u32, height: u32) -> Option<[ui::Rect; 5]> {
    let margin = 0u32;
    let pad = 18u32;

    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return None;
    }
    let content = safe.inset(ui::Insets::all(pad));

    let header_h = 120u32.min(safe.h);
    let tool_gap = 8u32;
    let tool_button_h = 30u32.min(header_h.saturating_sub(pad));
    let tool_button_w = 120u32.min(content.w);
    let tool_count = 5u32;
    let total_w = tool_button_w
        .saturating_mul(tool_count)
        .saturating_add(tool_gap.saturating_mul(tool_count.saturating_sub(1)));
    let toolbar_x = safe
        .x
        .saturating_add(safe.w.saturating_sub(pad))
        .saturating_sub(total_w);
    let toolbar_y = safe.y.saturating_add(pad);

    let mut rects = [ui::Rect::default(); 5];
    for (idx, rect) in rects.iter_mut().enumerate() {
        let x = toolbar_x.saturating_add((tool_button_w + tool_gap).saturating_mul(idx as u32));
        *rect = ui::Rect {
            x,
            y: toolbar_y,
            w: tool_button_w,
            h: tool_button_h,
        };
    }
    Some(rects)
}
