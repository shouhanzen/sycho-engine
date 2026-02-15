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
use engine::editor::{EditorSnapshot, EditorTimeline};
use engine::ui_tree::{UiEvent, UiInput, UiTree};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use winit::{
    dpi::PhysicalSize,
    event::{Event, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::ControlFlow,
};

use game::debug::DebugHud;
use game::headful::dig_camera as headful_dig_camera;
use game::headful::input_adapter as headful_input;
use game::headful::render_pipeline::{RenderCache, render_frame as render_headful_frame};
use game::headful::skilltree_camera as headful_camera;
use game::headful::view_transitions as headful_view;
use game::headful_editor_api::{RemoteCmd, RemoteServer};
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::sfx::{ACTION_SFX_VOLUME, MUSIC_VOLUME};
use game::skilltree::{SkillTreeEditorTool, SkillTreeRunMods, SkillTreeRuntime};
use game::state::{DEFAULT_GRAVITY_INTERVAL, DEFAULT_ROUND_LIMIT, GameState};
use game::tetris_core::{Piece, Vec2i};
use game::tetris_ui::{
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SkillTreeLayout, UiLayout,
};
use game::ui_ids::*;
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

    let base_logic = TetrisLogic::new(0, Piece::all()).with_bottomwell(true);
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
        Self {
            profile_mode: false,
            base_logic,
            base_round_limit,
            base_gravity_interval,
            sfx,
            debug_hud: DebugHud::new(),
            ui_tree: UiTree::new(),
            last_layout: UiLayout::default(),
            last_main_menu: MainMenuLayout::default(),
            last_pause_menu: PauseMenuLayout::default(),
            last_skilltree: SkillTreeLayout::default(),
            last_game_over_menu: GameOverMenuLayout::default(),
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
        }
    }

    fn play_click_sfx(&self) {
        if let Some(sfx) = self.sfx.as_ref() {
            sfx.play_click(ACTION_SFX_VOLUME);
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

    fn snapshot(runner: &HeadlessRunner<TetrisLogic>) -> EditorSnapshot {
        let frame = runner.frame();
        game::editor_api::snapshot_from_state(frame, runner.state())
    }

    fn timeline(runner: &HeadlessRunner<TetrisLogic>) -> EditorTimeline {
        let tm = runner.timemachine();
        EditorTimeline {
            frame: runner.frame(),
            history_len: runner.history().len(),
            can_rewind: tm.can_rewind(),
            can_forward: tm.can_forward(),
        }
    }

    fn handle_remote_command(&mut self, state: &mut HeadlessRunner<TetrisLogic>, cmd: RemoteCmd) {
        match cmd {
            RemoteCmd::GetState { respond } => {
                let _ = respond.send(Self::snapshot(state));
            }
            RemoteCmd::GetTimeline { respond } => {
                let _ = respond.send(Self::timeline(state));
            }
            RemoteCmd::Step { action_id, respond } => {
                match game::editor_api::action_from_id(&action_id) {
                    Some(action) => {
                        state.step(action);
                        let _ = respond.send(Ok(Self::snapshot(state)));
                    }
                    None => {
                        let _ = respond.send(Err(format!("unknown actionId: {action_id}")));
                    }
                }
            }
            RemoteCmd::Rewind { frames, respond } => {
                state.rewind(frames);
                let _ = respond.send(Self::snapshot(state));
            }
            RemoteCmd::Forward { frames, respond } => {
                state.forward(frames);
                let _ = respond.send(Self::snapshot(state));
            }
            RemoteCmd::Seek { frame, respond } => {
                state.seek(frame);
                let _ = respond.send(Self::snapshot(state));
            }
            RemoteCmd::Reset { respond } => {
                self.reset_active_run(state);
                let _ = respond.send(Self::snapshot(state));
            }
        }
    }

    fn drain_remote_commands(&mut self, state: &mut HeadlessRunner<TetrisLogic>) {
        loop {
            let next_cmd = {
                let Some(remote) = self.remote_editor_api.as_mut() else {
                    return;
                };
                match remote.rx.try_recv() {
                    Ok(cmd) => Some(cmd),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => None,
                }
            };
            let Some(cmd) = next_cmd else {
                break;
            };
            self.handle_remote_command(state, cmd);
        }
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
            state
                .round_timer
                .tick_if_running(dt, state.view.is_tetris_playing());
            if state.view.is_tetris_playing() && state.round_timer.is_up() {
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
        {
            let state = state.state_mut();
            if state.view.is_tetris_playing() {
                state.gravity_elapsed = state.gravity_elapsed.saturating_add(dt);
                while state.gravity_elapsed >= state.gravity_interval {
                    state.gravity_elapsed =
                        state.gravity_elapsed.saturating_sub(state.gravity_interval);
                    gravity_steps = gravity_steps.saturating_add(1);
                }
            } else {
                state.gravity_elapsed = Duration::ZERO;
            }
        }
        for _ in 0..gravity_steps {
            let gravity_start = Instant::now();
            state.step_profiled(InputAction::SoftDrop, &mut self.debug_hud);
            self.debug_hud.record_gravity(gravity_start.elapsed());
        }
    }

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
    ) {
        let pressed = |key| input.keys_pressed.contains(&key);

        if pressed(VirtualKeyCode::F3) {
            self.debug_hud.toggle();
        }
        if pressed(VirtualKeyCode::M) {
            if let Some(sfx) = self.sfx.as_ref() {
                sfx.toggle_music();
            }
        }

        let mut view = runner.state().view;
        match view {
            GameView::MainMenu => {
                if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                    let transition = headful_view::start_game(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    if transition.reset_tetris {
                        reset_run(
                            runner,
                            &self.base_logic,
                            self.base_round_limit,
                            self.base_gravity_interval,
                            &mut self.horizontal_repeat,
                        );
                    }
                    self.play_click_sfx();
                } else if pressed(VirtualKeyCode::K) {
                    let transition = headful_view::open_skilltree_editor(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    self.horizontal_repeat.clear();
                    let skilltree = &mut runner.state_mut().skilltree;
                    if !skilltree.editor.enabled {
                        skilltree.editor_toggle();
                    }
                    self.play_click_sfx();
                } else if pressed(VirtualKeyCode::Escape) {
                    self.exit_requested = true;
                }
            }
            GameView::SkillTree => {
                if pressed(VirtualKeyCode::F4) {
                    let skilltree = &mut runner.state_mut().skilltree;
                    skilltree.editor_toggle();
                    self.play_click_sfx();
                    return;
                }

                let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
                if skilltree_editor_enabled {
                    if pressed(VirtualKeyCode::Escape) {
                        {
                            let skilltree = &mut runner.state_mut().skilltree;
                            skilltree.editor_toggle();
                        }
                        let transition = headful_view::back(view);
                        view = transition.next_view;
                        runner.state_mut().view = view;
                        self.horizontal_repeat.clear();
                        self.play_click_sfx();
                        return;
                    }

                    let skilltree = &mut runner.state_mut().skilltree;
                    if pressed(VirtualKeyCode::Tab) {
                        skilltree.editor_cycle_tool();
                    }
                    if pressed(VirtualKeyCode::Left) {
                        skilltree.camera.pan.x -= 1.0;
                        skilltree.camera.target_pan = skilltree.camera.pan;
                    }
                    if pressed(VirtualKeyCode::Right) {
                        skilltree.camera.pan.x += 1.0;
                        skilltree.camera.target_pan = skilltree.camera.pan;
                    }
                    if pressed(VirtualKeyCode::Up) {
                        skilltree.camera.pan.y += 1.0;
                        skilltree.camera.target_pan = skilltree.camera.pan;
                    }
                    if pressed(VirtualKeyCode::Down) {
                        skilltree.camera.pan.y -= 1.0;
                        skilltree.camera.target_pan = skilltree.camera.pan;
                    }
                    if pressed(VirtualKeyCode::Minus) {
                        skilltree.camera.cell_px =
                            (skilltree.camera.cell_px - 2.0).max(SKILLTREE_CAMERA_MIN_CELL_PX);
                        skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                    }
                    if pressed(VirtualKeyCode::Equals) {
                        skilltree.camera.cell_px =
                            (skilltree.camera.cell_px + 2.0).min(SKILLTREE_CAMERA_MAX_CELL_PX);
                        skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                    }
                    if pressed(VirtualKeyCode::N) {
                        if let Some(world) = skilltree_world_cell_at_screen(
                            skilltree,
                            self.last_skilltree,
                            self.mouse_x,
                            self.mouse_y,
                        ) {
                            skilltree.editor_create_node_at(world);
                        }
                    }
                    if pressed(VirtualKeyCode::Delete) {
                        let _ = skilltree.editor_delete_selected();
                    }
                    if pressed(VirtualKeyCode::S) {
                        match skilltree.save_def() {
                            Ok(()) => {
                                skilltree.editor.dirty = false;
                                skilltree.editor.status = Some("SAVED".to_string());
                            }
                            Err(e) => {
                                skilltree.editor.status = Some(format!("SAVE FAILED: {e}"));
                            }
                        }
                    }
                    if pressed(VirtualKeyCode::R) {
                        skilltree.reload_def();
                        skilltree.editor.dirty = false;
                        skilltree.editor.status = Some("RELOADED".to_string());
                    }
                } else if pressed(VirtualKeyCode::Escape) {
                    let transition = headful_view::back(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                } else if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                    let transition = headful_view::start_game(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    if transition.reset_tetris {
                        reset_run(
                            runner,
                            &self.base_logic,
                            self.base_round_limit,
                            self.base_gravity_interval,
                            &mut self.horizontal_repeat,
                        );
                    }
                    self.play_click_sfx();
                }
            }
            GameView::GameOver => {
                if pressed(VirtualKeyCode::Escape) {
                    let transition = headful_view::back(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                } else if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                    let transition = headful_view::start_game(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    if transition.reset_tetris {
                        reset_run(
                            runner,
                            &self.base_logic,
                            self.base_round_limit,
                            self.base_gravity_interval,
                            &mut self.horizontal_repeat,
                        );
                    }
                    self.play_click_sfx();
                } else if pressed(VirtualKeyCode::K) {
                    let transition = headful_view::open_skilltree(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                }
            }
            GameView::Tetris { paused } => {
                if pressed(VirtualKeyCode::Escape) {
                    let transition = headful_view::toggle_pause(view);
                    view = transition.next_view;
                    {
                        let state = runner.state_mut();
                        state.view = view;
                        state.gravity_elapsed = Duration::ZERO;
                    }
                    self.horizontal_repeat.clear();
                    self.play_click_sfx();
                    return;
                }

                if paused {
                    return;
                }

                self.sync_horizontal_repeat_from_frame(runner, input, now);
                for key in [
                    VirtualKeyCode::Down,
                    VirtualKeyCode::S,
                    VirtualKeyCode::Up,
                    VirtualKeyCode::W,
                    VirtualKeyCode::Z,
                    VirtualKeyCode::X,
                    VirtualKeyCode::A,
                    VirtualKeyCode::Space,
                    VirtualKeyCode::C,
                ] {
                    if pressed(key) {
                        if let Some(action) = map_key_to_action(key) {
                            apply_action(runner, self.sfx.as_ref(), &mut self.debug_hud, action);
                        }
                    }
                }
            }
        }
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
        }

        let left_mouse_pressed = input.mouse_buttons_pressed.contains(&MouseButton::Left);
        let left_mouse_released = input.mouse_buttons_released.contains(&MouseButton::Left);
        let left_mouse_down = input.mouse_buttons_down.contains(&MouseButton::Left);

        self.process_keyboard_frame(state, &input, now);

        if left_mouse_pressed {
            let size = ctx.renderer.size();
            let debug_clicked =
                self.debug_hud
                    .handle_click(self.mouse_x, self.mouse_y, size.width, size.height);
            if debug_clicked {
                self.consume_next_mouse_up = true;
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

        self.update_skilltree_drag_from_frame(state, left_mouse_down);

        if left_mouse_released || (self.skilltree_cam_input.left_down && !left_mouse_down) {
            self.mouse_release_was_drag = self.skilltree_cam_input.drag_started;
            self.skilltree_cam_input.left_down = false;
            self.skilltree_cam_input.drag_started = false;
            self.skilltree_cam_input.drag_started_in_view = false;
        }

        let view = state.state().view;
        if view.is_tetris_playing() {
            if let Some(action) = self.horizontal_repeat.next_repeat_action(now) {
                apply_action(state, self.sfx.as_ref(), &mut self.debug_hud, action);
            }
        }

        let mut allow_ui = true;
        if left_mouse_released && self.consume_next_mouse_up {
            self.consume_next_mouse_up = false;
            allow_ui = false;
        }

        let mut ui_handled = false;
        if left_mouse_released && allow_ui {
            if let Some(action) = actions.first().copied() {
                ui_handled = self.handle_viewtree_action(state, action);
            }
        }

        if left_mouse_released && allow_ui && !ui_handled {
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
                    match action {
                        ACTION_SKILLTREE_START_RUN => {
                            let view = state.state().view;
                            let skilltree_editor_enabled = state.state().skilltree.editor.enabled;
                            if matches!(view, GameView::SkillTree) && !skilltree_editor_enabled {
                                let transition = headful_view::start_game(view);
                                state.state_mut().view = transition.next_view;
                                if transition.reset_tetris {
                                    reset_run(
                                        state,
                                        &self.base_logic,
                                        self.base_round_limit,
                                        self.base_gravity_interval,
                                        &mut self.horizontal_repeat,
                                    );
                                }
                                if let Some(sfx) = self.sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                                ui_handled = true;
                            }
                        }
                        ACTION_SKILLTREE_TOOL_SELECT
                        | ACTION_SKILLTREE_TOOL_MOVE
                        | ACTION_SKILLTREE_TOOL_ADD_CELL
                        | ACTION_SKILLTREE_TOOL_REMOVE_CELL
                        | ACTION_SKILLTREE_TOOL_LINK => {}
                        _ => {}
                    }
                }
            }
        }

        if left_mouse_released && allow_ui && !ui_handled {
            let view = state.state().view;
            if matches!(view, GameView::SkillTree) && !self.mouse_release_was_drag {
                let skilltree_editor_enabled = state.state().skilltree.editor.enabled;
                if skilltree_editor_enabled {
                    if let Some(world) = skilltree_world_cell_at_screen(
                        &state.state().skilltree,
                        self.last_skilltree,
                        self.mouse_x,
                        self.mouse_y,
                    ) {
                        let skilltree = &mut state.state_mut().skilltree;
                        let hit_id =
                            skilltree_node_at_world(skilltree, world).map(|s| s.to_string());

                        match skilltree.editor.tool {
                            SkillTreeEditorTool::Select => {
                                if let Some(id) = hit_id {
                                    skilltree.editor_select(&id, None);
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else {
                                    skilltree.editor_clear_selection();
                                }
                            }
                            SkillTreeEditorTool::Move => {
                                if let Some(id) = hit_id {
                                    if let Some(idx) = skilltree.node_index(&id) {
                                        let pos = skilltree.def.nodes[idx].pos;
                                        let grab = Vec2i::new(world.x - pos.x, world.y - pos.y);
                                        skilltree.editor_select(&id, Some(grab));
                                    } else {
                                        skilltree.editor_select(&id, None);
                                    }
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else {
                                    let grab = skilltree
                                        .editor
                                        .move_grab_offset
                                        .unwrap_or(Vec2i::new(0, 0));
                                    let new_pos = Vec2i::new(world.x - grab.x, world.y - grab.y);
                                    if skilltree.editor_move_selected_to(new_pos) {
                                        if let Some(sfx) = self.sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    }
                                }
                            }
                            SkillTreeEditorTool::AddCell => {
                                if let Some(id) = hit_id {
                                    let already =
                                        skilltree.editor.selected.as_deref() == Some(id.as_str());
                                    if !already {
                                        skilltree.editor_select(&id, None);
                                        if let Some(sfx) = self.sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    } else {
                                        if skilltree.editor_add_cell_at_world(world) {
                                            if let Some(sfx) = self.sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                        }
                                    }
                                }
                            }
                            SkillTreeEditorTool::RemoveCell => {
                                if let Some(id) = hit_id {
                                    if skilltree.editor.selected.as_deref() != Some(id.as_str()) {
                                        skilltree.editor_select(&id, None);
                                    }
                                    if skilltree.editor_remove_cell_at_world(world) {
                                        if let Some(sfx) = self.sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    }
                                }
                            }
                            SkillTreeEditorTool::ConnectPrereqs => {
                                if let Some(id) = hit_id {
                                    if let Some(from) = skilltree.editor.connect_from.clone() {
                                        if skilltree.editor_toggle_prereq(&from, &id) {
                                            if let Some(sfx) = self.sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                        }
                                    } else {
                                        skilltree.editor.connect_from = Some(id);
                                    }
                                }
                            }
                        }
                    }
                } else if let Some(world) = skilltree_world_cell_at_screen(
                    &state.state().skilltree,
                    self.last_skilltree,
                    self.mouse_x,
                    self.mouse_y,
                ) {
                    let hit_id = state.state().skilltree.def.nodes.iter().find_map(|node| {
                        node.shape.iter().find_map(|rel| {
                            let wx = node.pos.x + rel.x;
                            let wy = node.pos.y + rel.y;
                            if wx == world.x && wy == world.y {
                                Some(node.id.clone())
                            } else {
                                None
                            }
                        })
                    });
                    if let Some(id) = hit_id {
                        let transition = headful_view::open_skilltree(state.state().view);
                        state.state_mut().view = transition.next_view;
                        state.state_mut().skilltree.try_buy(&id);
                        if let Some(sfx) = self.sfx.as_ref() {
                            sfx.play_click(ACTION_SFX_VOLUME);
                        }
                    }
                }
            }
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
        };
        render_headful_frame(
            renderer,
            &mut self.ui_tree,
            &mut self.debug_hud,
            state,
            self.mouse_x,
            self.mouse_y,
            &mut cache,
            self.dig_camera.offset_y_px().round() as i32,
            self.last_frame_dt,
        );
        self.last_layout = cache.last_layout;
        self.last_main_menu = cache.last_main_menu;
        self.last_pause_menu = cache.last_pause_menu;
        self.last_skilltree = cache.last_skilltree;
        self.last_game_over_menu = cache.last_game_over_menu;
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

fn skilltree_world_cell_at_screen(
    skilltree: &SkillTreeRuntime,
    layout: SkillTreeLayout,
    sx: u32,
    sy: u32,
) -> Option<Vec2i> {
    headful_camera::skilltree_world_cell_at_screen(skilltree, layout, sx, sy)
}

fn skilltree_node_at_world<'a>(skilltree: &'a SkillTreeRuntime, world: Vec2i) -> Option<&'a str> {
    headful_camera::skilltree_node_at_world(skilltree, world)
}

const SKILLTREE_CAMERA_MIN_CELL_PX: f32 = headful_camera::SKILLTREE_CAMERA_MIN_CELL_PX;
const SKILLTREE_CAMERA_MAX_CELL_PX: f32 = headful_camera::SKILLTREE_CAMERA_MAX_CELL_PX;

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
        .with_score_bonus_per_line(tuning.score_bonus_per_line);
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
    frame: u64,
    chan: u16,
}

impl BgMusic {
    fn new() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame: 0,
            chan: 0,
        }
    }
}

impl Iterator for BgMusic {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // A tiny procedural "music" loop: an arpeggio with a basic envelope to avoid clicks.
        // This avoids shipping a large binary music asset while still giving the game background audio.
        const NOTES_HZ: [f32; 8] = [220.0, 261.63, 329.63, 261.63, 196.0, 246.94, 293.66, 246.94];

        let note_len_frames: u64 = (self.sample_rate as u64) / 4; // 0.25s per note @ 48kHz
        let note_i = ((self.frame / note_len_frames) % (NOTES_HZ.len() as u64)) as usize;
        let freq_hz = NOTES_HZ[note_i];

        let pos_in_note = self.frame % note_len_frames;
        let t = pos_in_note as f32 / self.sample_rate as f32;
        let phase = 2.0 * std::f32::consts::PI * freq_hz * t;

        let attack_frames: u64 = (self.sample_rate as u64) / 100; // 10ms
        let release_frames: u64 = (self.sample_rate as u64) / 40; // 25ms
        let release_start = note_len_frames.saturating_sub(release_frames);

        let env = if pos_in_note < attack_frames {
            pos_in_note as f32 / attack_frames.max(1) as f32
        } else if pos_in_note >= release_start {
            let remaining = note_len_frames.saturating_sub(pos_in_note);
            remaining as f32 / release_frames.max(1) as f32
        } else {
            1.0
        };

        let base = phase.sin();
        let harmonic = (phase * 2.0).sin() * 0.30;
        let sample = (base + harmonic) * 0.20 * env;

        // Advance in interleaved (stereo) sample space.
        self.chan += 1;
        if self.chan >= self.channels {
            self.chan = 0;
            self.frame = self.frame.wrapping_add(1);
        }

        Some(sample)
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

    fn toggle_music(&self) {
        let Some(sink) = self.music_sink.as_ref() else {
            return;
        };

        if self.music_playing.get() {
            sink.pause();
            self.music_playing.set(false);
        } else {
            sink.play();
            self.music_playing.set(true);
        }
    }
}

#[cfg(test)]
type HorizontalDir = headful_input::HorizontalDir;
type HorizontalRepeat = headful_input::HorizontalRepeat;
type DigCameraController = headful_dig_camera::DigCameraController;
#[cfg(test)]
type DigCameraConfig = headful_dig_camera::DigCameraConfig;

fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    headful_input::map_key_to_action(key)
}

fn should_play_action_sfx(action: InputAction) -> bool {
    headful_input::should_play_action_sfx(action)
}

fn apply_action(
    runner: &mut HeadlessRunner<TetrisLogic>,
    sfx: Option<&Sfx>,
    debug_hud: &mut DebugHud,
    action: InputAction,
) {
    let input_start = Instant::now();

    runner.step_profiled(action, debug_hud);

    if let Some(sfx) = sfx {
        if should_play_action_sfx(action) {
            sfx.play_click(ACTION_SFX_VOLUME);
        }
    }

    debug_hud.record_input(input_start.elapsed());
}

#[cfg(test)]
mod headful_tests;
