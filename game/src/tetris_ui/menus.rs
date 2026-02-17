use engine::graphics::Renderer2d;
use engine::render::color_for_cell;
use engine::slider::Slider;
use engine::ui;
use engine::ui_tree::UiTree;

use crate::settings::PlayerSettings;
use crate::ui_ids::*;

use super::{
    COLOR_PAUSE_MENU_BG, COLOR_PAUSE_MENU_BORDER, COLOR_PAUSE_MENU_DIM, COLOR_PAUSE_MENU_TEXT,
    MAIN_MENU_TITLE, PAUSE_MENU_DIM_ALPHA, Rect, blend_rect, draw_button, draw_rect_outline,
    draw_text, draw_text_scaled, fill_rect,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuLayout {
    pub panel: Rect,
    pub resume_button: Rect,
    pub end_run_button: Rect,
    pub settings_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MainMenuLayout {
    pub panel: Rect,
    pub start_button: Rect,
    pub skilltree_editor_button: Rect,
    pub settings_button: Rect,
    pub quit_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsMenuLayout {
    pub panel: Rect,
    pub master_track: Rect,
    pub music_track: Rect,
    pub sfx_track: Rect,
    pub shake_track: Rect,
    pub mute_toggle: Rect,
    pub music_toggle: Rect,
    pub show_timer_toggle: Rect,
    pub auto_pause_toggle: Rect,
    pub high_contrast_toggle: Rect,
    pub reduce_motion_toggle: Rect,
    pub back_button: Rect,
    pub reset_button: Rect,
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

        let panel_size = ui::Size::new(380, 320).clamp_max(safe.size());
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
        let base_ui = content.place(button_size, ui::Anchor::BottomCenter);
        let resume_button = Rect {
            x: base_ui.x,
            y: base_ui.y,
            w: base_ui.w,
            h: base_ui.h,
        };
        let settings_button = Rect {
            x: resume_button.x,
            y: resume_button
                .y
                .saturating_sub(resume_button.h.saturating_add(gap)),
            w: resume_button.w,
            h: resume_button.h,
        };
        let end_run_button = Rect {
            x: resume_button.x,
            y: settings_button
                .y
                .saturating_sub(settings_button.h.saturating_add(gap)),
            w: resume_button.w,
            h: resume_button.h,
        };

        ui_tree.ensure_container(UI_PAUSE_MENU_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_PAUSE_MENU_CONTAINER);
        ui_tree.ensure_button(UI_PAUSE_RESUME, resume_button, Some(ACTION_PAUSE_RESUME));
        ui_tree.add_child(UI_PAUSE_MENU_CONTAINER, UI_PAUSE_RESUME);
        ui_tree.ensure_button(UI_PAUSE_END_RUN, end_run_button, Some(ACTION_PAUSE_END_RUN));
        ui_tree.add_child(UI_PAUSE_MENU_CONTAINER, UI_PAUSE_END_RUN);
        ui_tree.ensure_button(UI_PAUSE_SETTINGS, settings_button, None);
        ui_tree.add_child(UI_PAUSE_MENU_CONTAINER, UI_PAUSE_SETTINGS);

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
            settings_button,
            "SETTINGS",
            ui_tree.is_hovered(UI_PAUSE_SETTINGS),
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
            settings_button,
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

        // Layout: a vertical stack (title, start, skilltree editor, settings, quit), centered.
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
            .saturating_add(button_size.h.saturating_mul(4))
            .saturating_add(button_gap.saturating_mul(3));
        let top_y = content
            .y
            .saturating_add(content.h.saturating_sub(stack_h) / 2);

        let title_x = content
            .x
            .saturating_add(content.w.saturating_sub(title_w) / 2);
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
            x: content
                .x
                .saturating_add(content.w.saturating_sub(button_size.w) / 2),
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
        let settings_button = Rect {
            x: start_button.x,
            y: skilltree_editor_button
                .y
                .saturating_add(skilltree_editor_button.h)
                .saturating_add(button_gap),
            w: start_button.w,
            h: start_button.h,
        };
        let quit_button = Rect {
            x: start_button.x,
            y: settings_button
                .y
                .saturating_add(settings_button.h)
                .saturating_add(button_gap),
            w: start_button.w,
            h: start_button.h,
        };

        ui_tree.ensure_container(UI_MAIN_MENU_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_MAIN_MENU_CONTAINER);
        ui_tree.ensure_button(
            UI_MAIN_MENU_START,
            start_button,
            Some(ACTION_MAIN_MENU_START),
        );
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_START);
        ui_tree.ensure_button(
            UI_MAIN_MENU_SKILLTREE_EDITOR,
            skilltree_editor_button,
            Some(ACTION_MAIN_MENU_SKILLTREE_EDITOR),
        );
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_SKILLTREE_EDITOR);
        ui_tree.ensure_button(UI_MAIN_MENU_SETTINGS, settings_button, None);
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_SETTINGS);
        ui_tree.ensure_button(UI_MAIN_MENU_QUIT, quit_button, Some(ACTION_MAIN_MENU_QUIT));
        ui_tree.add_child(UI_MAIN_MENU_CONTAINER, UI_MAIN_MENU_QUIT);

        for (id, rect, label) in [
            (UI_MAIN_MENU_START, start_button, "START"),
            (
                UI_MAIN_MENU_SKILLTREE_EDITOR,
                skilltree_editor_button,
                "SKILLTREE EDITOR",
            ),
            (UI_MAIN_MENU_SETTINGS, settings_button, "SETTINGS"),
            (UI_MAIN_MENU_QUIT, quit_button, "QUIT"),
        ] {
            let hovered = ui_tree.is_hovered(id);
            draw_button(frame, width, height, rect, label, hovered);
        }

        MainMenuLayout {
            panel,
            start_button,
            skilltree_editor_button,
            settings_button,
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
        ui_tree.ensure_button(
            UI_GAME_OVER_RESTART,
            restart_button,
            Some(ACTION_GAME_OVER_RESTART),
        );
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

pub struct SettingsMenuView;

impl SettingsMenuView {
    pub fn render(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        settings: &PlayerSettings,
    ) -> SettingsMenuLayout {
        let mut ui_tree = UiTree::new();
        ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
        ui_tree.add_root(UI_CANVAS);
        Self::render_with_ui(frame, width, height, &mut ui_tree, settings)
    }

    pub fn render_with_ui(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        ui_tree: &mut UiTree,
        settings: &PlayerSettings,
    ) -> SettingsMenuLayout {
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
        let screen = ui::Rect::from_size(width, height);
        let safe = screen.inset(ui::Insets::all(margin));
        if safe.w == 0 || safe.h == 0 {
            return SettingsMenuLayout::default();
        }

        let panel_size = ui::Size::new(760, 560).clamp_max(safe.size());
        if panel_size.w == 0 || panel_size.h == 0 {
            return SettingsMenuLayout::default();
        }

        let panel_ui = safe.place(panel_size, ui::Anchor::Center);
        let panel = Rect::new(panel_ui.x, panel_ui.y, panel_ui.w, panel_ui.h);
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
            "SETTINGS",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            panel.x.saturating_add(pad),
            panel.y.saturating_add(pad + 24),
            "ESC: BACK  DRAG SLIDERS TO APPLY",
            COLOR_PAUSE_MENU_TEXT,
        );

        ui_tree.ensure_container(UI_SETTINGS_MENU_CONTAINER, panel);
        ui_tree.add_child(UI_CANVAS, UI_SETTINGS_MENU_CONTAINER);

        let content = panel.inset(ui::Insets::all(pad));
        let slider_label_x = content.x.saturating_add(8);
        let slider_track_x = content.x.saturating_add(250);
        let slider_track_w = content.w.saturating_sub(270);
        let slider_track_h = 12u32;
        let slider_row_h = 46u32;
        let row0_y = content.y.saturating_add(58);

        let master_track = Rect::new(slider_track_x, row0_y + 10, slider_track_w, slider_track_h);
        let music_track = Rect::new(
            slider_track_x,
            row0_y.saturating_add(slider_row_h).saturating_add(10),
            slider_track_w,
            slider_track_h,
        );
        let sfx_track = Rect::new(
            slider_track_x,
            row0_y
                .saturating_add(slider_row_h.saturating_mul(2))
                .saturating_add(10),
            slider_track_w,
            slider_track_h,
        );
        let shake_track = Rect::new(
            slider_track_x,
            row0_y
                .saturating_add(slider_row_h.saturating_mul(3))
                .saturating_add(10),
            slider_track_w,
            slider_track_h,
        );

        draw_slider_row(
            frame,
            width,
            height,
            slider_label_x,
            row0_y,
            "MASTER VOLUME",
            Slider::new(master_track, 0.0, 1.0, settings.audio.master_volume),
            true,
        );
        draw_slider_row(
            frame,
            width,
            height,
            slider_label_x,
            row0_y.saturating_add(slider_row_h),
            "MUSIC VOLUME",
            Slider::new(music_track, 0.0, 1.0, settings.audio.music_volume),
            true,
        );
        draw_slider_row(
            frame,
            width,
            height,
            slider_label_x,
            row0_y.saturating_add(slider_row_h.saturating_mul(2)),
            "SFX VOLUME",
            Slider::new(sfx_track, 0.0, 1.0, settings.audio.sfx_volume),
            true,
        );
        draw_slider_row(
            frame,
            width,
            height,
            slider_label_x,
            row0_y.saturating_add(slider_row_h.saturating_mul(3)),
            "SCREEN SHAKE",
            Slider::new(
                shake_track,
                0.0,
                100.0,
                settings.video.clamped_screen_shake() as f32,
            ),
            false,
        );

        let toggle_y0 = row0_y
            .saturating_add(slider_row_h.saturating_mul(4))
            .saturating_add(12);
        let toggle_h = 34u32;
        let toggle_w = 200u32.min(content.w.saturating_sub(16));
        let toggle_gap = 10u32;
        let left_x = content.x.saturating_add(8);
        let right_x = content
            .x
            .saturating_add(content.w.saturating_sub(toggle_w).saturating_sub(8));

        let mute_toggle = Rect::new(left_x, toggle_y0, toggle_w, toggle_h);
        let music_toggle = Rect::new(
            left_x,
            toggle_y0 + (toggle_h + toggle_gap),
            toggle_w,
            toggle_h,
        );
        let show_timer_toggle = Rect::new(right_x, toggle_y0, toggle_w, toggle_h);
        let auto_pause_toggle = Rect::new(
            right_x,
            toggle_y0 + (toggle_h + toggle_gap),
            toggle_w,
            toggle_h,
        );
        let high_contrast_toggle = Rect::new(
            left_x,
            toggle_y0 + (toggle_h + toggle_gap) * 2,
            toggle_w,
            toggle_h,
        );
        let reduce_motion_toggle = Rect::new(
            right_x,
            toggle_y0 + (toggle_h + toggle_gap) * 2,
            toggle_w,
            toggle_h,
        );

        for (id, rect, label, on) in [
            (
                UI_SETTINGS_TOGGLE_MUTE,
                mute_toggle,
                "MUTE ALL",
                settings.audio.mute_all,
            ),
            (
                UI_SETTINGS_TOGGLE_MUSIC,
                music_toggle,
                "MUSIC ENABLED",
                settings.audio.music_enabled,
            ),
            (
                UI_SETTINGS_TOGGLE_TIMER,
                show_timer_toggle,
                "SHOW TIMER",
                settings.gameplay.show_round_timer,
            ),
            (
                UI_SETTINGS_TOGGLE_AUTO_PAUSE,
                auto_pause_toggle,
                "AUTO PAUSE (FOCUS)",
                settings.gameplay.auto_pause_on_focus_loss,
            ),
            (
                UI_SETTINGS_TOGGLE_HIGH_CONTRAST,
                high_contrast_toggle,
                "HIGH CONTRAST UI",
                settings.accessibility.high_contrast_ui,
            ),
            (
                UI_SETTINGS_TOGGLE_REDUCE_MOTION,
                reduce_motion_toggle,
                "REDUCE MOTION",
                settings.accessibility.reduce_motion,
            ),
        ] {
            ui_tree.ensure_button(id, rect, None);
            ui_tree.add_child(UI_SETTINGS_MENU_CONTAINER, id);
            let state = if on { "ON" } else { "OFF" };
            let line = format!("{label}: {state}");
            draw_button(frame, width, height, rect, &line, ui_tree.is_hovered(id));
        }

        let button_size = ui::Size::new(220, 42).clamp_max(content.size());
        let back_button_ui = content.place(button_size, ui::Anchor::BottomRight);
        let back_button = Rect::new(
            back_button_ui.x,
            back_button_ui.y,
            back_button_ui.w,
            back_button_ui.h,
        );
        let reset_button = Rect::new(
            back_button
                .x
                .saturating_sub(button_size.w.saturating_add(12)),
            back_button.y,
            button_size.w,
            button_size.h,
        );
        ui_tree.ensure_button(UI_SETTINGS_BACK, back_button, None);
        ui_tree.add_child(UI_SETTINGS_MENU_CONTAINER, UI_SETTINGS_BACK);
        ui_tree.ensure_button(UI_SETTINGS_RESET, reset_button, None);
        ui_tree.add_child(UI_SETTINGS_MENU_CONTAINER, UI_SETTINGS_RESET);

        draw_button(
            frame,
            width,
            height,
            back_button,
            "BACK",
            ui_tree.is_hovered(UI_SETTINGS_BACK),
        );
        draw_button(
            frame,
            width,
            height,
            reset_button,
            "RESET DEFAULTS",
            ui_tree.is_hovered(UI_SETTINGS_RESET),
        );

        SettingsMenuLayout {
            panel,
            master_track,
            music_track,
            sfx_track,
            shake_track,
            mute_toggle,
            music_toggle,
            show_timer_toggle,
            auto_pause_toggle,
            high_contrast_toggle,
            reduce_motion_toggle,
            back_button,
            reset_button,
        }
    }
}

fn draw_slider_row(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    label_x: u32,
    label_y: u32,
    label: &str,
    slider: Slider,
    unit_percent: bool,
) {
    draw_text(
        frame,
        width,
        height,
        label_x,
        label_y,
        label,
        COLOR_PAUSE_MENU_TEXT,
    );

    let track = slider.track;
    fill_rect(
        frame,
        width,
        height,
        track.x,
        track.y,
        track.w,
        track.h,
        [34, 34, 48, 255],
    );
    draw_rect_outline(
        frame,
        width,
        height,
        track.x,
        track.y,
        track.w,
        track.h,
        COLOR_PAUSE_MENU_BORDER,
    );

    let t = slider.normalized_value();
    let fill_w = ((track.w as f32) * t).round() as u32;
    fill_rect(
        frame,
        width,
        height,
        track.x,
        track.y,
        fill_w.min(track.w),
        track.h,
        [90, 130, 235, 255],
    );

    let thumb = slider.thumb_rect(14, 22);
    fill_rect(
        frame,
        width,
        height,
        thumb.x,
        thumb.y,
        thumb.w,
        thumb.h,
        [235, 235, 245, 255],
    );
    draw_rect_outline(
        frame,
        width,
        height,
        thumb.x,
        thumb.y,
        thumb.w,
        thumb.h,
        [16, 16, 24, 255],
    );

    let value = if unit_percent {
        format!("{:>3}%", (slider.value * 100.0).round() as i32)
    } else {
        format!("{:>3}%", slider.value.round() as i32)
    };
    draw_text(
        frame,
        width,
        height,
        track.x.saturating_add(track.w).saturating_sub(54),
        label_y,
        &value,
        COLOR_PAUSE_MENU_TEXT,
    );
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

pub fn draw_settings_menu(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    settings: &PlayerSettings,
) -> SettingsMenuLayout {
    SettingsMenuView::render(frame, width, height, settings)
}

pub fn draw_settings_menu_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
    settings: &PlayerSettings,
) -> SettingsMenuLayout {
    SettingsMenuView::render_with_ui(frame, width, height, ui_tree, settings)
}
