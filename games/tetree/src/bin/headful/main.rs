use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use engine::HeadlessRunner;
use engine::app::{AppConfig, AppContext, GameApp, TimelineGame, run_game};
use engine::input::InputFrame;
use engine::launch::{RunConfig, parse_timeline_cli};
use engine::profiling::FrameTimings;
use engine::surface::SurfaceSize;
use engine::timeline::StateSchema;
use winit::event::MouseButton;
#[cfg(test)]
use winit::event::VirtualKeyCode;

use game::debug::{DebugHud, DebugHudAction};
use game::game_ui::{GameUiAction, GameUiLayout};
use game::headful::audio::GameAudio;
use game::headful::dig_camera::DigCameraController;
use game::headful::input_adapter as headful_input;
use game::headful::render_pipeline::{
    FrameOverlays, export_video, render_frame as render_headful_frame,
};
use game::headful::skilltree_camera as headful_camera;
use game::headful::view_transitions as headful_view;
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::settings::{AudioSettings, PlayerSettings, SettingsStore};
use game::sfx::{ACTION_SFX_VOLUME, GLASS_BREAK_SFX_VOLUME};
use game::skilltree::{SkillTreeEditor, SkillTreeRunMods};
use game::state::{DEFAULT_GRAVITY_INTERVAL, DEFAULT_ROUND_LIMIT, GameState, TIMELINE_SCHEMA};
use game::tetris_core::{
    DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE, DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT, Piece,
    default_depth_wall_defs,
};
use game::tetris_ui::{SettingsMenuLayout, SettingsSlider, SkillTreeLayout};
use game::view::GameView;

struct HeadfulApp {
    profile_mode: bool,
    base_logic: TetrisLogic,
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    sfx: Option<GameAudio>,
    debug_hud: DebugHud,
    skilltree_editor: SkillTreeEditor,
    skilltree_cam_input: SkillTreeCameraInput,
    horizontal_repeat: HorizontalRepeat,
    exit_requested: bool,
    consume_next_mouse_up: bool,
    settings_store: SettingsStore,
    active_settings_slider: Option<SettingsSlider>,
    settings_dirty: bool,
}

struct PreviewSnapshot {
    state: Arc<GameState>,
    timeline_paused: bool,
    debug_hud: DebugHud,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = parse_timeline_cli()?;
    if cli.help {
        print_headful_help();
        return Ok(());
    }
    if cli.check_launch {
        return Ok(());
    }

    let project = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let default_directory = engine::launch::project_timeline(project, "headful");
    if cli.uses_default_timeline() {
        engine::launch::migrate_timeline(
            &project.join("../../target/timelines/headful"),
            &default_directory,
        )?;
    }
    if let Some(output) = cli.export_video.as_ref() {
        let directory = cli.timeline_path().unwrap_or(&default_directory);
        export_video(directory, output)?;
        println!("exported {}", output.display());
        return Ok(());
    }

    let config = AppConfig {
        title: "Tetree Headful".to_string(),
        desired_size: SurfaceSize::new(1920, 1080),
        clamp_to_monitor: true,
    };

    let mut base_logic = TetrisLogic::new(0, Piece::all()).with_bottomwell(true);
    if let Some(override_hp) = env_u32("ROLLOUT_DEPTH_WALL_HP").map(|hp| hp.max(1)) {
        let defs = default_depth_wall_defs()
            .into_iter()
            .map(|mut def| {
                def.hp = override_hp;
                def
            })
            .collect();
        base_logic = base_logic.with_depth_wall_defs(defs);
    }
    let per_line_damage =
        env_u32("ROLLOUT_DEPTH_WALL_DAMAGE_PER_LINE").unwrap_or(DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE);
    let multi_bonus_percent = env_u32("ROLLOUT_DEPTH_WALL_MULTI_BONUS_PERCENT")
        .unwrap_or(DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT);
    base_logic = base_logic.with_depth_wall_damage_tuning(per_line_damage, multi_bonus_percent);
    let run = cli.run_config(default_directory)?;
    let mut app = HeadfulApp::new(base_logic, DEFAULT_ROUND_LIMIT, DEFAULT_GRAVITY_INTERVAL);
    app.profile_mode = matches!(run, RunConfig::Profile);
    run_game(config, app, run)
}

