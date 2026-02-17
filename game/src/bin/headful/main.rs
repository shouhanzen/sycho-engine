use std::{
    cell::Cell,
    io,
    io::Cursor,
    time::{Duration, Instant},
};

use engine::HeadlessRunner;
use engine::app::{
    AppConfig, AppContext, CaptureCli, GameApp, InputFrame, ProfileConfig, RecordingConfig,
    ReplayConfig, RunMode, default_recording_path, parse_capture_cli_with_default_path, run_game,
    run_game_with_profile, run_game_with_recording, run_game_with_replay,
};
use engine::ui_tree::{UiEvent, UiInput, UiTree};
use engine::audio::{MusicRuntime, Quantize, Scene, StepPattern, Track, Waveform};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
#[cfg(test)]
use winit::event::VirtualKeyCode;
use winit::{
    dpi::PhysicalSize,
    event::{Event, MouseButton, WindowEvent},
    event_loop::ControlFlow,
};

use game::debug::DebugHud;
use game::headful::dig_camera as headful_dig_camera;
use game::headful::input_adapter as headful_input;
use game::headful::remote_control as headful_remote;
use game::headful::render_pipeline::{RenderCache, render_frame as render_headful_frame};
use game::headful::skilltree_camera as headful_camera;
use game::headful::view_transitions as headful_view;
use game::headful_editor_api::RemoteServer;
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::settings::{AudioSettings, PlayerSettings, SettingsStore};
use game::sfx::{ACTION_SFX_VOLUME, GLASS_BREAK_SFX_VOLUME, MUSIC_VOLUME};
use game::skilltree::{SkillTreeEditorTool, SkillTreeRunMods, SkillTreeRuntime};
use game::state::{DEFAULT_GRAVITY_INTERVAL, DEFAULT_ROUND_LIMIT, GameState};
use game::tetris_core::{
    BottomwellRunMods, DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE, DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT,
    Piece, default_depth_wall_defs,
};
use game::tetris_ui::{
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SettingsMenuLayout, SkillTreeLayout,
    UiLayout,
};
use game::view::GameView;
use game::view_tree::{
    GameUiAction, build_hud_view_tree, build_menu_view_tree, build_skilltree_toolbar_view_tree,
};

