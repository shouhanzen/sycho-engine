use engine::graphics::Renderer2d;
use engine::render::color_for_cell;
use engine::ui as ui;
use engine::ui_tree::UiTree;

use crate::ui_ids::*;

use super::{
    blend_rect, draw_button, draw_rect_outline, draw_text, draw_text_scaled, fill_rect, Rect,
    COLOR_PAUSE_MENU_BG, COLOR_PAUSE_MENU_BORDER, COLOR_PAUSE_MENU_DIM, COLOR_PAUSE_MENU_TEXT,
    MAIN_MENU_TITLE, PAUSE_MENU_DIM_ALPHA,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuLayout {
    pub panel: Rect,
    pub resume_button: Rect,
    pub end_run_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MainMenuLayout {
    pub panel: Rect,
    pub start_button: Rect,
    pub skilltree_editor_button: Rect,
    pub quit_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameOverMenuLayout {
    pub panel: Rect,
    pub restart_button: Rect,
    pub skilltree_button: Rect,
    pub quit_button: Rect,
}

pub struct PauseMenuView;

impl PauseMenuView {
    pub fn render(frame: &mut dyn Renderer2d, width: u32, height: u32) -> PauseMenuLayout {
        let mut ui_tree = UiTree::new();
        ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
        ui_tree.add_root(UI_CANVAS);
        Self::render_with_ui(frame, width, height, &mut ui_tree)
    }

    pub fn render_with_ui(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        ui_tree: &mut UiTree,
    ) -> PauseMenuLayout {
        // Dim the entire game view.
        blend_rect(
            frame,
            width,
            height,
            0,
            0,
            width,
            height,
            COLOR_PAUSE_MENU_DIM,
            PAUSE_MENU_DIM_ALPHA,
        );

        let margin = 32u32;
        let pad = 18u32;

        // Layout is expressed via the engine UI layout helpers, then converted to our local `Rect`
        // for hit-testing and drawing.
        let screen = ui::Rect::from_size(width, height);
        let safe = screen.inset(ui::Insets::all(margin));
        if safe.w == 0 || safe.h == 0 {
            return PauseMenuLayout::default();
        }

        let panel_size = ui::Size::new(360, 260).clamp_max(safe.size());
        if panel_size.w == 0 || panel_size.h == 0 {
            return PauseMenuLayout::default();
        }

        let panel_ui = safe.place(panel_size, ui::Anchor::Center);
        let panel = Rect {
            x: panel_ui.x,
            y: panel_ui.y,
            w: panel_ui.w,
            h: panel_ui.h,
        };

        fill_rect(
            frame,
            width,
            height,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            COLOR_PAUSE_MENU_BG,
        );
        draw_rect_outline(
            frame,
            width,
            height,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            COLOR_PAUSE_MENU_BORDER,
        );

        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad),
            "PAUSED",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 24),
            "ESC TO RESUME",
            COLOR_PAUSE_MENU_TEXT,
        );

        let content = panel_ui.inset(ui::Insets::all(pad));
        let gap = 12u32;
        let button_size = ui::Size::new(240, 44).clamp_max(content.size());
        let resume_ui = content.place(button_size, ui::Anchor::BottomCenter);
        let resume_button = Rect {
            x: resume_ui.x,
            y: resume_ui.y,
            w: resume_ui.w,
            h: resume_ui.h,
        };
        let end_run_button = Rect {
            x: resume_button.x,
            y: resume_button.y.saturating_sub(resume_button.h.saturating_add(gap)),
            w: resume_button.w,
            h: resume_button.h,
        };

        ui_tree.ensure_container(UI_PAUSE_MENU_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_PAUSE_MENU_CONTAINER);
        ui_tree.ensure_button(UI_PAUSE_RESUME, resume_button, Some(ACTION_PAUSE_RESUME));
        ui_tree.add_child(UI_PAUSE_MENU_CONTAINER, UI_PAUSE_RESUME);
        ui_tree.ensure_button(UI_PAUSE_END_RUN, end_run_button, Some(ACTION_PAUSE_END_RUN));
        ui_tree.add_child(UI_PAUSE_MENU_CONTAINER, UI_PAUSE_END_RUN);

        draw_button(
            frame,
            width,
            height,
            resume_button,
            "RESUME",
            ui_tree.is_hovered(UI_PAUSE_RESUME),
        );
        draw_button(
            frame,
            width,
            height,
            end_run_button,
            "END RUN",
            ui_tree.is_hovered(UI_PAUSE_END_RUN),
        );

        PauseMenuLayout {
            panel,
            resume_button,
            end_run_button,
        }
    }
}