impl HeadfulApp {
    fn new(
        base_logic: TetrisLogic,
        base_round_limit: Duration,
        base_gravity_interval: Duration,
    ) -> Self {
        let settings_store = SettingsStore::from_env();
        Self {
            profile_mode: false,
            base_logic,
            base_round_limit,
            base_gravity_interval,
            sfx: GameAudio::open(),
            debug_hud: DebugHud::new(),
            skilltree_editor: SkillTreeEditor::default(),
            skilltree_cam_input: SkillTreeCameraInput::default(),
            horizontal_repeat: HorizontalRepeat::default(),
            exit_requested: false,
            consume_next_mouse_up: false,
            settings_store,
            active_settings_slider: None,
            settings_dirty: false,
        }
    }

    fn play_click_sfx(&self, state: &GameState) {
        if let Some(sfx) = self.sfx.as_ref() {
            let gain = state.settings.values.audio.effective_sfx_gain();
            sfx.play_click(ACTION_SFX_VOLUME * gain);
        }
    }

    fn apply_audio_settings(&self, audio: AudioSettings) {
        if let Some(sfx) = self.sfx.as_ref() {
            sfx.apply_audio_settings(audio);
        }
    }

    fn settings_changed(&mut self, settings: &PlayerSettings, previous_audio: AudioSettings) {
        if settings.audio != previous_audio {
            self.apply_audio_settings(settings.audio);
        }
        self.settings_dirty = true;
    }

    fn save_settings_if_dirty(&mut self, settings: &PlayerSettings) {
        if !self.settings_dirty {
            return;
        }
        match self.settings_store.save(settings) {
            Ok(()) => {
                self.settings_dirty = false;
            }
            Err(err) => {
                eprintln!("warning: failed to save settings: {err}");
            }
        }
    }

    fn apply_settings_slider_x(
        &mut self,
        settings: &mut PlayerSettings,
        layout: SettingsMenuLayout,
        slider: SettingsSlider,
        x: u32,
    ) {
        let previous_audio = settings.audio;
        if layout.set_slider_x(slider, settings, x) {
            self.settings_changed(settings, previous_audio);
        }
    }

    fn reset_active_run(&mut self, state: &mut HeadlessRunner<TetrisLogic>) {
        let mods = state.state().skilltree.run_mods();
        let tuning = run_tuning_from_mods(self.base_round_limit, self.base_gravity_interval, mods);
        state.restart(logic_for_run(&self.base_logic, mods), |logic, state| {
            let mut tetris = logic.initial_core();
            tetris.inherit_depth_wall_progress(&state.tetris);
            state.tetris = tetris;
            state.round_timer = RoundTimer::new(tuning.round_limit);
            state.gravity_interval = tuning.gravity_interval;
            state.gravity_elapsed = Duration::ZERO;
            state
                .display
                .dig_camera
                .reset(state.tetris.background_depth_rows());
            state.input = None;
        });
        self.horizontal_repeat.clear();
    }

    fn apply_input_commands(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        commands: Vec<headful_input::HeadfulInputCommand>,
    ) {
        for command in commands {
            match command {
                headful_input::HeadfulInputCommand::ToggleDebugHud => {
                    self.debug_hud.toggle();
                }
                headful_input::HeadfulInputCommand::Ui(action) => {
                    self.handle_ui_action(runner, action);
                }
                headful_input::HeadfulInputCommand::SaveSkillTree => {
                    self.skilltree_editor
                        .save(&mut runner.state_mut().skilltree);
                }
                headful_input::HeadfulInputCommand::ReloadSkillTree => {
                    self.skilltree_editor
                        .reload(&mut runner.state_mut().skilltree);
                }
                headful_input::HeadfulInputCommand::PlayClick => {
                    self.play_click_sfx(runner.state());
                }
                headful_input::HeadfulInputCommand::ApplyAction(action) => {
                    apply_action(runner, self.sfx.as_ref(), &mut self.debug_hud, action);
                }
            }
        }
    }

