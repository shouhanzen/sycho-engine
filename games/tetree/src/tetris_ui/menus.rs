use engine::graphics::Renderer2d;
use engine::slider::Slider;
use engine::ui;

use crate::board_render::color_for_cell;
use crate::settings::PlayerSettings;

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
    title_x: u32,
    title_y: u32,
    title_scale: u32,
    pub settings_button: Rect,
    pub quit_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SettingsMenuLayout {
    pub panel: Rect,
    slider_tracks: [Rect; SettingsSlider::ALL.len()],
    pub mute_toggle: Rect,
    pub music_toggle: Rect,
    pub show_timer_toggle: Rect,
    pub auto_pause_toggle: Rect,
    pub reduce_motion_toggle: Rect,
    pub back_button: Rect,
    pub reset_button: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSlider {
    Master,
    Music,
    Sfx,
    ScreenShake,
}

impl SettingsSlider {
    const ALL: [Self; 4] = [Self::Master, Self::Music, Self::Sfx, Self::ScreenShake];

    fn label(self) -> &'static str {
        match self {
            Self::Master => "MASTER VOLUME",
            Self::Music => "MUSIC VOLUME",
            Self::Sfx => "SFX VOLUME",
            Self::ScreenShake => "SCREEN SHAKE",
        }
    }

    fn slider(self, track: Rect, settings: &PlayerSettings) -> Slider {
        let value = match self {
            Self::Master => settings.audio.master_volume,
            Self::Music => settings.audio.music_volume,
            Self::Sfx => settings.audio.sfx_volume,
            Self::ScreenShake => f32::from(settings.video.clamped_screen_shake()) / 100.0,
        };
        Slider::new(track, 0.0, 1.0, value)
    }

    fn set_value(self, settings: &mut PlayerSettings, value: f32) -> bool {
        let audio_gain = match self {
            Self::Master => &mut settings.audio.master_volume,
            Self::Music => &mut settings.audio.music_volume,
            Self::Sfx => &mut settings.audio.sfx_volume,
            Self::ScreenShake => {
                let next = (value * 100.0).round() as u8;
                return std::mem::replace(&mut settings.video.screen_shake_percent, next) != next;
            }
        };
        let changed = (*audio_gain - value).abs() > 1e-4;
        if changed {
            *audio_gain = value;
        }
        changed
    }
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
        let layout = PauseMenuLayout::compute(width, height);
        Self::draw(frame, width, height, layout, None);
        layout
    }

    pub(crate) fn draw(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        layout: PauseMenuLayout,
        pointer: Option<(u32, u32)>,
    ) {
        let hovered = |rect: Rect| pointer.is_some_and(|(x, y)| rect.contains(x, y));
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

        let PauseMenuLayout {
            panel,
            resume_button,
            end_run_button,
            settings_button,
        } = layout;
        if panel.w == 0 || panel.h == 0 {
            return;
        }
        let pad = 18u32;
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

        draw_button(
            frame,
            width,
            height,
            resume_button,
            "RESUME",
            hovered(resume_button),
        );
        draw_button(
            frame,
            width,
            height,
            settings_button,
            "SETTINGS",
            hovered(settings_button),
        );
        draw_button(
            frame,
            width,
            height,
            end_run_button,
            "END RUN",
            hovered(end_run_button),
        );
    }
}

impl PauseMenuLayout {
    pub(crate) fn compute(width: u32, height: u32) -> Self {
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
        let layout = MainMenuLayout::compute(width, height);
        Self::draw(frame, width, height, layout, None);
        layout
    }

    pub(crate) fn draw(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        layout: MainMenuLayout,
        pointer: Option<(u32, u32)>,
    ) {
        let hovered = |rect: Rect| pointer.is_some_and(|(x, y)| rect.contains(x, y));
        // Main menu is its own scene: clear the frame so the Tetris board is not visible underneath.
        fill_rect(frame, width, height, 0, 0, width, height, color_for_cell(0));

        let MainMenuLayout {
            panel,
            title_x,
            title_y,
            title_scale,
            start_button,
            skilltree_editor_button,
            settings_button,
            quit_button,
        } = layout;
        if panel.w == 0 || panel.h == 0 {
            return;
        }
        draw_text_scaled(
            frame,
            width,
            height,
            title_x,
            title_y,
            MAIN_MENU_TITLE,
            COLOR_PAUSE_MENU_TEXT,
            title_scale,
        );

        for (rect, label) in [
            (start_button, "START"),
            (skilltree_editor_button, "SKILLTREE EDITOR"),
            (settings_button, "SETTINGS"),
            (quit_button, "QUIT"),
        ] {
            let hovered = hovered(rect);
            draw_button(frame, width, height, rect, label, hovered);
        }
    }
}

