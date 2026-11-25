use std::{
    cell::Cell,
    io,
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant},
};

use engine::HeadlessRunner;
use engine::app::{
    AppConfig, AppContext, GameApp, InputFrame, ProfileConfig, RunMode, TimelineCli,
    TimelineConfig, TimelineGame, parse_timeline_cli, run_game_with_profile,
    run_game_with_timeline,
};
use engine::audio::{MusicRuntime, Quantize, Scene, StepPattern, Track, Waveform};
use engine::timeline::{StateSchema, TimelineCommand, session_directory};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use winit::event::VirtualKeyCode;
use winit::{
    dpi::PhysicalSize,
    event::{Event, MouseButton, WindowEvent},
    event_loop::ControlFlow,
};

use game::debug::DebugHud;
use game::game_ui::{GameUiAction, GameUiLayout};
use game::headful::dig_camera as headful_dig_camera;
use game::headful::input_adapter as headful_input;
use game::headful::render_pipeline::{
    DrawTimings, RenderInputs, export_video, render_frame as render_headful_frame,
};
use game::headful::skilltree_camera as headful_camera;
use game::headful::view_transitions as headful_view;
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::settings::{AudioSettings, PlayerSettings, SettingsStore};
use game::sfx::{ACTION_SFX_VOLUME, GLASS_BREAK_SFX_VOLUME, MUSIC_VOLUME};
use game::skilltree::{SkillTreeRunMods, SkillTreeRuntime};
use game::state::{
    DEFAULT_GRAVITY_INTERVAL, DEFAULT_ROUND_LIMIT, GameState, RecordedInput, TIMELINE_SCHEMA,
};
use game::tetris_core::{
    BottomwellRunMods, DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE,
    DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT, Piece, default_depth_wall_defs,
};
use game::tetris_ui::{Rect, SettingsMenuLayout, SkillTreeLayout};
use game::view::GameView;

struct HeadfulApp {
    profile_mode: bool,
    timeline_mode: bool,
    base_logic: TetrisLogic,
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    sfx: Option<Sfx>,
    debug_hud: DebugHud,
    historical: bool,
    skilltree_layout: SkillTreeLayout,
    mouse_x: u32,
    mouse_y: u32,
    skilltree_cam_input: SkillTreeCameraInput,
    horizontal_repeat: HorizontalRepeat,
    dig_camera: DigCameraController,
    frame_interval: Duration,
    next_redraw: Instant,
    last_frame_dt: Duration,
    exit_requested: bool,
    mouse_release_was_drag: bool,
    consume_next_mouse_up: bool,
    settings_store: SettingsStore,
    player_settings: PlayerSettings,
    settings_open: bool,
    active_settings_slider: Option<ActiveSettingsSlider>,
    settings_dirty: bool,
}

struct PreviewSnapshot {
    state: GameState,
    inputs: RenderInputs,
    debug_hud: DebugHud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSettingsSlider {
    Master,
    Music,
    Sfx,
    ScreenShake,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let TimelineCli {
        help,
        timeline_path,
        fresh,
        playback,
        export_video: output,
    } = parse_timeline_cli()?;
    if help {
        print_headful_help();
        return Ok(());
    }

    let default_directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/timelines/headful");
    if let Some(output) = output {
        let directory = timeline_path.unwrap_or(default_directory);
        export_video(&directory, &output)?;
        println!("exported {}", output.display());
        return Ok(());
    }

    let profile_frames = env_usize("ROLLOUT_HEADFUL_PROFILE_FRAMES").unwrap_or(0);
    if (timeline_path.is_some() || fresh || playback) && profile_frames > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --timeline, --fresh, or --playback with profiling mode",
        )
        .into());
    }
    let desired = if let (Some(w), Some(h)) = (
        env_u32("ROLLOUT_HEADFUL_PROFILE_WIDTH"),
        env_u32("ROLLOUT_HEADFUL_PROFILE_HEIGHT"),
    ) {
        PhysicalSize::new(w.max(1), h.max(1))
    } else {
        PhysicalSize::new(1920u32, 1080u32)
    };

    let config = AppConfig {
        title: "Tetree Headful".to_string(),
        desired_size: desired,
        clamp_to_monitor: true,
        vsync: env_bool("ROLLOUT_HEADFUL_VSYNC"),
        present_mode: env_present_mode("ROLLOUT_HEADFUL_PRESENT_MODE"),
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
    let app = HeadfulApp::new(base_logic, DEFAULT_ROUND_LIMIT, DEFAULT_GRAVITY_INTERVAL);

    if profile_frames > 0 {
        run_game_with_profile(
            config,
            app,
            ProfileConfig {
                target_frames: profile_frames,
            },
        )
    } else {
        let directory = session_directory(default_directory, timeline_path, fresh)?;
        run_game_with_timeline(
            config,
            app,
            TimelineConfig {
                directory,
                playback,
            },
        )
    }
}