    fn handle_ui_action(
        &mut self,
        state: &mut HeadlessRunner<TetrisLogic>,
        action: GameUiAction,
    ) -> bool {
        let view = state.state().view;
        match action {
            GameUiAction::StartGame
                if matches!(view, GameView::MainMenu | GameView::GameOver)
                    || (view == GameView::SkillTree && !state.state().skilltree.editor.enabled) =>
            {
                let transition = headful_view::start_game(view);
                state.state_mut().view = transition.next_view;
                if transition.reset_tetris {
                    self.reset_active_run(state);
                }
            }
            GameUiAction::OpenSkillTreeEditor if view == GameView::MainMenu => {
                state.state_mut().view = headful_view::open_skilltree_editor(view).next_view;
                self.horizontal_repeat.clear();
                let skilltree = &mut state.state_mut().skilltree;
                if !skilltree.editor.enabled {
                    skilltree.editor_toggle();
                }
            }
            GameUiAction::OpenSkillTree if view == GameView::GameOver => {
                state.state_mut().view = headful_view::open_skilltree(view).next_view;
                self.horizontal_repeat.clear();
            }
            GameUiAction::Back if matches!(view, GameView::SkillTree | GameView::GameOver) => {
                let state = state.state_mut();
                if view == GameView::SkillTree && state.skilltree.editor.enabled {
                    if state.skilltree.editor.search_open {
                        state.skilltree.editor_close_search();
                        self.play_click_sfx(state);
                        return true;
                    }
                    state.skilltree.editor_toggle();
                }
                state.view = headful_view::back(view).next_view;
                self.horizontal_repeat.clear();
            }
            GameUiAction::Quit if matches!(view, GameView::MainMenu | GameView::GameOver) => {
                self.exit_requested = true;
                return true;
            }
            GameUiAction::PauseToggle if view.is_tetris() => {
                let state = state.state_mut();
                state.view = headful_view::toggle_pause(view).next_view;
                state.gravity_elapsed = Duration::ZERO;
                self.horizontal_repeat.clear();
            }
            GameUiAction::EndRun if view.is_tetris_paused() => {
                self.finish_round(state.state_mut());
            }
            GameUiAction::HoldPiece if view.is_tetris_playing() => {
                apply_action(
                    state,
                    self.sfx.as_ref(),
                    &mut self.debug_hud,
                    InputAction::Hold,
                );
                return true;
            }
            GameUiAction::SkillTreeTool(tool)
                if view == GameView::SkillTree && state.state().skilltree.editor.enabled =>
            {
                state.state_mut().skilltree.editor_set_tool(tool);
            }
            GameUiAction::OpenSettings
                if view.can_open_settings() && !state.state().settings.open =>
            {
                state.state_mut().settings.open = true;
                self.active_settings_slider = None;
            }
            GameUiAction::CloseSettings if state.state().settings.open => {
                state.state_mut().settings.open = false;
                self.active_settings_slider = None;
                self.save_settings_if_dirty(&state.state().settings.values);
            }
            settings_action => {
                return self.apply_settings_action(state.state_mut(), settings_action);
            }
        }
        self.play_click_sfx(state.state());
        true
    }

    fn apply_settings_action(&mut self, state: &mut GameState, action: GameUiAction) -> bool {
        if !state.settings.open && action != GameUiAction::ToggleMusic {
            return false;
        }
        let settings = &mut state.settings.values;
        let previous_audio = settings.audio;
        match action {
            GameUiAction::ResetSettings => *settings = PlayerSettings::default(),
            GameUiAction::ToggleMute => settings.audio.mute_all = !settings.audio.mute_all,
            GameUiAction::ToggleMusic => {
                settings.audio.music_enabled = !settings.audio.music_enabled
            }
            GameUiAction::ToggleTimer => {
                settings.gameplay.show_round_timer = !settings.gameplay.show_round_timer;
            }
            GameUiAction::ToggleAutoPause => {
                settings.gameplay.auto_pause_on_focus_loss =
                    !settings.gameplay.auto_pause_on_focus_loss;
            }
            GameUiAction::ToggleReduceMotion => {
                settings.accessibility.reduce_motion = !settings.accessibility.reduce_motion;
            }
            _ => return false,
        }
        self.settings_changed(settings, previous_audio);
        self.save_settings_if_dirty(settings);
        if state.settings.open {
            self.play_click_sfx(state);
        }
        true
    }

