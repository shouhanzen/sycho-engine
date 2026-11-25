use std::time::{Duration, Instant};
use std::{io, path::Path};

use engine::embedded::valid_preview_size;
use engine::graphics::{FittedRenderer, Renderer2d};
use engine::recording::{Mp4Config, export_timeline};
use engine::surface::SurfaceSize;
use engine::timeline::TimelineReader;

use crate::debug::DebugHud;
use crate::game_ui::{GameUiLayout, Menu};
use crate::state::{GameState, TIMELINE_SCHEMA};
use crate::tetris_ui::{
    GameOverMenuView, MainMenuView, PauseMenuView, Rect, SettingsMenuView,
    draw_skilltree_in_layout, draw_tetris_hud_view, draw_tetris_world_in_layout,
};
use crate::view::GameView;

pub struct RenderInputs {
    pub world_offset_y_px: i32,
    pub frame_dt: Duration,
    pub show_round_timer: bool,
    pub historical: bool,
}

pub struct DrawTimings {
    frame_dt: Duration,
    board_dt: Duration,
    draw_dt: Duration,
    overlay_dt: Duration,
    frame_total_dt: Duration,
}

impl DrawTimings {
    pub fn record(self, debug_hud: &mut DebugHud) {
        debug_hud.on_frame(
            self.frame_dt,
            self.board_dt,
            self.draw_dt,
            self.overlay_dt,
            Duration::ZERO,
            self.frame_total_dt,
        );
    }
}

pub fn export_video(directory: &Path, output: &Path) -> io::Result<()> {
    let timeline = TimelineReader::open(directory, TIMELINE_SCHEMA)?;
    let mut debug_hud = DebugHud::new();
    let config = Mp4Config {
        width: 1280,
        height: 720,
        fps: 60,
    };
    export_timeline(&timeline, output, config, |state, renderer| {
        render_frame(
            renderer,
            &debug_hud,
            state,
            RenderInputs {
                world_offset_y_px: 0,
                frame_dt: Duration::ZERO,
                historical: true,
                show_round_timer: true,
            },
        )
        .record(&mut debug_hud);
    })
}

pub fn render_frame(
    renderer: &mut dyn Renderer2d,
    debug_hud: &DebugHud,
    state: &GameState,
    inputs: RenderInputs,
) -> DrawTimings {
    if inputs.historical
        && let Some(input) = &state.input
    {
        let viewport = SurfaceSize::new(input.viewport.0, input.viewport.1);
        if viewport != renderer.size() && valid_preview_size(viewport) {
            renderer.clear([0, 0, 0, 255]);
            return draw_frame(
                &mut FittedRenderer::new(renderer, viewport),
                debug_hud,
                state,
                inputs,
            );
        }
    }
    draw_frame(renderer, debug_hud, state, inputs)
}

fn draw_frame(
    renderer: &mut dyn Renderer2d,
    debug_hud: &DebugHud,
    state: &GameState,
    inputs: RenderInputs,
) -> DrawTimings {
    let frame_start = Instant::now();
    let board_start = Instant::now();
    let view = state.view;
    let board_dt = board_start.elapsed();

    let size = renderer.size();
    let pointer = state
        .input
        .as_ref()
        .and_then(|input| input.pointer_at(size));
    let settings = state.settings_overlay.as_ref();
    let layout = GameUiLayout::compute(state, size, settings.is_some());
    let game_pointer = if settings.is_some() { None } else { pointer };

    let draw_start = Instant::now();
    if let Some(tetris) = layout.tetris {
        draw_tetris_world_in_layout(
            renderer,
            size.width,
            size.height,
            state.tetris(),
            tetris,
            inputs.world_offset_y_px,
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
        .filter(|_| view.is_tetris() && inputs.show_round_timer)
    {
        let hud_x = tetris.pause_button.x.saturating_sub(180);
        let hud_y = tetris.pause_button.y.saturating_add(34);
        let timer_disabled = debug_hud.round_timer_disabled();
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

    let draw_dt = draw_start.elapsed();

    let overlay_start = Instant::now();
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
    debug_hud.draw_overlay(renderer, size.width, size.height);
    if let Some((x, y)) = pointer.filter(|_| inputs.historical) {
        draw_recorded_cursor(renderer, x, y);
    }
    let overlay_dt = overlay_start.elapsed();

    DrawTimings {
        frame_dt: inputs.frame_dt,
        board_dt,
        draw_dt,
        overlay_dt,
        frame_total_dt: frame_start.elapsed(),
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