pub struct MainMenuView;

impl MainMenuView {
    pub fn render(frame: &mut dyn Renderer2d, width: u32, height: u32) -> MainMenuLayout {
        let mut ui_tree = UiTree::new();
        ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
        ui_tree.add_root(UI_CANVAS);
        Self::render_with_ui(frame, width, height, &mut ui_tree)
    }

    pub fn render_with_ui(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        ui_tree: &mut UiTree,
    ) -> MainMenuLayout {
        // Main menu is its own scene: clear the frame so the Tetris board is not visible underneath.
        fill_rect(frame, width, height, 0, 0, width, height, color_for_cell(0));

        let margin = 32u32;
        let pad = 18u32;

        // "Scene bounds" used for layout/hit-testing (not a modal panel).
        let screen = ui::Rect::from_size(width, height);
        let safe = screen.inset(ui::Insets::all(margin));
        if safe.w == 0 || safe.h == 0 {
            return MainMenuLayout::default();
        }

        let panel = Rect {
            x: safe.x,
            y: safe.y,
            w: safe.w,
            h: safe.h,
        };
        ui_tree.ensure_container(UI_SKILLTREE_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_SKILLTREE_CONTAINER);

        // Layout: a vertical stack (title, start, skilltree editor, quit), centered in the safe region.
        let title = MAIN_MENU_TITLE;
        let title_chars = title.chars().count() as u32;
        let glyph_cols = 4u32; // 3 glyph columns + 1 column spacing (matches `draw_text` advances).
        let denom = title_chars.saturating_mul(glyph_cols).max(1);
        let max_scale = 12u32;
        let title_scale = (safe.w / denom).clamp(2, max_scale);
        let title_w = denom.saturating_mul(title_scale).min(safe.w);
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

        let title_x = content.x.saturating_add(content.w.saturating_sub(title_w) / 2);
        let title_y = top_y;
        draw_text_scaled(
            frame,
            width,
            height,
            title_x,
            title_y,
            title,
            COLOR_PAUSE_MENU_TEXT,
            title_scale,
        );

        let buttons_y = title_y
            .saturating_add(title_h)
            .saturating_add(title_button_gap);
        let start_button = Rect {
            x: content.x.saturating_add(content.w.saturating_sub(button_size.w) / 2),
            y: buttons_y,
            w: button_size.w,
            h: button_size.h,
        };
        let skilltree_editor_button = Rect {
            x: start_button.x,
            y: start_button
                .y
                .saturating_add(start_button.h)
                .saturating_add(button_gap),
            w: start_button.w,
            h: start_button.h,
        };
        let quit_button = Rect {
            x: start_button.x,
            y: skilltree_editor_button
                .y
                .saturating_add(skilltree_editor_button.h)
                .saturating_add(button_gap),
            w: start_button.w,
            h: start_button.h,
        };

        ui_tree.ensure_container(UI_MAIN_MENU_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_MAIN_MENU_CONTAINER);
        ui_tree.ensure_button(UI_MAIN_MENU_START, start_button, Some(ACTION_MAIN_MENU_START));
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_START);
        ui_tree.ensure_button(
            UI_MAIN_MENU_SKILLTREE_EDITOR,
            skilltree_editor_button,
            Some(ACTION_MAIN_MENU_SKILLTREE_EDITOR),
        );
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_SKILLTREE_EDITOR);
        ui_tree.ensure_button(UI_MAIN_MENU_QUIT, quit_button, Some(ACTION_MAIN_MENU_QUIT));
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_QUIT);

        for (id, rect, label) in [
            (UI_MAIN_MENU_START, start_button, "START"),
            (UI_MAIN_MENU_SKILLTREE_EDITOR, skilltree_editor_button, "SKILLTREE EDITOR"),
            (UI_MAIN_MENU_QUIT, quit_button, "QUIT"),
        ] {
            let hovered = ui_tree.is_hovered(id);
            draw_button(frame, width, height, rect, label, hovered);
        }

        MainMenuLayout {
            panel,
            start_button,
            skilltree_editor_button,
            quit_button,
        }
    }
}

pub struct GameOverMenuView;

impl GameOverMenuView {
    pub fn render(frame: &mut dyn Renderer2d, width: u32, height: u32) -> GameOverMenuLayout {
        let mut ui_tree = UiTree::new();
        ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
        ui_tree.add_root(UI_CANVAS);
        Self::render_with_ui(frame, width, height, &mut ui_tree)
    }