    fn update_round_timer_and_game_over(
        &mut self,
        state: &mut HeadlessRunner<TetrisLogic>,
        dt: Duration,
    ) {
        let state = state.state_mut();
        if self.profile_mode || state.round_timer_disabled || !state.view.is_tetris_playing() {
            return;
        }

        state.round_timer.tick_if_running(dt, true);
        if state.round_timer.is_up() {
            self.finish_round(state);
        }
    }

    fn finish_round(&mut self, state: &mut GameState) {
        let earned = headful_view::money_earned_from_run(state);
        state.skilltree.add_money(earned);
        state.view = headful_view::game_over(state.view).next_view;
        state.gravity_elapsed = Duration::ZERO;
        self.horizontal_repeat.clear();
    }

    fn apply_gravity_steps(&mut self, state: &mut HeadlessRunner<TetrisLogic>, dt: Duration) {
        let mut gravity_steps = 0usize;
        let mut gravity_step_ms = 0u32;
        let mut line_clear_dt_ms = 0u32;
        {
            let state = state.state_mut();
            if state.view.is_tetris_playing() {
                if state.tetris.is_line_clear_active() {
                    // Drive clear animation by frame dt so clear timing does not
                    // quantize to the gravity interval.
                    line_clear_dt_ms = duration_to_ms_u32(dt);
                } else {
                    gravity_step_ms = duration_to_ms_u32(state.gravity_interval);
                    state.gravity_elapsed = state.gravity_elapsed.saturating_add(dt);
                    while state.gravity_elapsed >= state.gravity_interval {
                        state.gravity_elapsed =
                            state.gravity_elapsed.saturating_sub(state.gravity_interval);
                        gravity_steps = gravity_steps.saturating_add(1);
                    }
                }
            } else {
                state.gravity_elapsed = Duration::ZERO;
            }
        }
        if line_clear_dt_ms > 0 {
            state.step(InputAction::GravityTick {
                dt_ms: line_clear_dt_ms,
            });
            return;
        }
        for _ in 0..gravity_steps {
            state.step(InputAction::GravityTick {
                dt_ms: gravity_step_ms,
            });
        }
    }

    #[cfg(test)]
    fn sync_horizontal_repeat_from_frame(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        input: &InputFrame,
        now: Instant,
    ) {
        let mut immediate_actions = Vec::new();
        headful_input::sync_horizontal_repeat_from_frame(
            input,
            &mut self.horizontal_repeat,
            now,
            |action| immediate_actions.push(action),
        );
        for action in immediate_actions {
            apply_action(runner, self.sfx.as_ref(), &mut self.debug_hud, action);
        }
    }

    fn process_keyboard_frame(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        input: &InputFrame,
        now: Instant,
        skilltree_layout: SkillTreeLayout,
    ) {
        let commands = headful_input::process_keyboard_frame(
            runner,
            input,
            now,
            &mut self.horizontal_repeat,
            skilltree_layout,
            &mut self.skilltree_editor,
        );
        self.apply_input_commands(runner, commands);
    }

    fn update_skilltree_drag_from_frame(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        layout: SkillTreeLayout,
        pointer: (u32, u32),
    ) {
        headful_camera::update_drag_from_frame(
            &mut runner.state_mut().skilltree,
            layout,
            &mut self.skilltree_cam_input,
            pointer,
        );
    }
}

impl GameApp for HeadfulApp {
    type State = HeadlessRunner<TetrisLogic>;
    const LOGICAL_DISPLAY: bool = true;