impl HeadfulApp {
    fn new(
        base_logic: TetrisLogic,
        base_round_limit: Duration,
        base_gravity_interval: Duration,
    ) -> Self {
        let settings_store = SettingsStore::from_env();
        let player_settings = settings_store.load();
        let sfx = match Sfx::new() {
            Ok(sfx) => Some(sfx),
            Err(err) => {
                eprintln!("warning: audio disabled: {err}");
                if is_running_in_wsl() {
                    eprintln!(
                        "hint: in WSL install `libasound2-plugins pulseaudio-utils alsa-utils` so ALSA can route to WSLg PulseAudio"
                    );
                }
                None
            }
        };
        let frame_interval = Duration::from_secs_f64(1.0 / 60.0);
        let mut debug_hud = DebugHud::new();
        if env_bool("ROLLOUT_DEBUG_DISABLE_ROUND_TIMER").unwrap_or(false) {
            debug_hud.set_round_timer_disabled(true);
        }
        let app = Self {
            profile_mode: false,
            timeline_mode: false,
            base_logic,
            base_round_limit,
            base_gravity_interval,
            sfx,
            debug_hud,
            historical: false,
            skilltree_layout: SkillTreeLayout::default(),
            mouse_x: 0,
            mouse_y: 0,
            skilltree_cam_input: SkillTreeCameraInput::default(),
            horizontal_repeat: HorizontalRepeat::default(),
            dig_camera: DigCameraController::from_env(),
            frame_interval,
            next_redraw: Instant::now(),
            last_frame_dt: Duration::ZERO,
            exit_requested: false,
            mouse_release_was_drag: false,
            consume_next_mouse_up: false,
            settings_store,
            player_settings,
            settings_open: false,
            active_settings_slider: None,
            settings_dirty: false,
        };
        app.apply_audio_settings();
        app
    }

    fn render_inputs(&self) -> RenderInputs {
        let camera_offset = if self.player_settings.accessibility.reduce_motion {
            0.0
        } else {
            self.dig_camera.offset_y_px()
                * (self.player_settings.video.clamped_screen_shake() as f32 / 100.0)
        };
        RenderInputs {
            world_offset_y_px: camera_offset.round() as i32,
            frame_dt: self.last_frame_dt,
            historical: self.historical,
            show_round_timer: self.player_settings.gameplay.show_round_timer,
        }
    }

    fn play_click_sfx(&self) {
        if let Some(sfx) = self.sfx.as_ref() {
            let gain = self.player_settings.audio.effective_sfx_gain();
            sfx.play_click(ACTION_SFX_VOLUME * gain);
        }
    }

    fn apply_audio_settings(&self) {
        if let Some(sfx) = self.sfx.as_ref() {
            sfx.apply_audio_settings(self.player_settings.audio);
        }
    }

    fn mark_settings_dirty(&mut self) {
        self.settings_dirty = true;
    }

    fn save_settings_if_dirty(&mut self) {
        if !self.settings_dirty {
            return;
        }
        self.player_settings = self.player_settings.clone().sanitized();
        match self.settings_store.save(&self.player_settings) {
            Ok(()) => {
                self.settings_dirty = false;
            }
            Err(err) => {
                eprintln!("warning: failed to save settings: {err}");
            }
        }
    }

    fn open_settings_from_view(&mut self, view: GameView) -> bool {
        if !matches!(view, GameView::MainMenu | GameView::Tetris { paused: true }) {
            return false;
        }
        self.settings_open = true;
        self.active_settings_slider = None;
        true
    }

    fn close_settings(&mut self) {
        self.settings_open = false;
        self.active_settings_slider = None;
        self.save_settings_if_dirty();
    }

    fn toggle_music_enabled(&mut self) {
        self.player_settings.audio.music_enabled = !self.player_settings.audio.music_enabled;
        self.apply_audio_settings();
        self.mark_settings_dirty();
    }

    fn settings_slider_from_pointer(
        layout: SettingsMenuLayout,
        x: u32,
        y: u32,
    ) -> Option<ActiveSettingsSlider> {
        if layout.master_track.contains(x, y) {
            Some(ActiveSettingsSlider::Master)
        } else if layout.music_track.contains(x, y) {
            Some(ActiveSettingsSlider::Music)
        } else if layout.sfx_track.contains(x, y) {
            Some(ActiveSettingsSlider::Sfx)
        } else if layout.shake_track.contains(x, y) {
            Some(ActiveSettingsSlider::ScreenShake)
        } else {
            None
        }
    }

