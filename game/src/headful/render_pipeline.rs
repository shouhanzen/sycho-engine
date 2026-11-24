use std::time::{Duration, Instant};
use std::{io, path::Path};

use engine::graphics::Renderer2d;
use engine::recording::{Mp4Config, export_timeline};
use engine::timeline::TimelineReader;
use engine::ui_tree::UiTree;

use crate::debug::DebugHud;
use crate::settings::PlayerSettings;
use crate::state::{GameState, TIMELINE_SCHEMA};
use crate::tetris_ui::{
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SettingsMenuLayout, SkillTreeLayout,
    UiLayout, draw_game_over_menu_with_ui, draw_main_menu_with_ui, draw_pause_menu_with_ui,
    draw_settings_menu_with_ui, draw_skilltree_runtime_with_ui_and_mouse, draw_tetris_hud_view,
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
    pub last_settings_menu: SettingsMenuLayout,
}

pub struct RenderInputs {
    pub mouse: (u32, u32),
    pub world_offset_y_px: i32,
    pub frame_dt: Duration,
    pub settings_overlay: Option<PlayerSettings>,
    pub show_round_timer: bool,
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
    let mut ui_tree = UiTree::new();
    let mut debug_hud = DebugHud::new();
    let mut cache = RenderCache::default();
    let config = Mp4Config {
        width: 1280,
        height: 720,
        fps: 30,
    };
    export_timeline(&timeline, output, config, |state, renderer| {
        render_frame(
            renderer,
            &mut ui_tree,
            &debug_hud,
            state,
            &mut cache,
            RenderInputs {
                mouse: (u32::MAX, u32::MAX),
                world_offset_y_px: 0,
                frame_dt: Duration::ZERO,
                settings_overlay: None,
                show_round_timer: true,
            },
        )
        .record(&mut debug_hud);
    })
}

pub fn render_frame(
    renderer: &mut dyn Renderer2d,
    ui_tree: &mut UiTree,
    debug_hud: &DebugHud,
    state: &GameState,
    cache: &mut RenderCache,
    inputs: RenderInputs,
) -> DrawTimings {
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
            inputs.world_offset_y_px,
        );
        if view.is_tetris() {
            draw_tetris_hud_view(
                renderer,
                size.width,
                size.height,
                state.tetris(),
                tetris_layout,
                Some(inputs.mouse),
            );
        }
        cache.last_layout = tetris_layout;
    }

    if view.is_tetris() && inputs.show_round_timer {
        let hud_x = cache.last_layout.pause_button.x.saturating_sub(180);
        let hud_y = cache
            .last_layout
            .pause_button
            .y
            .saturating_add(6)
            .saturating_add(28);
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
    match view {
        GameView::MainMenu => {
            cache.last_main_menu =
                draw_main_menu_with_ui(renderer, size.width, size.height, ui_tree);
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
            cache.last_settings_menu = SettingsMenuLayout::default();
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
                Some(inputs.mouse),
            );
            cache.last_game_over_menu = GameOverMenuLayout::default();
            cache.last_settings_menu = SettingsMenuLayout::default();
        }
        GameView::Tetris { paused: true } => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu =
                draw_pause_menu_with_ui(renderer, size.width, size.height, ui_tree);
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
            cache.last_settings_menu = SettingsMenuLayout::default();
        }
        GameView::Tetris { paused: false } => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu = GameOverMenuLayout::default();
            cache.last_settings_menu = SettingsMenuLayout::default();
        }
        GameView::GameOver => {
            cache.last_main_menu = MainMenuLayout::default();
            cache.last_pause_menu = PauseMenuLayout::default();
            cache.last_skilltree = SkillTreeLayout::default();
            cache.last_game_over_menu =
                draw_game_over_menu_with_ui(renderer, size.width, size.height, ui_tree);
            cache.last_settings_menu = SettingsMenuLayout::default();
        }
    }
    if let Some(settings) = &inputs.settings_overlay {
        cache.last_settings_menu =
            draw_settings_menu_with_ui(renderer, size.width, size.height, ui_tree, settings);
    } else {
        cache.last_settings_menu = SettingsMenuLayout::default();
    }
    debug_hud.draw_overlay(renderer, size.width, size.height);
    let overlay_dt = overlay_start.elapsed();

    DrawTimings {
        frame_dt: inputs.frame_dt,
        board_dt,
        draw_dt,
        overlay_dt,
        frame_total_dt: frame_start.elapsed(),
    }
}