    fn init_state(&mut self, ctx: &mut AppContext) -> Self::State {
        let mut runner = HeadlessRunner::new(self.base_logic.clone());
        let state = runner.state_mut();
        state.view = if self.profile_mode {
            GameView::Tetris { paused: false }
        } else {
            GameView::MainMenu
        };
        state.skilltree = self.skilltree_editor.load();
        if let Some(warning) = state.skilltree.load_warning_message() {
            self.debug_hud.log_warning(warning.to_string());
            eprintln!("warning: skilltree load issue: {warning}");
        }
        state.round_timer = RoundTimer::new(self.base_round_limit);
        state.gravity_interval = self.base_gravity_interval;
        state.gravity_elapsed = Duration::ZERO;
        state.display.viewport = ctx.display().logical_size;
        state.display.dig_camera = DigCameraController::from_env();
        state
            .display
            .dig_camera
            .reset(state.tetris.background_depth_rows());
        state.round_timer_disabled = env_bool("ROLLOUT_DEBUG_DISABLE_ROUND_TIMER").unwrap_or(false);
        state.settings.values = self.settings_store.load();
        self.apply_audio_settings(state.settings.values.audio);
        runner
    }

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: &InputFrame,
        dt: Duration,
        ctx: &mut AppContext,
    ) {
        let now = Instant::now();

        if !input.window_focused {
            self.horizontal_repeat.clear();
            self.skilltree_cam_input = SkillTreeCameraInput::default();
            self.active_settings_slider = None;
            self.consume_next_mouse_up = false;
            if state
                .state()
                .settings
                .values
                .gameplay
                .auto_pause_on_focus_loss
                && matches!(state.state().view, GameView::Tetris { paused: false })
            {
                let state = state.state_mut();
                state.view = GameView::Tetris { paused: true };
                state.gravity_elapsed = Duration::ZERO;
            }
        }

        let left_mouse_pressed = input.mouse_buttons_pressed.contains(&MouseButton::Left);
        let left_mouse_released = input.mouse_buttons_released.contains(&MouseButton::Left);
        let left_mouse_down = input.mouse_buttons_down.contains(&MouseButton::Left);
        let pointer = input.mouse_pos.filter(|_| input.window_focused);

        let size = ctx.display().logical_size;
        let keyboard_layout = if input.keys_pressed.is_empty() {
            SkillTreeLayout::default()
        } else {
            GameUiLayout::compute_skilltree(state.state(), size)
        };
        self.process_keyboard_frame(state, input, now, keyboard_layout);
        let layout = GameUiLayout::compute(state.state(), size);

        if left_mouse_pressed && let Some((x, y)) = pointer {
            self.consume_next_mouse_up = false;
            self.active_settings_slider = None;
            if let Some(action) = self.debug_hud.action_at(
                x,
                y,
                size.width,
                size.height,
                state.state().round_timer_disabled,
            ) {
                match action {
                    DebugHudAction::ToggleMinimized => self.debug_hud.toggle_minimized(),
                    DebugHudAction::ToggleRoundTimer => {
                        let state = state.state_mut();
                        state.round_timer_disabled = !state.round_timer_disabled;
                    }
                }
                self.consume_next_mouse_up = true;
            } else if let Some(settings) = layout.settings {
                self.active_settings_slider = settings.slider_at(x, y);
                if let Some(slider) = self.active_settings_slider {
                    self.apply_settings_slider_x(
                        &mut state.state_mut().settings.values,
                        settings,
                        slider,
                        x,
                    );
                }
            } else if state.state().view == GameView::SkillTree {
                self.skilltree_cam_input = SkillTreeCameraInput {
                    left_down: true,
                    drag_started: false,
                    drag_started_in_view: headful_camera::skilltree_grid_viewport(layout.skilltree)
                        .is_some_and(|grid| grid.contains(x, y)),
                    down_x: x,
                    down_y: y,
                    last_x: x,
                    last_y: y,
                };
            }
        }

        if left_mouse_down && let Some((x, y)) = pointer {
            if let Some(settings) = layout.settings {
                if !left_mouse_pressed && let Some(slider) = self.active_settings_slider {
                    self.apply_settings_slider_x(
                        &mut state.state_mut().settings.values,
                        settings,
                        slider,
                        x,
                    );
                }
            } else {
                self.update_skilltree_drag_from_frame(state, layout.skilltree, (x, y));
            }
        }

        let released_drag = left_mouse_released && self.skilltree_cam_input.drag_started;
        if left_mouse_released || !left_mouse_down {
            self.skilltree_cam_input = SkillTreeCameraInput::default();
            self.active_settings_slider = None;
        }

        let view = state.state().view;
        if view.is_tetris_playing() {
            if let Some(action) = self.horizontal_repeat.next_repeat_action(now) {
                apply_action(state, self.sfx.as_ref(), &mut self.debug_hud, action);
            }
        }

        if left_mouse_released
            && !std::mem::take(&mut self.consume_next_mouse_up)
            && let Some(pointer) = pointer
        {
            let handled = layout
                .action_at(state.state(), pointer)
                .is_some_and(|action| self.handle_ui_action(state, action));
            if !handled && !state.state().settings.open {
                let commands = headful_input::handle_skilltree_world_click(
                    state,
                    layout.skilltree,
                    pointer.0,
                    pointer.1,
                    released_drag,
                    &mut self.skilltree_editor,
                );
                self.apply_input_commands(state, commands);
            }
        }
        if left_mouse_released && state.state().settings.open {
            self.save_settings_if_dirty(&state.state().settings.values);
        }

        self.update_round_timer_and_game_over(state, dt);
        self.apply_gravity_steps(state, dt);

        let view = state.state().view;
        if matches!(view, GameView::SkillTree) {
            let skilltree = &mut state.state_mut().skilltree;
            if let Some((x, y)) = pointer {
                headful_camera::apply_wheel_zoom(skilltree, layout.skilltree, x, y, input.scroll.1);
                headful_camera::apply_edge_pan(
                    skilltree,
                    layout.skilltree,
                    x,
                    y,
                    dt,
                    self.skilltree_cam_input.left_down,
                );
            }
            headful_camera::finalize_camera(skilltree, layout.skilltree);
        }

        let state = state.state_mut();
        update_dig_camera(state, dt);
        state.display.viewport = size;
        state.input = Some(input.clone());
    }

    fn render(
        &mut self,
        runner: &Self::State,
        timeline_paused: bool,
        renderer: &mut dyn engine::graphics::Renderer2d,
    ) {
        render_headful_frame(
            renderer,
            runner.state(),
            FrameOverlays {
                debug_hud: Some(&self.debug_hud),
                playback_cursor: timeline_paused,
            },
        );
    }

    fn on_frame_timings(&mut self, timings: FrameTimings) {
        self.debug_hud.on_frame(timings);
    }

    fn master_volume(&self, state: &Self::State) -> Option<u8> {
        Some((state.state().settings.values.audio.master_volume * 100.0).round() as u8)
    }

    fn set_master_volume(&mut self, state: &mut Self::State, percent: u8, commit: bool) {
        let settings = &mut state.state_mut().settings.values;
        let previous_audio = settings.audio;
        let volume = f32::from(percent.min(100)) / 100.0;
        if settings.audio.master_volume != volume {
            settings.audio.master_volume = volume;
            self.settings_changed(settings, previous_audio);
        }
        if commit {
            self.save_settings_if_dirty(settings);
        }
    }

    fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    fn shutdown(&mut self, state: &Self::State) {
        self.save_settings_if_dirty(&state.state().settings.values);
    }
}