    fn apply_settings_slider_x(
        &mut self,
        layout: SettingsMenuLayout,
        slider: ActiveSettingsSlider,
        x: u32,
    ) {
        let mut changed = false;
        match slider {
            ActiveSettingsSlider::Master => {
                let slider = engine::slider::Slider::new(
                    layout.master_track,
                    0.0,
                    1.0,
                    self.player_settings.audio.master_volume,
                );
                let next = slider.value_from_x(x);
                if (next - self.player_settings.audio.master_volume).abs() > 1e-4 {
                    self.player_settings.audio.master_volume = next;
                    changed = true;
                }
            }
            ActiveSettingsSlider::Music => {
                let slider = engine::slider::Slider::new(
                    layout.music_track,
                    0.0,
                    1.0,
                    self.player_settings.audio.music_volume,
                );
                let next = slider.value_from_x(x);
                if (next - self.player_settings.audio.music_volume).abs() > 1e-4 {
                    self.player_settings.audio.music_volume = next;
                    changed = true;
                }
            }
            ActiveSettingsSlider::Sfx => {
                let slider = engine::slider::Slider::new(
                    layout.sfx_track,
                    0.0,
                    1.0,
                    self.player_settings.audio.sfx_volume,
                );
                let next = slider.value_from_x(x);
                if (next - self.player_settings.audio.sfx_volume).abs() > 1e-4 {
                    self.player_settings.audio.sfx_volume = next;
                    changed = true;
                }
            }
            ActiveSettingsSlider::ScreenShake => {
                let slider = engine::slider::Slider::new(
                    layout.shake_track,
                    0.0,
                    100.0,
                    self.player_settings.video.screen_shake_percent as f32,
                );
                let next = slider.value_from_x(x).round().clamp(0.0, 100.0) as u8;
                if next != self.player_settings.video.screen_shake_percent {
                    self.player_settings.video.screen_shake_percent = next;
                    changed = true;
                }
            }
        }
        if changed {
            self.apply_audio_settings();
            self.mark_settings_dirty();
        }
    }