struct HeadfulApp {
    profile_mode: bool,
    base_logic: TetrisLogic,
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    sfx: Option<Sfx>,
    debug_hud: DebugHud,
    ui_tree: UiTree,
    last_layout: UiLayout,
    last_main_menu: MainMenuLayout,
    last_pause_menu: PauseMenuLayout,
    last_skilltree: SkillTreeLayout,
    last_game_over_menu: GameOverMenuLayout,
    last_settings_menu: SettingsMenuLayout,
    mouse_x: u32,
    mouse_y: u32,
    skilltree_cam_input: SkillTreeCameraInput,
    horizontal_repeat: HorizontalRepeat,
    dig_camera: DigCameraController,
    frame_interval: Duration,
    next_redraw: Instant,
    remote_editor_api: Option<RemoteServer>,
    last_frame_dt: Duration,
    exit_requested: bool,
    mouse_release_was_drag: bool,
    consume_next_mouse_up: bool,
    render_state: Option<GameState>,
    settings_store: SettingsStore,
    player_settings: PlayerSettings,
    settings_open: bool,
    settings_origin: SettingsOrigin,
    active_settings_slider: Option<ActiveSettingsSlider>,
    settings_dirty: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SettingsOrigin {
    #[default]
    MainMenu,
    PauseMenu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveSettingsSlider {
    Master,
    Music,
    Sfx,
    ScreenShake,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let CaptureCli {
        help,
        record_path,
        replay_path,
    } = parse_capture_cli_with_default_path(|| default_recording_path("headful"))?;
    if help {
        print_headful_help();
        return Ok(());
    }

    if let Some(path) = record_path.as_ref() {
        println!("state recording enabled: will save to {}", path.display());
    }
    if let Some(path) = replay_path.as_ref() {
        println!("replay: {}", path.display());
    }

    let profile_frames = env_usize("ROLLOUT_HEADFUL_PROFILE_FRAMES").unwrap_or(0);
    if replay_path.is_some() && profile_frames > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --replay with ROLLOUT_HEADFUL_PROFILE_FRAMES (profiling mode)",
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

    if let Some(path) = replay_path {
        run_game_with_replay(config, app, ReplayConfig { path, fps: 15 })
    } else if let Some(path) = record_path {
        run_game_with_recording(config, app, RecordingConfig { path })
    } else if profile_frames > 0 {
        run_game_with_profile(
            config,
            app,
            ProfileConfig {
                target_frames: profile_frames,
            },
        )
    } else {
        run_game(config, app)
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
        let remote_editor_api = match env_u16("ROLLOUT_HEADFUL_EDITOR_PORT").unwrap_or(0) {
            0 => None,
            port => match RemoteServer::start(port) {
                Ok(server) => {
                    println!("headful editor api: http://{}", server.info.addr);
                    Some(server)
                }
                Err(err) => {
                    eprintln!(
                        "warning: failed to start headful editor api on 127.0.0.1:{port}: {err}"
                    );
                    None
                }
            },
        };
        let mut debug_hud = DebugHud::new();
        if env_bool("ROLLOUT_DEBUG_DISABLE_ROUND_TIMER").unwrap_or(false) {
            debug_hud.set_round_timer_disabled(true);
        }
        let app = Self {
            profile_mode: false,
            base_logic,
            base_round_limit,
            base_gravity_interval,
            sfx,
            debug_hud,
            ui_tree: UiTree::new(),
            last_layout: UiLayout::default(),
            last_main_menu: MainMenuLayout::default(),
            last_pause_menu: PauseMenuLayout::default(),
            last_skilltree: SkillTreeLayout::default(),
            last_game_over_menu: GameOverMenuLayout::default(),
            last_settings_menu: SettingsMenuLayout::default(),
            mouse_x: 0,
            mouse_y: 0,
            skilltree_cam_input: SkillTreeCameraInput::default(),
            horizontal_repeat: HorizontalRepeat::default(),
            dig_camera: DigCameraController::from_env(),
            frame_interval,
            next_redraw: Instant::now(),
            remote_editor_api,
            last_frame_dt: Duration::ZERO,
            exit_requested: false,
            mouse_release_was_drag: false,
            consume_next_mouse_up: false,
            render_state: None,
            settings_store,
            player_settings,
            settings_open: false,
            settings_origin: SettingsOrigin::default(),
            active_settings_slider: None,
            settings_dirty: false,
        };
        app.apply_audio_settings();
        app
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
        let origin = match view {
            GameView::MainMenu => SettingsOrigin::MainMenu,
            GameView::Tetris { paused: true } => SettingsOrigin::PauseMenu,
            _ => return false,
        };
        self.settings_open = true;
        self.settings_origin = origin;
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

    fn settings_slider_from_pointer(&self, x: u32, y: u32) -> Option<ActiveSettingsSlider> {
        let layout = self.last_settings_menu;
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

    fn apply_settings_slider_x(&mut self, slider: ActiveSettingsSlider, x: u32) {
        let mut changed = false;
        match slider {
            ActiveSettingsSlider::Master => {
                let slider = engine::slider::Slider::new(
                    self.last_settings_menu.master_track,
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
                    self.last_settings_menu.music_track,
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
                    self.last_settings_menu.sfx_track,
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
                    self.last_settings_menu.shake_track,
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

    fn drain_remote_commands(&mut self, state: &mut HeadlessRunner<TetrisLogic>) {
        let mut remote_editor_api = self.remote_editor_api.take();
        headful_remote::drain_remote_commands(remote_editor_api.as_mut(), state, |runner| {
            self.reset_active_run(runner);
        });
        self.remote_editor_api = remote_editor_api;
    }

    fn handle_viewtree_action(
        &mut self,
        state: &mut HeadlessRunner<TetrisLogic>,
        action: GameUiAction,
    ) -> bool {
        match action {
            GameUiAction::StartGame => {
                let view = state.state().view;
                if matches!(view, GameView::MainMenu) {
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
            GameUiAction::SkillTreeToolSelect => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                    skilltree.editor_set_tool(SkillTreeEditorTool::Select);
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::SkillTreeToolMove => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                    skilltree.editor_set_tool(SkillTreeEditorTool::Move);
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::SkillTreeToolAddCell => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                    skilltree.editor_set_tool(SkillTreeEditorTool::AddCell);
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::SkillTreeToolRemoveCell => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                    skilltree.editor_set_tool(SkillTreeEditorTool::RemoveCell);
                    self.play_click_sfx();
                    return true;
                }
            }
            GameUiAction::SkillTreeToolConnect => {
                let view = state.state().view;
                let skilltree = &mut state.state_mut().skilltree;
                if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                    skilltree.editor_set_tool(SkillTreeEditorTool::ConnectPrereqs);
                    self.play_click_sfx();
                    return true;
                }
            }
        }

        false
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
            self.last_skilltree,
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
            self.last_skilltree,
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
    type Action = GameUiAction;
    type Effect = ();

    fn init_state(&mut self, _ctx: &mut AppContext) -> Self::State {
        let mut runner = HeadlessRunner::new(self.base_logic.clone());
        if let Some(record_every) = env_usize("ROLLOUT_RECORD_EVERY_N_FRAMES") {
            runner.set_record_every_n_frames(record_every.max(1));
        }
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
        self.render_state = Some(runner.state().clone());
        runner
    }

    fn on_run_mode(&mut self, mode: RunMode, state: &mut Self::State, _ctx: &mut AppContext) {
        self.profile_mode = matches!(mode, RunMode::Profile);
        if self.profile_mode {
            state.state_mut().view = GameView::Tetris { paused: false };
        }
    }

    fn build_view(
        &self,
        state: &Self::State,
        ctx: &AppContext,
    ) -> engine::view_tree::ViewTree<Self::Action> {
        let size = ctx.renderer.size();
        let mut tree = build_menu_view_tree(state.state().view, size.width, size.height);
        tree.nodes
            .extend(build_hud_view_tree(state.state(), size.width, size.height).nodes);
        tree.nodes.extend(
            build_skilltree_toolbar_view_tree(state.state(), size.width, size.height).nodes,
        );
        tree
    }

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: InputFrame,
        dt: Duration,
        actions: &[Self::Action],
        ctx: &mut AppContext,
    ) -> Vec<Self::Effect> {
        self.last_frame_dt = dt;
        let now = Instant::now();

        self.drain_remote_commands(state);

        if let Some((mx, my)) = input.mouse_pos {
            self.mouse_x = mx;
            self.mouse_y = my;
        }
        let pointer_pos = Some((self.mouse_x, self.mouse_y));
        if input.mouse_pos.is_some() {
            let _ = self.ui_tree.process_input(UiInput {
                mouse_pos: pointer_pos,
                mouse_down: false,
                mouse_up: false,
            });
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

        if self.settings_open {
            if pressed(winit::event::VirtualKeyCode::Escape) {
                self.close_settings();
                self.play_click_sfx();
            }
        } else if pressed(winit::event::VirtualKeyCode::O) {
            if self.open_settings_from_view(state.state().view) {
                self.play_click_sfx();
            }
        }

        if !self.settings_open {
            self.process_keyboard_frame(state, &input, now);
        }

        if left_mouse_pressed {
            let size = ctx.renderer.size();
            let debug_clicked =
                self.debug_hud
                    .handle_click(self.mouse_x, self.mouse_y, size.width, size.height);
            if debug_clicked {
                self.consume_next_mouse_up = true;
            } else if self.settings_open {
                if let Some(slider) = self.settings_slider_from_pointer(self.mouse_x, self.mouse_y)
                {
                    self.active_settings_slider = Some(slider);
                    self.apply_settings_slider_x(slider, self.mouse_x);
                } else {
                    self.active_settings_slider = None;
                }
            } else {
                let _ = self.ui_tree.process_input(UiInput {
                    mouse_pos: pointer_pos,
                    mouse_down: true,
                    mouse_up: false,
                });
                if matches!(state.state().view, GameView::SkillTree) {
                    self.skilltree_cam_input.left_down = true;
                    self.skilltree_cam_input.drag_started = false;
                    self.skilltree_cam_input.drag_started_in_view =
                        skilltree_grid_viewport(self.last_skilltree)
                            .map(|r| r.contains(self.mouse_x, self.mouse_y))
                            .unwrap_or(false);
                    self.skilltree_cam_input.down_x = self.mouse_x;
                    self.skilltree_cam_input.down_y = self.mouse_y;
                    self.skilltree_cam_input.last_x = self.mouse_x;
                    self.skilltree_cam_input.last_y = self.mouse_y;
                }
            }
        }

        if self.settings_open {
            if left_mouse_down {
                if let Some(active) = self.active_settings_slider {
                    self.apply_settings_slider_x(active, self.mouse_x);
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

        let mut ui_handled = false;
        if left_mouse_released && allow_ui && self.settings_open {
            let l = self.last_settings_menu;
            if l.back_button.contains(self.mouse_x, self.mouse_y) {
                self.close_settings();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.reset_button.contains(self.mouse_x, self.mouse_y) {
                self.player_settings = PlayerSettings::default();
                self.apply_audio_settings();
                self.mark_settings_dirty();
                self.save_settings_if_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.mute_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.audio.mute_all = !self.player_settings.audio.mute_all;
                self.apply_audio_settings();
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.music_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.audio.music_enabled =
                    !self.player_settings.audio.music_enabled;
                self.apply_audio_settings();
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.show_timer_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.gameplay.show_round_timer =
                    !self.player_settings.gameplay.show_round_timer;
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.auto_pause_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.gameplay.auto_pause_on_focus_loss =
                    !self.player_settings.gameplay.auto_pause_on_focus_loss;
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.high_contrast_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.accessibility.high_contrast_ui =
                    !self.player_settings.accessibility.high_contrast_ui;
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            } else if l.reduce_motion_toggle.contains(self.mouse_x, self.mouse_y) {
                self.player_settings.accessibility.reduce_motion =
                    !self.player_settings.accessibility.reduce_motion;
                self.mark_settings_dirty();
                self.play_click_sfx();
                ui_handled = true;
            }
            self.save_settings_if_dirty();
        }

        if left_mouse_released && allow_ui && !ui_handled && !self.settings_open {
            let view = state.state().view;
            let clicked_settings = match view {
                GameView::MainMenu => self
                    .last_main_menu
                    .settings_button
                    .contains(self.mouse_x, self.mouse_y),
                GameView::Tetris { paused: true } => self
                    .last_pause_menu
                    .settings_button
                    .contains(self.mouse_x, self.mouse_y),
                _ => false,
            };
            if clicked_settings && self.open_settings_from_view(view) {
                self.play_click_sfx();
                ui_handled = true;
            }
        }

        if left_mouse_released && allow_ui && !ui_handled && !self.settings_open {
            if let Some(action) = actions.first().copied() {
                ui_handled = self.handle_viewtree_action(state, action);
            }
        }

        if left_mouse_released && allow_ui && !ui_handled && !self.settings_open {
            let ui_events = self.ui_tree.process_input(UiInput {
                mouse_pos: pointer_pos,
                mouse_down: left_mouse_pressed,
                mouse_up: left_mouse_released,
            });
            for event in ui_events {
                if let UiEvent::Click {
                    action: Some(action),
                    ..
                } = event
                {
                    let result = headful_input::handle_ui_tree_click_action(state, action);
                    self.apply_input_commands(state, result.commands);
                    if result.handled {
                        ui_handled = true;
                    }
                }
            }
        }

        if left_mouse_released && allow_ui && !ui_handled && !self.settings_open {
            let commands = headful_input::handle_skilltree_world_click(
                state,
                self.last_skilltree,
                self.mouse_x,
                self.mouse_y,
                self.mouse_release_was_drag,
            );
            self.apply_input_commands(state, commands);
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
                self.last_skilltree,
                self.mouse_x,
                self.mouse_y,
                input.scroll_y,
            );
            headful_camera::apply_edge_pan(
                skilltree,
                self.last_skilltree,
                self.mouse_x,
                self.mouse_y,
                dt,
                self.skilltree_cam_input.left_down,
            );
            headful_camera::finalize_camera(skilltree, self.last_skilltree);
        }

        self.update_dig_camera_state(state, dt);
        self.render_state = Some(state.state().clone());
        Vec::new()
    }

    fn render(
        &mut self,
        _view: &engine::view_tree::ViewTree<Self::Action>,
        renderer: &mut dyn engine::graphics::Renderer2d,
    ) {
        let Some(state) = self.render_state.as_ref() else {
            return;
        };

        let mut cache = RenderCache {
            last_layout: self.last_layout,
            last_main_menu: self.last_main_menu,
            last_pause_menu: self.last_pause_menu,
            last_skilltree: self.last_skilltree,
            last_game_over_menu: self.last_game_over_menu,
            last_settings_menu: self.last_settings_menu,
        };
        let mut camera_offset = self.dig_camera.offset_y_px();
        if self.player_settings.accessibility.reduce_motion {
            camera_offset = 0.0;
        } else {
            camera_offset *= self.player_settings.video.clamped_screen_shake() as f32 / 100.0;
        }
        render_headful_frame(
            renderer,
            &mut self.ui_tree,
            &mut self.debug_hud,
            state,
            self.mouse_x,
            self.mouse_y,
            &mut cache,
            camera_offset.round() as i32,
            self.last_frame_dt,
            self.settings_open.then_some(&self.player_settings),
            self.player_settings.gameplay.show_round_timer,
        );
        self.last_layout = cache.last_layout;
        self.last_main_menu = cache.last_main_menu;
        self.last_pause_menu = cache.last_pause_menu;
        self.last_skilltree = cache.last_skilltree;
        self.last_game_over_menu = cache.last_game_over_menu;
        self.last_settings_menu = cache.last_settings_menu;
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
                self.save_settings_if_dirty();
                if let Some(remote) = self.remote_editor_api.as_mut() {
                    remote.shutdown();
                }

                return true;
            }
            _ => {}
        }

        false
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
    score_bonus_per_line: u32,
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
        score_bonus_per_line: mods.score_bonus_per_line,
    }
}

fn reset_run(
    runner: &mut HeadlessRunner<TetrisLogic>,
    base_logic: &TetrisLogic,
    base_round_limit: Duration,
    base_gravity_interval: Duration,
    horizontal_repeat: &mut HorizontalRepeat,
) {
    let skilltree = runner.state().skilltree.clone();
    let view = runner.state().view;
    let mods = skilltree.run_mods();
    let tuning = run_tuning_from_mods(base_round_limit, base_gravity_interval, mods);

    let logic = base_logic
        .clone()
        .with_score_bonus_per_line(tuning.score_bonus_per_line)
        .with_bottomwell_run_mods(bottomwell_run_mods_from_skill_mods(mods));
    let mut next_runner = HeadlessRunner::new(logic);
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

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok().and_then(|v| v.parse::<u16>().ok())
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
  headful [--record [PATH]]
  headful --replay PATH

Flags:
  --record [PATH]   Save the in-memory TimeMachine (frame-by-frame state history) to a JSON file on exit.
                   If PATH is omitted, writes to: target/recordings/headful_<nanos>.json
  --replay PATH     Load a previously saved JSON recording and replay it.
                   Replay controls:
                     Space: play/pause
                     Left/Right: step -/+1 frame (pauses)
                     Home/End: jump to start/end (pauses)
                     Up/Down: speed x2 / ÷2
                     Esc: quit
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
                vec![Some(110.0), None, Some(98.0), None, Some(123.47), None, Some(98.0), None],
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
                .with_track(kick)
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