impl TimelineGame for HeadfulApp {
    type Snapshot = GameState;
    type PreviewSnapshot = PreviewSnapshot;
    const SCHEMA: StateSchema = TIMELINE_SCHEMA;

    fn preview_snapshot(&self, state: Arc<GameState>, timeline_paused: bool) -> PreviewSnapshot {
        PreviewSnapshot {
            state,
            timeline_paused,
            debug_hud: self.debug_hud.clone(),
        }
    }

    fn draw_preview(snapshot: PreviewSnapshot, renderer: &mut dyn engine::graphics::Renderer2d) {
        render_headful_frame(
            renderer,
            &snapshot.state,
            FrameOverlays {
                debug_hud: Some(&snapshot.debug_hud),
                playback_cursor: snapshot.timeline_paused,
            },
        );
    }

    fn snapshot(state: &Self::State) -> &Self::Snapshot {
        state.state()
    }

    fn restore_state(&mut self, snapshot: Self::Snapshot) -> Self::State {
        self.skilltree_editor.on_restore();
        self.apply_audio_settings(snapshot.settings.values.audio);
        self.settings_dirty = false;
        let logic = logic_for_run(&self.base_logic, snapshot.skilltree.run_mods());
        let runner = HeadlessRunner::from_state(logic, snapshot);
        self.horizontal_repeat.clear();
        self.skilltree_cam_input = SkillTreeCameraInput::default();
        self.active_settings_slider = None;
        self.consume_next_mouse_up = false;
        runner
    }
}

