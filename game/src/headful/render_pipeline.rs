use std::time::{Duration, Instant};

use engine::graphics::Renderer2d;
use engine::ui_tree::UiTree;

use crate::debug::DebugHud;
use crate::state::GameState;
use crate::tetris_ui::{
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SkillTreeLayout, UiLayout,
    draw_game_over_menu_with_ui, draw_main_menu_with_ui, draw_pause_menu_with_ui,
    draw_skilltree_runtime_with_ui_and_mouse, draw_tetris_hud_view,
    draw_tetris_world_with_camera_offset,
};
use crate::ui_ids::UI_CANVAS;
use crate::view::GameView;

#[derive(Clone, Copy, Default)]
pub struct RenderCache {
    pub last_layout: UiLayout,
    pub last_main_menu: MainMenuLayout,
    pub last_pause_menu: PauseMenuLayout,
    pub last_skilltree: SkillTreeLayout,
    pub last_game_over_menu: GameOverMenuLayout,
}

pub fn render_frame(
    renderer: &mut dyn Renderer2d,
    ui_tree: &mut UiTree,
    debug_hud: &mut DebugHud,
    state: &GameState,
    mouse_x: u32,
    mouse_y: u32,
    cache: &mut RenderCache,
    world_offset_y_px: i32,
    last_frame_dt: Duration,
) {
    let frame_start = Instant::now();
    let board_start = Instant::now();
    let view = state.view;
    let board_dt = board_start.elapsed();

    let size = renderer.size();
    ui_tree.begin_frame();
    ui_tree.ensure_canvas(UI_CANVAS, Rect::from_size(size.width, size.height));
    ui_tree.add_root(UI_CANVAS);

    let draw_start = Instant::now();
    if matches!(view, GameView::SkillTree | GameView::MainMenu) {
        cache.last_layout = UiLayout::default();
    } else {
        let tetris_layout = draw_tetris_world_with_camera_offset(
            renderer,
            size.width,
            size.height,
            state.tetris(),
            world_offset_y_px,
        );
        if view.is_tetris() {
            draw_tetris_hud_view(
                renderer,
                size.width,
                size.height,
                state.tetris(),
                tetris_layout,
                Some((mouse_x, mouse_y)),
            );
        }
        cache.last_layout = tetris_layout;
    }

    if view.is_tetris() {
        let hud_x = cache.last_layout.pause_button.x.saturating_sub(180);
        let hud_y = cache
            .last_layout
            .pause_button
            .y
            .saturating_add(6)
            .saturating_add(28);
        let remaining_s = state.round_timer.remaining().as_secs_f32();
        let timer_text = format!("TIME {remaining_s:>4.1}");
        renderer.draw_text(hud_x, hud_y, &timer_text, [235, 235, 245, 255]);
    }

    let draw_dt = draw_start.elapsed();

    let overlay_start = Instant::now();
    match view {
        GameView::MainMenu => {
            cache.last_main_menu =
                draw_main_menu_with_ui(renderer, size.width, size.height, ui_tree);
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
        }
        GameView::SkillTree => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = draw_skilltree_runtime_with_ui_and_mouse(
                renderer,
                size.width,
                size.height,
                ui_tree,
                &state.skilltree,
                Some((mouse_x, mouse_y)),
            );
            cache.last_game_over_menu = GameOverMenuLayout::default();
        }
        GameView::Tetris { paused: true } => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu =
                draw_pause_menu_with_ui(renderer, size.width, size.height, ui_tree);
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
        }
        GameView::Tetris { paused: false } => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
        }
        GameView::GameOver => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu =
                draw_game_over_menu_with_ui(renderer, size.width, size.height, ui_tree);
        }
    }
    debug_hud.draw_overlay(renderer, size.width, size.height);
    let overlay_dt = overlay_start.elapsed();

    let present_dt = Duration::ZERO;
    let frame_total_dt = frame_start.elapsed();
    debug_hud.on_frame(
        last_frame_dt,
        board_dt,
        draw_dt,
        overlay_dt,
        present_dt,
        frame_total_dt,
    );
}