    fn reset_active_run(&mut self, state: &mut HeadlessRunner<TetrisLogic>) {
        reset_run(
            state,
            &self.base_logic,
            self.base_round_limit,
            self.base_gravity_interval,
            &mut self.horizontal_repeat,
            self.timeline_mode,
        );
        self.dig_camera
            .reset(state.state().tetris.background_depth_rows());
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
                headful_input::HeadfulInputCommand::ToggleMusic => {
                    self.toggle_music_enabled();
                }
                headful_input::HeadfulInputCommand::ExitRequested => {
                    self.exit_requested = true;
                }
                headful_input::HeadfulInputCommand::PlayClick => {
                    self.play_click_sfx();
                }
                headful_input::HeadfulInputCommand::ResetRun => {
                    self.reset_active_run(runner);
                }
                headful_input::HeadfulInputCommand::ApplyAction(action) => {
                    apply_action(
                        runner,
                        self.sfx.as_ref(),
                        self.player_settings.audio,
                        &mut self.debug_hud,
                        action,
                    );
                }
            }
        }
    }

    fn handle_ui_action(
        &mut self,
        state: &mut HeadlessRunner<TetrisLogic>,
        action: GameUiAction,
    ) -> bool {
        match action {
            GameUiAction::StartGame => {
                let view = state.state().view;
                if matches!(view, GameView::MainMenu)
                    || (view == GameView::SkillTree && !state.state().skilltree.editor.enabled)
                {
                    let transition = headful_view::start_game(view);
                    state.state_mut().view = transition.next_view;
                    if transition.reset_tetris {
                        self.reset_active_run(state);
                    }
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::OpenSkillTreeEditor => {
                let view = state.state().view;
                if matches!(view, GameView::MainMenu) {
                    let transition = headful_view::open_skilltree_editor(view);
                    state.state_mut().view = transition.next_view;
                    self.horizontal_repeat.clear();
                    let skilltree = &mut state.state_mut().skilltree;
                    if !skilltree.editor.enabled {
                        skilltree.editor_toggle();
                    }
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::Quit => {
                let view = state.state().view;
                if matches!(view, GameView::MainMenu | GameView::GameOver) {
                    self.exit_requested = true;
                    return true;
                }
            }
            GameUiAction::PauseToggle => {
                let view = state.state().view;
                if view.is_tetris() {
                    let transition = headful_view::toggle_pause(view);
                    let state = state.state_mut();
                    state.view = transition.next_view;
                    state.gravity_elapsed = Duration::ZERO;
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::Resume => {
                let view = state.state().view;
                if matches!(view, GameView::Tetris { paused: true }) {
                    let transition = headful_view::toggle_pause(view);
                    let state = state.state_mut();
                    state.view = transition.next_view;
                    state.gravity_elapsed = Duration::ZERO;
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::EndRun => {
                let view = state.state().view;
                if matches!(view, GameView::Tetris { paused: true }) {
                    let earned = money_earned_from_run(state.state());
                    if earned > 0 {
                        state.state_mut().skilltree.add_money(earned);
                    }
                    let transition = headful_view::game_over(view);
                    state.state_mut().view = transition.next_view;
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::HoldPiece => {
                let view = state.state().view;
                if matches!(view, GameView::Tetris { paused: false }) {
                    apply_action(
                        state,
                        self.sfx.as_ref(),
                        self.player_settings.audio,
                        &mut self.debug_hud,
                        InputAction::Hold,
                    );
                    return true;
                }
            }
            GameUiAction::Restart => {
                let view = state.state().view;
                if matches!(view, GameView::GameOver) {
                    let transition = headful_view::start_game(view);
                    state.state_mut().view = transition.next_view;
                    if transition.reset_tetris {
                        self.reset_active_run(state);
                    }
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::OpenSkillTree => {
                let view = state.state().view;
                if matches!(view, GameView::GameOver) {
                    let transition = headful_view::open_skilltree(view);
                    state.state_mut().view = transition.next_view;
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::SkillTreeTool(tool) => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if view == GameView::SkillTree && skilltree.editor.enabled {
                    skilltree.editor_set_tool(tool);
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::OpenSettings => {
                if self.open_settings_from_view(state.state().view) {
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::CloseSettings => {
                self.close_settings();
                self.play_click_sfx();
                return true;
            }
            settings_action => return self.apply_settings_action(settings_action),
        }

        false
    }

    fn apply_settings_action(&mut self, action: GameUiAction) -> bool {
        let settings = &mut self.player_settings;
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
            GameUiAction::ToggleHighContrast => {
                settings.accessibility.high_contrast_ui = !settings.accessibility.high_contrast_ui;
            }
            GameUiAction::ToggleReduceMotion => {
                settings.accessibility.reduce_motion = !settings.accessibility.reduce_motion;
            }
            _ => return false,
        }
        if matches!(
            action,
            GameUiAction::ResetSettings | GameUiAction::ToggleMute | GameUiAction::ToggleMusic
        ) {
            self.apply_audio_settings();
        }
        self.mark_settings_dirty();
        self.save_settings_if_dirty();
        self.play_click_sfx();
        true
    }

    fn update_round_timer_and_game_over(
        &mut self,
        state: &mut HeadlessRunner<TetrisLogic>,
        dt: Duration,
    ) {
        if self.profile_mode {
            return;
        }

        let mut trigger_game_over = false;
        {
            let state = state.state_mut();
            if !self.debug_hud.round_timer_disabled() {
                state
                    .round_timer
                    .tick_if_running(dt, state.view.is_tetris_playing());
            }
            if !self.debug_hud.round_timer_disabled()
                && state.view.is_tetris_playing()
                && state.round_timer.is_up()
            {
                let earned = money_earned_from_run(state);
                if earned > 0 {
                    state.skilltree.add_money(earned);
                }
                let transition = headful_view::game_over(state.view);
                state.view = transition.next_view;
                state.gravity_elapsed = Duration::ZERO;
                trigger_game_over = true;
            }
        }
        if trigger_game_over {
            self.horizontal_repeat.clear();
        }
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
            let gravity_start = Instant::now();
            state.step_profiled(
                InputAction::GravityTick {
                    dt_ms: line_clear_dt_ms,
                },
                &mut self.debug_hud,
            );
            self.debug_hud.record_gravity(gravity_start.elapsed());
            return;
        }
        for _ in 0..gravity_steps {
            let gravity_start = Instant::now();
            state.step_profiled(
                InputAction::GravityTick {
                    dt_ms: gravity_step_ms,
                },
                &mut self.debug_hud,
            );
            self.debug_hud.record_gravity(gravity_start.elapsed());
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
            apply_action(
                runner,
                self.sfx.as_ref(),
                self.player_settings.audio,
                &mut self.debug_hud,
                action,
            );
        }
    }

    fn process_keyboard_frame(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        input: &InputFrame,
        now: Instant,
    ) {
        let commands = headful_input::process_keyboard_frame(
            runner,
            input,
            now,
            &mut self.horizontal_repeat,
            self.skilltree_layout,
            self.mouse_x,
            self.mouse_y,
        );
        self.apply_input_commands(runner, commands);
    }

    fn update_skilltree_drag_from_frame(
        &mut self,
        runner: &mut HeadlessRunner<TetrisLogic>,
        left_mouse_down: bool,
    ) {
        let in_skilltree_view = matches!(runner.state().view, GameView::SkillTree);
        headful_camera::update_drag_from_frame(
            &mut runner.state_mut().skilltree,
            self.skilltree_layout,
            &mut self.skilltree_cam_input,
            self.mouse_x,
            self.mouse_y,
            left_mouse_down,
            in_skilltree_view,
        );
    }

    fn update_dig_camera_state(&mut self, runner: &HeadlessRunner<TetrisLogic>, dt: Duration) {
        let view = runner.state().view;
        let paused = matches!(view, GameView::Tetris { paused: true });
        let depth_rows = runner.state().tetris.background_depth_rows();
        self.dig_camera.update(depth_rows, dt, paused);
    }
}

impl GameApp for HeadfulApp {
    type State = HeadlessRunner<TetrisLogic>;

    fn init_state(&mut self, _ctx: &mut AppContext) -> Self::State {
        let mut runner = HeadlessRunner::new(self.base_logic.clone());
        let state = runner.state_mut();
        state.view = if self.profile_mode {
            GameView::Tetris { paused: false }
        } else {
            GameView::MainMenu
        };
        state.skilltree = SkillTreeRuntime::load_default();
        if let Some(warning) = state.skilltree.load_warning_message() {
            self.debug_hud.log_warning(warning.to_string());
            eprintln!("warning: skilltree load issue: {warning}");
        }
        state.round_timer = RoundTimer::new(self.base_round_limit);
        state.gravity_interval = self.base_gravity_interval;
        state.gravity_elapsed = Duration::ZERO;
        self.dig_camera.reset(state.tetris.background_depth_rows());
        runner
    }

    fn on_run_mode(&mut self, mode: RunMode, state: &mut Self::State, _ctx: &mut AppContext) {
        self.profile_mode = matches!(mode, RunMode::Profile);
        self.timeline_mode = matches!(mode, RunMode::Timeline);
        if self.timeline_mode {
            state.state_mut().use_timeline_progress();
        }
        if self.profile_mode {
            state.state_mut().view = GameView::Tetris { paused: false };
        }
    }

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: &InputFrame,
        dt: Duration,
        ctx: &mut AppContext,
    ) {
        self.last_frame_dt = dt;
        let now = Instant::now();

        if let Some((mx, my)) = input.mouse_pos {
            self.mouse_x = mx;
            self.mouse_y = my;
        }
        if !input.window_focused {
            self.horizontal_repeat.clear();
            self.skilltree_cam_input.left_down = false;
            self.skilltree_cam_input.drag_started = false;
            self.skilltree_cam_input.drag_started_in_view = false;
            if self.player_settings.gameplay.auto_pause_on_focus_loss
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
        let pressed = |key| input.keys_pressed.contains(&key);

        let settings_were_open = self.settings_open;
        if settings_were_open {
            if pressed(winit::event::VirtualKeyCode::Escape) {
                self.close_settings();
                self.play_click_sfx();
            }
        } else if pressed(winit::event::VirtualKeyCode::O) {
            if self.open_settings_from_view(state.state().view) {
                self.play_click_sfx();
            }
        }

        let size = ctx.surface_size;
        if !self.settings_open && !settings_were_open {
            if state.state().view == GameView::SkillTree {
                self.skilltree_layout = GameUiLayout::compute(state.state(), size, false).skilltree;
            }
            self.process_keyboard_frame(state, input, now);
        }
        let layout = GameUiLayout::compute(state.state(), size, self.settings_open);
        self.skilltree_layout = layout.skilltree;

        if left_mouse_pressed {
            let debug_clicked =
                self.debug_hud
                    .handle_click(self.mouse_x, self.mouse_y, size.width, size.height);
            if debug_clicked {
                self.consume_next_mouse_up = true;
            } else if let Some(settings) = layout.settings {
                self.active_settings_slider =
                    Self::settings_slider_from_pointer(settings, self.mouse_x, self.mouse_y);
                if let Some(slider) = self.active_settings_slider {
                    self.apply_settings_slider_x(settings, slider, self.mouse_x);
                }
            } else if state.state().view == GameView::SkillTree {
                self.skilltree_cam_input.left_down = true;
                self.skilltree_cam_input.drag_started = false;
                self.skilltree_cam_input.drag_started_in_view =
                    skilltree_grid_viewport(self.skilltree_layout)
                        .map(|r| r.contains(self.mouse_x, self.mouse_y))
                        .unwrap_or(false);
                self.skilltree_cam_input.down_x = self.mouse_x;
                self.skilltree_cam_input.down_y = self.mouse_y;
                self.skilltree_cam_input.last_x = self.mouse_x;
                self.skilltree_cam_input.last_y = self.mouse_y;
            }
        }

        if let Some(settings) = layout.settings {
            if left_mouse_down {
                if let Some(active) = self.active_settings_slider {
                    self.apply_settings_slider_x(settings, active, self.mouse_x);
                }
            }
        } else {
            self.update_skilltree_drag_from_frame(state, left_mouse_down);
        }

        if left_mouse_released || (self.skilltree_cam_input.left_down && !left_mouse_down) {
            self.mouse_release_was_drag = self.skilltree_cam_input.drag_started;
            self.skilltree_cam_input.left_down = false;
            self.skilltree_cam_input.drag_started = false;
            self.skilltree_cam_input.drag_started_in_view = false;
            self.active_settings_slider = None;
        }

        let view = state.state().view;
        if view.is_tetris_playing() {
            if let Some(action) = self.horizontal_repeat.next_repeat_action(now) {
                apply_action(
                    state,
                    self.sfx.as_ref(),
                    self.player_settings.audio,
                    &mut self.debug_hud,
                    action,
                );
            }
        }

        let mut allow_ui = true;
        if left_mouse_released && self.consume_next_mouse_up {
            self.consume_next_mouse_up = false;
            allow_ui = false;
        }

        if left_mouse_released
            && allow_ui
            && let Some(pointer) = input.mouse_pos
        {
            let handled = layout
                .action_at(state.state(), pointer)
                .is_some_and(|action| self.handle_ui_action(state, action));
            if !handled && !self.settings_open {
                let commands = headful_input::handle_skilltree_world_click(
                    state,
                    self.skilltree_layout,
                    pointer.0,
                    pointer.1,
                    self.mouse_release_was_drag,
                );
                self.apply_input_commands(state, commands);
            }
        }
        if left_mouse_released && self.settings_open {
            self.save_settings_if_dirty();
        }

        if left_mouse_released {
            self.mouse_release_was_drag = false;
        }

        self.update_round_timer_and_game_over(state, dt);
        self.apply_gravity_steps(state, dt);

        let view = state.state().view;
        if matches!(view, GameView::SkillTree) {
            let skilltree = &mut state.state_mut().skilltree;
            headful_camera::apply_wheel_zoom(
                skilltree,
                self.skilltree_layout,
                self.mouse_x,
                self.mouse_y,
                input.scroll_y,
            );
            headful_camera::apply_edge_pan(
                skilltree,
                self.skilltree_layout,
                self.mouse_x,
                self.mouse_y,
                dt,
                self.skilltree_cam_input.left_down,
            );
            headful_camera::finalize_camera(skilltree, self.skilltree_layout);
        }

        self.update_dig_camera_state(state, dt);
        let state = state.state_mut();
        state.input = Some(RecordedInput::from_frame(input, size));
        state.settings_overlay = self.settings_open.then(|| self.player_settings.clone());
    }

    fn render(&mut self, runner: &Self::State, renderer: &mut dyn engine::graphics::Renderer2d) {
        render_headful_frame(
            renderer,
            &self.debug_hud,
            runner.state(),
            self.render_inputs(),
        )
        .record(&mut self.debug_hud);
    }

    fn exit_requested(&self) -> bool {
        self.exit_requested
    }

    fn shutdown(&mut self) {
        self.save_settings_if_dirty();
    }

    fn handle_event(
        &mut self,
        event: &Event<()>,
        _runner: &mut Self::State,
        _input: &mut InputFrame,
        _ctx: &mut AppContext,
        control_flow: &mut ControlFlow,
    ) -> bool {
        // Lifecycle-only: gameplay/UI input is handled via `InputFrame` in `update_state`.
        *control_flow = ControlFlow::WaitUntil(self.next_redraw);

        if self.exit_requested {
            *control_flow = ControlFlow::Exit;
            return true;
        }

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
                return true;
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                if now < self.next_redraw {
                    return true;
                }
                self.next_redraw = now + self.frame_interval;
            }
            Event::LoopDestroyed => {
                self.shutdown();
                return true;
            }
            _ => {}
        }

        false
    }
}

impl TimelineGame for HeadfulApp {
    type Snapshot = GameState;
    type PreviewSnapshot = PreviewSnapshot;
    type PreviewFeedback = DrawTimings;
    const SCHEMA: StateSchema = TIMELINE_SCHEMA;

    fn preview_snapshot(&self, runner: &Self::State) -> PreviewSnapshot {
        PreviewSnapshot {
            state: runner.state().clone(),
            inputs: self.render_inputs(),
            debug_hud: self.debug_hud.clone(),
        }
    }

    fn draw_preview(
        snapshot: PreviewSnapshot,
        renderer: &mut dyn engine::graphics::Renderer2d,
    ) -> DrawTimings {
        render_headful_frame(
            renderer,
            &snapshot.debug_hud,
            &snapshot.state,
            snapshot.inputs,
        )
    }

    fn apply_preview_feedback(&mut self, _runner: &Self::State, timings: DrawTimings) {
        timings.record(&mut self.debug_hud);
    }

    fn set_history_view(&mut self, historical: bool) {
        self.historical = historical;
    }

    fn snapshot(state: &Self::State) -> &Self::Snapshot {
        state.state()
    }

    fn restore_state(&mut self, mut snapshot: Self::Snapshot) -> Self::State {
        snapshot.use_timeline_progress();
        let logic = logic_for_run(&self.base_logic, snapshot.skilltree.run_mods());
        self.settings_open = snapshot.settings_overlay.is_some();
        let runner = HeadlessRunner::from_state(logic, snapshot);
        self.horizontal_repeat.clear();
        self.dig_camera
            .reset(runner.state().tetris.background_depth_rows());
        self.skilltree_cam_input = SkillTreeCameraInput::default();
        self.skilltree_layout = SkillTreeLayout::default();
        self.active_settings_slider = None;
        self.mouse_release_was_drag = false;
        self.consume_next_mouse_up = false;
        self.last_frame_dt = Duration::ZERO;
        runner
    }

    fn timeline_command(&self, input: &InputFrame) -> Option<TimelineCommand> {
        use VirtualKeyCode::{F5, F6, F7, F8, LAlt, Left, P, RAlt, Right, Space};

        let option_held = input.keys_down.contains(&LAlt) || input.keys_down.contains(&RAlt);
        [
            (F5, P, TimelineCommand::TogglePause),
            (F6, Left, TimelineCommand::Rewind),
            (F7, Right, TimelineCommand::Forward),
            (F8, Space, TimelineCommand::TogglePlayback),
        ]
        .into_iter()
        .find_map(|(function_key, alternate, command)| {
            (input.keys_pressed.contains(&function_key)
                || (option_held && input.keys_pressed.contains(&alternate)))
            .then_some(command)
        })
    }
}

fn money_earned_from_run(state: &GameState) -> u32 {
    headful_view::money_earned_from_run(state)
}

type SkillTreeCameraInput = headful_camera::SkillTreeCameraInput;

fn skilltree_grid_viewport(layout: SkillTreeLayout) -> Option<Rect> {
    headful_camera::skilltree_grid_viewport(layout)
}

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

fn bottomwell_run_mods_from_skill_mods(mods: SkillTreeRunMods) -> BottomwellRunMods {
    BottomwellRunMods {
        deep_shaft_rows: mods.deep_shaft_rows,
        ore_weight_points: mods.ore_weight_points,
        coin_weight_points: mods.coin_weight_points,
        ore_score_bonus: mods.ore_score_bonus,
        coin_score_bonus: mods.coin_score_bonus,
        ore_money_bonus: mods.ore_money_bonus,
        coin_money_bonus: mods.coin_money_bonus,
        hole_patch_chance_bp: mods.hole_patch_chance_bp,
        hole_align_chance_bp: mods.hole_align_chance_bp,
    }
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
        .with_bottomwell_run_mods(bottomwell_run_mods_from_skill_mods(mods))
}

fn reset_run(
    runner: &mut HeadlessRunner<TetrisLogic>,
    base_logic: &TetrisLogic,
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    horizontal_repeat: &mut HorizontalRepeat,
    timeline_mode: bool,
) {
    let skilltree = runner.state().skilltree.clone();
    let view = runner.state().view;
    let mods = skilltree.run_mods();
    let tuning = run_tuning_from_mods(base_round_limit, base_gravity_interval, mods);

    let logic = logic_for_run(base_logic, mods);
    let mut next_runner = HeadlessRunner::new(logic);
    if timeline_mode {
        next_runner
            .state_mut()
            .tetris
            .inherit_timeline_progress(&runner.state().tetris);
    }
    {
        let state = next_runner.state_mut();
        state.skilltree = skilltree;
        state.view = view;
        state.round_timer = RoundTimer::new(tuning.round_limit);
        state.gravity_interval = tuning.gravity_interval;
        state.gravity_elapsed = Duration::ZERO;
    }
    *runner = next_runner;
    horizontal_repeat.clear();
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
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

fn env_present_mode(name: &str) -> Option<pixels::wgpu::PresentMode> {
    use pixels::wgpu::PresentMode;

    let v = std::env::var(name).ok()?;
    match v.to_ascii_lowercase().as_str() {
        "auto" | "auto_vsync" | "vsync" => Some(PresentMode::AutoVsync),
        "auto_no_vsync" | "auto_novsync" | "no_vsync" | "novsync" => Some(PresentMode::AutoNoVsync),
        "fifo" => Some(PresentMode::Fifo),
        "mailbox" => Some(PresentMode::Mailbox),
        "immediate" => Some(PresentMode::Immediate),
        _ => None,
    }
}

fn is_running_in_wsl() -> bool {
    std::env::var_os("WSL_INTEROP").is_some() || std::env::var_os("WSL_DISTRO_NAME").is_some()
}

fn print_headful_help() {
    // go.sh is the primary control surface, but `cargo run -p game --bin headful -- --help`
    // should still be self-explanatory.
    println!(
        r#"Tetree Headful

Usage:
  headful [--timeline DIR] [--fresh | --playback]
  headful --export-video PATH [--timeline DIR]

Flags:
  --timeline DIR    Select a timeline (default: target/timelines/headful).
  --fresh           Start a separate timeline. With --timeline, DIR must be new.
  --playback        Open a saved timeline paused at its first snapshot.
  --export-video PATH
                    Render a saved timeline to MP4 without opening a window.
  Option+P (F5)     Pause live play or explicitly resume it from the cursor.
  Option+Left (F6)  Step backward one snapshot.
  Option+Right (F7) Step forward one snapshot.
  Option+Space (F8) Play / pause the recorded timeline. Playback stops at the end.
  Use Alt in place of Option on other keyboards.
  --help, -h        Show this help.
"#
    );
}
#[derive(Debug, Clone)]
struct BgMusic {
    sample_rate: u32,
    channels: u16,
    chan: u16,
    last_sample: f32,
    runtime: MusicRuntime,
}

impl BgMusic {
    fn new() -> Self {
        let notes = vec![
            Some(220.0),
            Some(261.63),
            Some(329.63),
            Some(261.63),
            Some(196.0),
            Some(246.94),
            Some(293.66),
            Some(246.94),
        ];
        let arp = Track::new(
            "arp",
            StepPattern::from_notes(notes, 0.5).with_envelope(0.04, 0.12),
        )
        .with_gain(0.14)
        .with_waveform(Waveform::Sine)
        .with_harmonic_mix(0.30);

        let bass = Track::new(
            "bass",
            StepPattern::from_notes(
                vec![
                    Some(110.0),
                    None,
                    Some(98.0),
                    None,
                    Some(123.47),
                    None,
                    Some(98.0),
                    None,
                ],
                0.5,
            )
            .with_envelope(0.02, 0.10),
        )
        .with_gain(0.10)
        .with_waveform(Waveform::Triangle);

        let kick = Track::new(
            "kick",
            StepPattern::from_notes(
                vec![
                    Some(55.0),
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                ],
                0.25,
            )
            .with_envelope(0.001, 0.08),
        )
        .with_gain(0.16)
        .with_waveform(Waveform::Sine);

        let mut runtime = MusicRuntime::new(48_000, 120.0);
        let _ = runtime.add_scene(
            Scene::new("main")
                .with_track(arp)
                .with_track(bass)
                .with_track(kick),
        );
        let _ = runtime.schedule_scene_switch("main", Quantize::Immediate);

        Self {
            sample_rate: 48_000,
            channels: 2,
            chan: 0,
            last_sample: 0.0,
            runtime,
        }
    }
}

impl Iterator for BgMusic {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // Reuse each mono sample for L/R channels.
        if self.chan == 0 {
            self.chan = 1;
            self.last_sample = self.runtime.next_mono_sample();
            Some(self.last_sample)
        } else {
            self.chan = 0;
            Some(self.last_sample)
        }
    }
}

impl rodio::Source for BgMusic {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[derive(Debug, Clone)]
struct GlassBreakNoise {
    sample_rate: u32,
    channels: u16,
    frame: u64,
    chan: u16,
    state: u32,
}

impl GlassBreakNoise {
    const DURATION_MS: u64 = 180;

    fn new(seed: u32) -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame: 0,
            chan: 0,
            state: seed.max(1),
        }
    }

    fn total_frames(&self) -> u64 {
        self.sample_rate as u64 * Self::DURATION_MS / 1_000
    }

    fn next_noise(&mut self) -> f32 {
        // Xorshift32 pseudo-noise for a light "glassy" burst.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        let unit = (x as f32) / (u32::MAX as f32);
        unit * 2.0 - 1.0
    }
}

impl Iterator for GlassBreakNoise {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.frame >= self.total_frames() {
            return None;
        }
        let t = self.frame as f32 / self.sample_rate as f32;
        let progress = self.frame as f32 / self.total_frames() as f32;
        let env = (1.0 - progress).max(0.0).powf(2.2);
        let chirp = (2.0 * std::f32::consts::PI * (1_800.0 * t + 900.0 * t * t)).sin();
        let noise = self.next_noise();
        let sample = (chirp * 0.35 + noise * 0.65) * env * 0.45;

        self.chan += 1;
        if self.chan >= self.channels {
            self.chan = 0;
            self.frame = self.frame.saturating_add(1);
        }
        Some(sample)
    }
}

impl rodio::Source for GlassBreakNoise {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.total_frames().saturating_sub(self.frame) as usize)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_millis(Self::DURATION_MS))
    }
}

struct Sfx {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    click_wav: &'static [u8],
    music_sink: Option<Sink>,
    music_playing: Cell<bool>,
}

impl Sfx {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (stream, handle) = OutputStream::try_default()?;
        let music_sink = Sink::try_new(&handle).ok().map(|sink| {
            sink.set_volume(MUSIC_VOLUME);
            sink.append(BgMusic::new());
            sink
        });
        Ok(Self {
            _stream: stream,
            handle,
            click_wav: include_bytes!("../../../assets/sfx/click.wav"),
            music_playing: Cell::new(music_sink.is_some()),
            music_sink,
        })
    }

    fn play_click(&self, volume: f32) {
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.set_volume(volume);

        let Ok(source) = Decoder::new(Cursor::new(self.click_wav)) else {
            return;
        };
        sink.append(source);
        sink.detach();
    }

    fn play_glass_break(&self, volume: f32) {
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.set_volume(volume);
        sink.append(GlassBreakNoise::new(0xA53F_91C7));
        sink.detach();
    }

    fn apply_audio_settings(&self, audio: AudioSettings) {
        let Some(sink) = self.music_sink.as_ref() else {
            return;
        };

        let gain = MUSIC_VOLUME * audio.effective_music_gain();
        sink.set_volume(gain);
        if audio.music_enabled && !audio.mute_all {
            sink.play();
            self.music_playing.set(true);
        } else {
            sink.pause();
            self.music_playing.set(false);
        }
    }
}

#[cfg(test)]
type HorizontalDir = headful_input::HorizontalDir;
type HorizontalRepeat = headful_input::HorizontalRepeat;
type DigCameraController = headful_dig_camera::DigCameraController;
#[cfg(test)]
type DigCameraConfig = headful_dig_camera::DigCameraConfig;

#[cfg(test)]
fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    headful_input::map_key_to_action(key)
}

fn should_play_action_sfx(action: InputAction) -> bool {
    headful_input::should_play_action_sfx(action)
}

fn apply_action(
    runner: &mut HeadlessRunner<TetrisLogic>,
    sfx: Option<&Sfx>,
    audio: AudioSettings,
    debug_hud: &mut DebugHud,
    action: InputAction,
) {
    let input_start = Instant::now();
    let before_glass_shatters = runner.state().tetris.glass_shatter_count();

    runner.step_profiled(action, debug_hud);

    if let Some(sfx) = sfx {
        if should_play_action_sfx(action) {
            sfx.play_click(ACTION_SFX_VOLUME * audio.effective_sfx_gain());
        }
        let after_glass_shatters = runner.state().tetris.glass_shatter_count();
        if after_glass_shatters > before_glass_shatters {
            sfx.play_glass_break(GLASS_BREAK_SFX_VOLUME * audio.effective_sfx_gain());
        }
    }

    debug_hud.record_input(input_start.elapsed());
}

#[cfg(test)]
mod headful_tests;
