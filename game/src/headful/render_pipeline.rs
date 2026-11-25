use std::{io, path::Path};

use engine::embedded::valid_preview_size;
use engine::graphics::{FittedRenderer, Renderer2d};
use engine::recording::{Mp4Config, export_timeline};
use engine::timeline::TimelineReader;

use crate::debug::DebugHud;
use crate::game_ui::{GameUiLayout, Menu};
use crate::state::{GameState, TIMELINE_SCHEMA};
use crate::tetris_ui::{
    GameOverMenuView, MainMenuView, PauseMenuView, Rect, SettingsMenuView,
    draw_skilltree_in_layout, draw_tetris_hud_view, draw_tetris_world_in_layout,
};
use crate::view::GameView;

#[derive(Default)]
pub struct FrameOverlays<'a> {
    pub debug_hud: Option<&'a DebugHud>,
    pub playback_cursor: bool,
}

pub fn export_video(directory: &Path, output: &Path) -> io::Result<()> {
    let timeline = TimelineReader::open(directory, TIMELINE_SCHEMA)?;
    let config = Mp4Config {
        width: 1280,
        height: 720,
        fps: 60,
    };
    export_timeline(&timeline, output, config, |state, renderer| {
        render_frame(
            renderer,
            state,
            FrameOverlays {
                playback_cursor: true,
                ..FrameOverlays::default()
            },
        );
    })
}

pub fn render_frame(renderer: &mut dyn Renderer2d, state: &GameState, overlays: FrameOverlays<'_>) {
    let viewport = state.display.viewport;
    if viewport != renderer.size() && valid_preview_size(viewport) {
        renderer.clear([0, 0, 0, 255]);
        draw_frame(
            &mut FittedRenderer::new(renderer, viewport),
            state,
            overlays,
        )
    } else {
        draw_frame(renderer, state, overlays)
    }
}

fn draw_frame(renderer: &mut dyn Renderer2d, state: &GameState, overlays: FrameOverlays<'_>) {
    let view = state.view;

    let size = renderer.size();
    let pointer = state.pointer();
    let settings = state.settings.open.then_some(&state.settings.values);
    let layout = GameUiLayout::compute(state, size);
    let game_pointer = if settings.is_some() { None } else { pointer };

    if let Some(tetris) = layout.tetris {
        draw_tetris_world_in_layout(
            renderer,
            size.width,
            size.height,
            state.tetris(),
            tetris,
            state.display.world_offset_y_px(),
        );
        if view.is_tetris() {
            draw_tetris_hud_view(
                renderer,
                size.width,
                size.height,
                state.tetris(),
                tetris,
                game_pointer,
            );
        }
    }

    if let Some(tetris) = layout
        .tetris
        .filter(|_| view.is_tetris() && state.display.show_round_timer)
    {
        let hud_x = tetris.pause_button.x.saturating_sub(180);
        let hud_y = tetris.pause_button.y.saturating_add(34);
        let timer_disabled = state.round_timer_disabled;
        let timer_text = if timer_disabled {
            "TIME OFF".to_string()
        } else {
            let remaining_s = state.round_timer.remaining().as_secs_f32();
            format!("TIME {remaining_s:>4.1}")
        };
        let timer_color = if timer_disabled {
            [245, 198, 92, 255]
        } else {
            [235, 235, 245, 255]
        };
        renderer.draw_text(hud_x, hud_y, &timer_text, timer_color);
    }

    match layout.menu {
        Some(Menu::Main(menu)) => {
            MainMenuView::draw(renderer, size.width, size.height, menu, game_pointer)
        }
        Some(Menu::Pause(menu)) => {
            PauseMenuView::draw(renderer, size.width, size.height, menu, game_pointer)
        }
        Some(Menu::GameOver(menu)) => {
            GameOverMenuView::draw(renderer, size.width, size.height, menu, game_pointer)
        }
        None if view == GameView::SkillTree => draw_skilltree_in_layout(
            renderer,
            size.width,
            size.height,
            layout.skilltree,
            &state.skilltree,
            game_pointer,
        ),
        None => {}
    }
    if let Some((settings, menu)) = settings.zip(layout.settings) {
        SettingsMenuView::draw(renderer, size.width, size.height, menu, settings, pointer);
    }
    if let Some(debug_hud) = overlays.debug_hud {
        debug_hud.draw_overlay(renderer, size.width, size.height);
    }
    if let Some((x, y)) = pointer.filter(|_| overlays.playback_cursor) {
        draw_recorded_cursor(renderer, x, y);
    }
}

// A small outlined pointer remains visible over both light and dark game scenes.
fn draw_recorded_cursor(renderer: &mut dyn Renderer2d, x: u32, y: u32) {
    for (dy, width) in [2, 3, 4, 5, 6, 7, 8, 5, 4, 2].into_iter().enumerate() {
        let y = y.saturating_add(dy as u32);
        renderer.fill_rect(Rect::new(x, y, width, 1), [18, 18, 24, 255]);
        if dy > 0 && dy < 9 && width > 2 {
            renderer.fill_rect(Rect::new(x + 1, y, width - 2, 1), [245, 245, 255, 255]);
        }
    }
}