    pub fn render_with_ui(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        ui_tree: &mut UiTree,
    ) -> GameOverMenuLayout {
        // Dim the entire game view.
        blend_rect(
            frame,
            width,
            height,
            0,
            0,
            width,
            height,
            COLOR_PAUSE_MENU_DIM,
            PAUSE_MENU_DIM_ALPHA,
        );

        let margin = 32u32;
        let pad = 18u32;

        let panel_w = 420u32.min(width.saturating_sub(margin.saturating_mul(2)));
        let panel_h = 280u32.min(height.saturating_sub(margin.saturating_mul(2)));
        if panel_w == 0 || panel_h == 0 {
            return GameOverMenuLayout::default();
        }

        let panel = Rect {
            x: width.saturating_sub(panel_w) / 2,
            y: height.saturating_sub(panel_h) / 2,
            w: panel_w,
            h: panel_h,
        };

        fill_rect(
            frame,
            width,
            height,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            COLOR_PAUSE_MENU_BG,
        );
        draw_rect_outline(
            frame,
            width,
            height,
            panel.x,
            panel.y,
            panel.w,
            panel.h,
            COLOR_PAUSE_MENU_BORDER,
        );

        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad),
            "GAME OVER",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 24),
            "RUN ENDED",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 48),
            "ENTER TO RESTART",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 72),
            "K: SKILL TREE",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 96),
            "ESC: MAIN MENU",
            COLOR_PAUSE_MENU_TEXT,
        );

        let button_h = 44u32.min(panel.h.saturating_sub(pad.saturating_mul(2)));
        let button_w = 240u32.min(panel.w.saturating_sub(pad.saturating_mul(2)));
        let gap = 12u32;
        let buttons_total_h = button_h
            .saturating_mul(3)
            .saturating_add(gap.saturating_mul(2));
        let top_y = panel
            .y
            .saturating_add(panel.h.saturating_sub(pad.saturating_add(buttons_total_h)));

        let restart_button = Rect {
            x: panel.x.saturating_add(panel.w.saturating_sub(button_w) / 2),
            y: top_y,
            w: button_w,
            h: button_h,
        };
        let skilltree_button = Rect {
            x: restart_button.x,
            y: restart_button.y.saturating_add(button_h + gap),
            w: button_w,
            h: button_h,
        };
        let quit_button = Rect {
            x: restart_button.x,
            y: skilltree_button.y.saturating_add(button_h + gap),
            w: button_w,
            h: button_h,
        };

        ui_tree.ensure_container(UI_GAME_OVER_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_GAME_OVER_CONTAINER);
        ui_tree.ensure_button(UI_GAME_OVER_RESTART, restart_button, Some(ACTION_GAME_OVER_RESTART));
        ui_tree.add_child(UI_GAME_OVER_CONTAINER, UI_GAME_OVER_RESTART);
        ui_tree.ensure_button(
            UI_GAME_OVER_SKILLTREE,
            skilltree_button,
            Some(ACTION_GAME_OVER_SKILLTREE),
        );
        ui_tree.add_child(UI_GAME_OVER_CONTAINER, UI_GAME_OVER_SKILLTREE);
        ui_tree.ensure_button(UI_GAME_OVER_QUIT, quit_button, Some(ACTION_GAME_OVER_QUIT));
        ui_tree.add_child(UI_GAME_OVER_CONTAINER, UI_GAME_OVER_QUIT);

        for (id, rect, label) in [
            (UI_GAME_OVER_RESTART, restart_button, "RESTART"),
            (UI_GAME_OVER_SKILLTREE, skilltree_button, "SKILL TREE"),
            (UI_GAME_OVER_QUIT, quit_button, "QUIT"),
        ] {
            let hovered = ui_tree.is_hovered(id);
            draw_button(frame, width, height, rect, label, hovered);
        }

        GameOverMenuLayout {
            panel,
            restart_button,
            skilltree_button,
            quit_button,
        }
    }
}

pub fn draw_pause_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> PauseMenuLayout {
    PauseMenuView::render(frame, width, height)
}

pub fn draw_pause_menu_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
) -> PauseMenuLayout {
    PauseMenuView::render_with_ui(frame, width, height, ui_tree)
}

pub fn draw_main_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> MainMenuLayout {
    MainMenuView::render(frame, width, height)
}

pub fn draw_main_menu_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
) -> MainMenuLayout {
    MainMenuView::render_with_ui(frame, width, height, ui_tree)
}

pub fn draw_game_over_menu(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
) -> GameOverMenuLayout {
    GameOverMenuView::render(frame, width, height)
}

pub fn draw_game_over_menu_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
) -> GameOverMenuLayout {
    GameOverMenuView::render_with_ui(frame, width, height, ui_tree)
}