fn update_dig_camera(state: &mut GameState, dt: Duration) {
    let paused = matches!(state.view, GameView::Tetris { paused: true });
    state
        .display
        .dig_camera
        .update(state.tetris.background_depth_rows(), dt, paused);
}

type SkillTreeCameraInput = headful_camera::SkillTreeCameraInput;

#[derive(Debug, Clone, Copy)]
struct RunTuning {
    round_limit: Duration,
    gravity_interval: Duration,
}

fn gravity_interval_from_percent(base: Duration, faster_percent: u32) -> Duration {
    // Reduce the interval by `faster_percent` (e.g. 10% => 500ms -> 450ms). Clamp to avoid zero.
    let pct = faster_percent.min(95) as u64;
    let base_ms = base.as_millis() as u64;
    let ms = base_ms.saturating_mul(100u64.saturating_sub(pct)) / 100;
    Duration::from_millis(ms.max(25))
}

fn duration_to_ms_u32(duration: Duration) -> u32 {
    duration.as_millis().min(u128::from(u32::MAX)) as u32
}

fn run_tuning_from_mods(
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    mods: SkillTreeRunMods,
) -> RunTuning {
    RunTuning {
        round_limit: base_round_limit
            .saturating_add(Duration::from_secs(mods.extra_round_time_seconds as u64)),
        gravity_interval: gravity_interval_from_percent(
            base_gravity_interval,
            mods.gravity_faster_percent,
        ),
    }
}

fn logic_for_run(base_logic: &TetrisLogic, mods: SkillTreeRunMods) -> TetrisLogic {
    base_logic
        .clone()
        .with_score_bonus_per_line(mods.score_bonus_per_line)
        .with_bottomwell_run_mods(mods.bottomwell)
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.parse::<u32>().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|v| match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

fn print_headful_help() {
    // go.sh is the primary control surface, but `cargo run -p tetree --bin headful -- --help`
    // should still be self-explanatory.
    println!(
        r#"Tetree Headful

Usage:
  headful [--timeline DIR] [--fresh | --playback]
  headful --no-recording
  headful --export-video PATH [--timeline DIR]
  headful --profile

Flags:
  --timeline DIR    Select a timeline (default: games/tetree/.sycho/timelines/headful).
  --fresh           Start a separate timeline. With --timeline, DIR must be new.
  --playback        Play a saved timeline from its first snapshot.
  --no-recording    Run standalone without opening a timeline.
  --profile         Profile the standalone game and write a trace to target/perf_traces/.
  --export-video PATH
                    Render a saved timeline to MP4 without opening a window.
  --help, -h        Show this help.
"#
    );
}
#[cfg(test)]
type HorizontalDir = headful_input::HorizontalDir;
type HorizontalRepeat = headful_input::HorizontalRepeat;

#[cfg(test)]
fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    headful_input::map_key_to_action(key)
}

fn should_play_action_sfx(action: InputAction) -> bool {
    headful_input::should_play_action_sfx(action)
}

fn apply_action(
    runner: &mut HeadlessRunner<TetrisLogic>,
    sfx: Option<&GameAudio>,
    debug_hud: &mut DebugHud,
    action: InputAction,
) {
    let before_glass_shatters = runner.state().tetris.glass_shatter_count();

    runner.step(action);

    if let Some(sfx) = sfx {
        let audio = runner.state().settings.values.audio;
        if should_play_action_sfx(action) {
            sfx.play_click(ACTION_SFX_VOLUME * audio.effective_sfx_gain());
        }
        let after_glass_shatters = runner.state().tetris.glass_shatter_count();
        if after_glass_shatters > before_glass_shatters {
            sfx.play_glass_break(GLASS_BREAK_SFX_VOLUME * audio.effective_sfx_gain());
        }
    }

    debug_hud.mark_input();
}

#[cfg(test)]
mod headful_tests;