impl MainMenuLayout {
    pub(crate) fn compute(width: u32, height: u32) -> Self {
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

        MainMenuLayout {
            panel,
            title_x,
            title_y,
            title_scale,
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
        let layout = GameOverMenuLayout::compute(width, height);
        Self::draw(frame, width, height, layout, None);
        layout
    }

    pub(crate) fn draw(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        layout: GameOverMenuLayout,
        pointer: Option<(u32, u32)>,
    ) {
        let hovered = |rect: Rect| pointer.is_some_and(|(x, y)| rect.contains(x, y));
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

        let GameOverMenuLayout {
            panel,
            restart_button,
            skilltree_button,
            quit_button,
        } = layout;
        if panel.w == 0 || panel.h == 0 {
            return;
        }
        let pad = 18u32;
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

        for (rect, label) in [
            (restart_button, "RESTART"),
            (skilltree_button, "SKILL TREE"),
            (quit_button, "QUIT"),
        ] {
            let hovered = hovered(rect);
            draw_button(frame, width, height, rect, label, hovered);
        }
    }
}

impl GameOverMenuLayout {
    pub(crate) fn compute(width: u32, height: u32) -> Self {
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
    pub(crate) fn draw(
        frame: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        layout: SettingsMenuLayout,
        settings: &PlayerSettings,
        pointer: Option<(u32, u32)>,
    ) {
        let hovered = |rect: Rect| pointer.is_some_and(|(x, y)| rect.contains(x, y));
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

        let SettingsMenuLayout {
            panel,
            mute_toggle,
            music_toggle,
            show_timer_toggle,
            auto_pause_toggle,
            reduce_motion_toggle,
            back_button,
            reset_button,
            ..
        } = layout;
        if panel.w == 0 || panel.h == 0 {
            return;
        }
        let pad = 18u32;
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

        let slider_label_x = panel.x.saturating_add(pad + 8);
        for control in SettingsSlider::ALL {
            let slider = layout.slider(control, settings);
            draw_slider_row(
                frame,
                width,
                height,
                slider_label_x,
                slider.track.y.saturating_sub(10),
                control.label(),
                slider,
            );
        }

        for (rect, label, on) in [
            (mute_toggle, "MUTE ALL", settings.audio.mute_all),
            (music_toggle, "MUSIC ENABLED", settings.audio.music_enabled),
            (
                show_timer_toggle,
                "SHOW TIMER",
                settings.gameplay.show_round_timer,
            ),
            (
                auto_pause_toggle,
                "AUTO PAUSE (FOCUS)",
                settings.gameplay.auto_pause_on_focus_loss,
            ),
            (
                reduce_motion_toggle,
                "REDUCE MOTION",
                settings.accessibility.reduce_motion,
            ),
        ] {
            let state = if on { "ON" } else { "OFF" };
            let line = format!("{label}: {state}");
            draw_button(frame, width, height, rect, &line, hovered(rect));
        }

        draw_button(
            frame,
            width,
            height,
            back_button,
            "BACK",
            hovered(back_button),
        );
        draw_button(
            frame,
            width,
            height,
            reset_button,
            "RESET DEFAULTS",
            hovered(reset_button),
        );
    }
}

impl SettingsMenuLayout {
    fn slider(&self, control: SettingsSlider, settings: &PlayerSettings) -> Slider {
        control.slider(self.slider_tracks[control as usize], settings)
    }

    pub fn slider_at(&self, x: u32, y: u32) -> Option<SettingsSlider> {
        SettingsSlider::ALL
            .into_iter()
            .find(|control| self.slider_tracks[*control as usize].contains(x, y))
    }

    pub fn set_slider_x(
        &self,
        control: SettingsSlider,
        settings: &mut PlayerSettings,
        x: u32,
    ) -> bool {
        let value = self.slider(control, settings).value_from_x(x);
        control.set_value(settings, value)
    }

    pub(crate) fn compute(width: u32, height: u32) -> Self {
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
        let content = panel.inset(ui::Insets::all(pad));
        let slider_track_x = content.x.saturating_add(250);
        let slider_track_w = content.w.saturating_sub(270);
        let slider_track_h = 12u32;
        let slider_row_h = 46u32;
        let row0_y = content.y.saturating_add(58);

        let slider_tracks = SettingsSlider::ALL.map(|control| {
            let y = row0_y
                .saturating_add(slider_row_h * control as u32)
                .saturating_add(10);
            Rect::new(slider_track_x, y, slider_track_w, slider_track_h)
        });

        let toggle_y0 = row0_y
            .saturating_add(slider_row_h.saturating_mul(slider_tracks.len() as u32))
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
        let reduce_motion_toggle = Rect::new(
            right_x,
            toggle_y0 + (toggle_h + toggle_gap) * 2,
            toggle_w,
            toggle_h,
        );

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

        SettingsMenuLayout {
            panel,
            slider_tracks,
            mute_toggle,
            music_toggle,
            show_timer_toggle,
            auto_pause_toggle,
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

    let value = format!("{:>3}%", (slider.value * 100.0).round() as i32);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_slider_hit_and_drag_update_the_rendered_value() {
        let layout = SettingsMenuLayout::compute(960, 640);
        let mut settings = PlayerSettings::default();
        for control in SettingsSlider::ALL {
            let track = layout.slider(control, &settings).track;
            assert!(track.w > 1);
            assert_eq!(layout.slider_at(track.x, track.y), Some(control));
            assert!(layout.set_slider_x(control, &mut settings, 0));
            assert_eq!(layout.slider(control, &settings).value, 0.0);
            assert!(!layout.set_slider_x(control, &mut settings, 0));
        }
        assert_eq!(settings.audio.master_volume, 0.0);
        assert_eq!(settings.audio.music_volume, 0.0);
        assert_eq!(settings.audio.sfx_volume, 0.0);
        assert_eq!(settings.video.screen_shake_percent, 0);

        assert!(layout.set_slider_x(SettingsSlider::ScreenShake, &mut settings, u32::MAX));
        assert_eq!(settings.video.screen_shake_percent, 100);
    }
}

pub fn draw_pause_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> PauseMenuLayout {
    PauseMenuView::render(frame, width, height)
}

pub fn draw_main_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> MainMenuLayout {
    MainMenuView::render(frame, width, height)
}

pub fn draw_game_over_menu(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
) -> GameOverMenuLayout {
    GameOverMenuView::render(frame, width, height)
}
