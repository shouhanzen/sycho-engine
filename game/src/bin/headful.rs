use std::{
    cell::Cell,
    io,
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use engine::HeadlessRunner;
use engine::app::{
    AppConfig,
    AppContext,
    GameApp,
    InputFrame,
    RecordingConfig,
    ProfileConfig,
    ReplayConfig,
    run_game,
    run_game_with_recording,
    run_game_with_profile,
    run_game_with_replay,
};
use engine::editor::{EditorSnapshot, EditorTimeline};
use engine::pixels_renderer::PixelsRenderer2d;
use engine::ui_tree::{UiEvent, UiInput, UiTree};
use engine::view_tree::hit_test_actions;
use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use game::debug::DebugHud;
use game::headful_editor_api::{RemoteCmd, RemoteServer};
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::state::{GameState, DEFAULT_GRAVITY_INTERVAL, DEFAULT_ROUND_LIMIT};
use game::skilltree::{
    clamp_camera_min_to_bounds, skilltree_world_bounds, SkillTreeEditorTool, SkillTreeRunMods,
    SkillTreeRuntime, Vec2f,
};
use game::sfx::{ACTION_SFX_VOLUME, MUSIC_VOLUME};
use game::tetris_core::{Piece, Vec2i};
use game::tetris_ui::{
    draw_game_over_menu_with_ui, draw_main_menu_with_ui, draw_pause_menu_with_ui,
    draw_skilltree_runtime_with_ui_and_mouse, draw_tetris_hud_view, draw_tetris_world,
    GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SkillTreeLayout, UiLayout,
};
use game::ui_ids::*;
use game::view::{GameView, GameViewEffect, GameViewEvent};
use game::view_tree::{
    build_hud_view_tree, build_menu_view_tree, build_skilltree_toolbar_view_tree, GameUiAction,
};

struct HeadfulApp {
    profiling: bool,
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
    last_frame: Instant,
    horizontal_repeat: HorizontalRepeat,
    target_fps: u32,
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
    let HeadfulCli {
        help,
        record,
        record_path: record_path_arg,
        replay_path,
    } = parse_headful_cli()?;
    if help {
        print_headful_help();
        return Ok(());
    }

    let replay_mode = replay_path.is_some();
    let record_path = record.then(|| record_path_arg.unwrap_or_else(default_recording_path));
    if let Some(path) = record_path.as_ref() {
        println!("state recording enabled: will save to {}", path.display());
    }
    if let Some(path) = replay_path.as_ref() {
        println!("replay: {}", path.display());
    }

    let profile_frames = env_usize("ROLLOUT_HEADFUL_PROFILE_FRAMES").unwrap_or(0);
    if replay_mode && profile_frames > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --replay with ROLLOUT_HEADFUL_PROFILE_FRAMES (profiling mode)",
        )
        .into());
    }
    let profiling = profile_frames > 0;

    let desired = if let (Some(w), Some(h)) =
        (env_u32("ROLLOUT_HEADFUL_PROFILE_WIDTH"), env_u32("ROLLOUT_HEADFUL_PROFILE_HEIGHT"))
    {
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

    let base_logic = TetrisLogic::new(0, Piece::all());
    let app = HeadfulApp::new(profiling, base_logic, DEFAULT_ROUND_LIMIT, DEFAULT_GRAVITY_INTERVAL);

    if let Some(path) = replay_path {
        run_game_with_replay(
            config,
            app,
            ReplayConfig {
                path,
                fps: 15,
            },
        )
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
        profiling: bool,
        base_logic: TetrisLogic,
        base_round_limit: Duration,
        base_gravity_interval: Duration,
    ) -> Self {
        let sfx = Sfx::new().ok();
        let target_fps: u32 = 60;
        let frame_interval = Duration::from_secs_f64(1.0 / (target_fps as f64));
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
            profiling,
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
            last_frame: Instant::now(),
            horizontal_repeat: HorizontalRepeat::default(),
            target_fps,
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
        state.view = if self.profiling {
            GameView::Tetris { paused: false }
        } else {
            GameView::MainMenu
        };
        state.skilltree = SkillTreeRuntime::load_default();
        state.round_timer = RoundTimer::new(self.base_round_limit);
        state.gravity_interval = self.base_gravity_interval;
        state.gravity_elapsed = Duration::ZERO;
        self.render_state = Some(runner.state().clone());
        runner
    }

    fn build_view(&self, state: &Self::State, ctx: &AppContext) -> engine::view_tree::ViewTree<Self::Action> {
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
        _ctx: &mut AppContext,
    ) -> Vec<Self::Effect> {
        self.last_frame_dt = dt;
        let now = Instant::now();

        if let Some(remote) = self.remote_editor_api.as_mut() {
            loop {
                let cmd = match remote.rx.try_recv() {
                    Ok(cmd) => cmd,
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                };

                let snapshot = |runner: &HeadlessRunner<TetrisLogic>| -> EditorSnapshot {
                    let frame = runner.frame();
                    game::editor_api::snapshot_from_state(frame, runner.state())
                };
                let timeline = |runner: &HeadlessRunner<TetrisLogic>| -> EditorTimeline {
                    let tm = runner.timemachine();
                    EditorTimeline {
                        frame: runner.frame(),
                        history_len: runner.history().len(),
                        can_rewind: tm.can_rewind(),
                        can_forward: tm.can_forward(),
                    }
                };

                match cmd {
                    RemoteCmd::GetState { respond } => {
                        let _ = respond.send(snapshot(state));
                    }
                    RemoteCmd::GetTimeline { respond } => {
                        let _ = respond.send(timeline(state));
                    }
                    RemoteCmd::Step { action_id, respond } => {
                        match game::editor_api::action_from_id(&action_id) {
                            Some(action) => {
                                state.step(action);
                                let _ = respond.send(Ok(snapshot(state)));
                            }
                            None => {
                                let _ = respond.send(Err(format!("unknown actionId: {action_id}")));
                            }
                        }
                    }
                    RemoteCmd::Rewind { frames, respond } => {
                        state.rewind(frames);
                        let _ = respond.send(snapshot(state));
                    }
                    RemoteCmd::Forward { frames, respond } => {
                        state.forward(frames);
                        let _ = respond.send(snapshot(state));
                    }
                    RemoteCmd::Seek { frame, respond } => {
                        state.seek(frame);
                        let _ = respond.send(snapshot(state));
                    }
                    RemoteCmd::Reset { respond } => {
                        reset_run(
                            state,
                            &self.base_logic,
                            self.base_round_limit,
                            self.base_gravity_interval,
                            &mut self.horizontal_repeat,
                        );
                        let _ = respond.send(snapshot(state));
                    }
                }
            }
        }

        let view = state.state().view;
        if view.is_tetris_playing() {
            if let Some(action) = self.horizontal_repeat.next_repeat_action(now) {
                apply_action(state, self.sfx.as_ref(), &mut self.debug_hud, action);
            }
        }

        let mut allow_ui = true;
        if input.mouse_up && self.consume_next_mouse_up {
            self.consume_next_mouse_up = false;
            allow_ui = false;
        }

        let mut ui_handled = false;
        if input.mouse_up && allow_ui {
            if let Some(action) = actions.first().copied() {
                match action {
                    GameUiAction::StartGame => {
                        let view = state.state().view;
                        if matches!(view, GameView::MainMenu) {
                            let (next, effect) = view.handle(GameViewEvent::StartGame);
                            state.state_mut().view = next;
                            if matches!(effect, GameViewEffect::ResetTetris) {
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
                    GameUiAction::OpenSkillTreeEditor => {
                        let view = state.state().view;
                        if matches!(view, GameView::MainMenu) {
                            let (next, _) = view.handle(GameViewEvent::OpenSkillTreeEditor);
                            state.state_mut().view = next;
                            self.horizontal_repeat.clear();
                            let skilltree = &mut state.state_mut().skilltree;
                            if !skilltree.editor.enabled {
                                skilltree.editor_toggle();
                            }
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::Quit => {
                        let view = state.state().view;
                        if matches!(view, GameView::MainMenu | GameView::GameOver) {
                            self.exit_requested = true;
                            ui_handled = true;
                        }
                    }
                    GameUiAction::PauseToggle => {
                        let view = state.state().view;
                        if view.is_tetris() {
                            let (next, _) = view.handle(GameViewEvent::TogglePause);
                            let state = state.state_mut();
                            state.view = next;
                            state.gravity_elapsed = Duration::ZERO;
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::Resume => {
                        let view = state.state().view;
                        if matches!(view, GameView::Tetris { paused: true }) {
                            let (next, _) = view.handle(GameViewEvent::TogglePause);
                            let state = state.state_mut();
                            state.view = next;
                            state.gravity_elapsed = Duration::ZERO;
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::EndRun => {
                        let view = state.state().view;
                        if matches!(view, GameView::Tetris { paused: true }) {
                            let earned = money_earned_from_run(state.state());
                            if earned > 0 {
                                state.state_mut().skilltree.add_money(earned);
                            }
                            let (next, _) = view.handle(GameViewEvent::GameOver);
                            state.state_mut().view = next;
                            self.horizontal_repeat.clear();
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
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
                            ui_handled = true;
                        }
                    }
                    GameUiAction::Restart => {
                        let view = state.state().view;
                        if matches!(view, GameView::GameOver) {
                            let (next, effect) = view.handle(GameViewEvent::StartGame);
                            state.state_mut().view = next;
                            if matches!(effect, GameViewEffect::ResetTetris) {
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
                    GameUiAction::OpenSkillTree => {
                        let view = state.state().view;
                        if matches!(view, GameView::GameOver) {
                            let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                            state.state_mut().view = next;
                            self.horizontal_repeat.clear();
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::SkillTreeToolSelect => {
                        let view = state.state().view;
                        let skilltree = &mut state.state_mut().skilltree;
                        if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                            skilltree.editor_set_tool(SkillTreeEditorTool::Select);
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::SkillTreeToolMove => {
                        let view = state.state().view;
                        let skilltree = &mut state.state_mut().skilltree;
                        if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                            skilltree.editor_set_tool(SkillTreeEditorTool::Move);
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::SkillTreeToolAddCell => {
                        let view = state.state().view;
                        let skilltree = &mut state.state_mut().skilltree;
                        if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                            skilltree.editor_set_tool(SkillTreeEditorTool::AddCell);
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::SkillTreeToolRemoveCell => {
                        let view = state.state().view;
                        let skilltree = &mut state.state_mut().skilltree;
                        if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                            skilltree.editor_set_tool(SkillTreeEditorTool::RemoveCell);
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                    GameUiAction::SkillTreeToolConnect => {
                        let view = state.state().view;
                        let skilltree = &mut state.state_mut().skilltree;
                        if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                            skilltree.editor_set_tool(SkillTreeEditorTool::ConnectPrereqs);
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                            ui_handled = true;
                        }
                    }
                }
            }
        }

        if input.mouse_up && allow_ui && !ui_handled {
            let ui_events = self.ui_tree.process_input(UiInput {
                mouse_pos: Some((self.mouse_x, self.mouse_y)),
                mouse_down: input.mouse_down,
                mouse_up: input.mouse_up,
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
                                let (next, effect) = view.handle(GameViewEvent::StartGame);
                                state.state_mut().view = next;
                                if matches!(effect, GameViewEffect::ResetTetris) {
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

        if input.mouse_up && allow_ui && !ui_handled {
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
                    let hit_id = state
                        .state()
                        .skilltree
                        .def
                        .nodes
                        .iter()
                        .find_map(|node| {
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
                        let (next, _) = state.state().view.handle(GameViewEvent::OpenSkillTree);
                        state.state_mut().view = next;
                        state.state_mut().skilltree.try_buy(&id);
                        if let Some(sfx) = self.sfx.as_ref() {
                            sfx.play_click(ACTION_SFX_VOLUME);
                        }
                    }
                }
            }
        }

        if input.mouse_up {
            self.mouse_release_was_drag = false;
        }

        if !self.profiling {
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
                    let (next, _) = state.view.handle(GameViewEvent::GameOver);
                    state.view = next;
                    state.gravity_elapsed = Duration::ZERO;
                    trigger_game_over = true;
                }
            }
            if trigger_game_over {
                self.horizontal_repeat.clear();
            }
        }

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

        let view = state.state().view;
        if matches!(view, GameView::SkillTree) {
            let skilltree = &mut state.state_mut().skilltree;
            let dt_s = dt.as_secs_f32();

            let scroll_y = self.skilltree_cam_input.pending_scroll_y;
            self.skilltree_cam_input.pending_scroll_y = 0.0;
            if scroll_y != 0.0 {
                let zoom_factor = 1.12f32.powf(scroll_y);
                skilltree.camera.target_cell_px = (skilltree.camera.target_cell_px * zoom_factor)
                    .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);

                if let Some(viewport) = skilltree_grid_viewport(self.last_skilltree) {
                    if viewport.contains(self.mouse_x, self.mouse_y) && self.last_skilltree.grid_cell > 0
                    {
                        let old_cell = self.last_skilltree.grid_cell.max(1) as f32;
                        let sx = self.mouse_x as f32 + 0.5;
                        let sy = self.mouse_y as f32 + 0.5;

                        let default_cam_min_x_old = -(self.last_skilltree.grid_cols as i32) / 2;
                        let cam_min_x_old = default_cam_min_x_old as f32 + skilltree.camera.pan.x;
                        let cam_min_y_old = skilltree.camera.pan.y;

                        let col_f = (sx - self.last_skilltree.grid_origin_x as f32) / old_cell;
                        let row_from_top_f =
                            (sy - self.last_skilltree.grid_origin_y as f32) / old_cell;
                        let world_x = cam_min_x_old + col_f;
                        let world_y = cam_min_y_old
                            + (self.last_skilltree.grid_rows as f32)
                            - row_from_top_f;

                        let grid_cell_new = skilltree
                            .camera
                            .target_cell_px
                            .round()
                            .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX)
                            as u32;
                        if grid_cell_new > 0 {
                            let grid = self.last_skilltree.grid;
                            let grid_cols_new = grid.w / grid_cell_new;
                            let grid_rows_new = grid.h / grid_cell_new;
                            if grid_cols_new > 0 && grid_rows_new > 0 {
                                let grid_pixel_w_new =
                                    grid_cols_new.saturating_mul(grid_cell_new);
                                let grid_pixel_h_new =
                                    grid_rows_new.saturating_mul(grid_cell_new);
                                let grid_origin_x_new = grid
                                    .x
                                    .saturating_add(grid.w.saturating_sub(grid_pixel_w_new) / 2);
                                let grid_origin_y_new = grid
                                    .y
                                    .saturating_add(grid.h.saturating_sub(grid_pixel_h_new) / 2);

                                let new_cell = grid_cell_new as f32;
                                let default_cam_min_x_new = -(grid_cols_new as i32) / 2;

                                let cam_min_x_new =
                                    world_x - (sx - grid_origin_x_new as f32) / new_cell;
                                let cam_min_y_new = world_y
                                    - (grid_rows_new as f32)
                                    + (sy - grid_origin_y_new as f32) / new_cell;

                                skilltree.camera.target_pan.x =
                                    cam_min_x_new - default_cam_min_x_new as f32;
                                skilltree.camera.target_pan.y = cam_min_y_new;

                                clamp_skilltree_camera_to_bounds(
                                    skilltree,
                                    grid_cols_new,
                                    grid_rows_new,
                                );
                            }
                        }
                    }
                }
            }

            if !self.skilltree_cam_input.left_down {
                if let Some(viewport) = skilltree_grid_viewport(self.last_skilltree) {
                    if viewport.contains(self.mouse_x, self.mouse_y) && self.last_skilltree.grid_cell > 0
                    {
                        let mx = self.mouse_x as f32;
                        let my = self.mouse_y as f32;
                        let x0 = viewport.x as f32;
                        let y0 = viewport.y as f32;
                        let x1 = (viewport.x.saturating_add(viewport.w)) as f32;
                        let y1 = (viewport.y.saturating_add(viewport.h)) as f32;

                        let margin = SKILLTREE_EDGE_PAN_MARGIN_PX.max(1.0);
                        let left = (mx - x0).max(0.0);
                        let right = (x1 - mx).max(0.0);
                        let top = (my - y0).max(0.0);
                        let bottom = (y1 - my).max(0.0);

                        let mut vx = 0.0f32;
                        let mut vy = 0.0f32;
                        if left < margin {
                            let t = 1.0 - left / margin;
                            vx -= t * t;
                        }
                        if right < margin {
                            let t = 1.0 - right / margin;
                            vx += t * t;
                        }
                        if top < margin {
                            let t = 1.0 - top / margin;
                            vy += t * t;
                        }
                        if bottom < margin {
                            let t = 1.0 - bottom / margin;
                            vy -= t * t;
                        }

                        if (vx != 0.0 || vy != 0.0) && dt_s > 0.0 {
                            let cell_px = (self.last_skilltree.grid_cell as f32).max(1.0);
                            let dx_cells = (vx * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s)
                                / cell_px;
                            let dy_cells = (vy * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s)
                                / cell_px;
                            skilltree.camera.target_pan.x += dx_cells;
                            skilltree.camera.target_pan.y += dy_cells;
                            clamp_skilltree_camera_to_bounds(
                                skilltree,
                                self.last_skilltree.grid_cols,
                                self.last_skilltree.grid_rows,
                            );
                        }
                    }
                }
            }

            skilltree.camera.target_cell_px = skilltree
                .camera
                .target_cell_px
                .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);
            skilltree.camera.pan.x = skilltree.camera.target_pan.x;
            skilltree.camera.pan.y = skilltree.camera.target_pan.y;
            skilltree.camera.cell_px = skilltree
                .camera
                .target_cell_px
                .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);
            clamp_skilltree_camera_to_bounds(
                skilltree,
                self.last_skilltree.grid_cols,
                self.last_skilltree.grid_rows,
            );
        }

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

        let frame_start = Instant::now();
        let board_start = Instant::now();
        let view = state.view;
        let board_dt = board_start.elapsed();

        let size = renderer.size();
        self.ui_tree.begin_frame();
        self.ui_tree
            .ensure_canvas(UI_CANVAS, Rect::from_size(size.width, size.height));
        self.ui_tree.add_root(UI_CANVAS);

        let draw_start = Instant::now();
        if matches!(view, GameView::SkillTree | GameView::MainMenu) {
            self.last_layout = UiLayout::default();
        } else {
            let tetris_layout = draw_tetris_world(renderer, size.width, size.height, state.tetris());
            if view.is_tetris() {
                draw_tetris_hud_view(
                    renderer,
                    size.width,
                    size.height,
                    state.tetris(),
                    tetris_layout,
                    Some((self.mouse_x, self.mouse_y)),
                );
            }
            self.last_layout = tetris_layout;
        }

        if view.is_tetris() {
            let hud_x = self.last_layout.pause_button.x.saturating_sub(180);
            let hud_y = self
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
                self.last_main_menu = draw_main_menu_with_ui(
                    renderer,
                    size.width,
                    size.height,
                    &mut self.ui_tree,
                );
                self.last_pause_menu = PauseMenuLayout::default();
                self.last_skilltree = SkillTreeLayout::default();
                self.last_game_over_menu = GameOverMenuLayout::default();
            }
            GameView::SkillTree => {
                self.last_main_menu = MainMenuLayout::default();
                self.last_pause_menu = PauseMenuLayout::default();
                self.last_skilltree = draw_skilltree_runtime_with_ui_and_mouse(
                    renderer,
                    size.width,
                    size.height,
                    &mut self.ui_tree,
                    &state.skilltree,
                    Some((self.mouse_x, self.mouse_y)),
                );
                self.last_game_over_menu = GameOverMenuLayout::default();
            }
            GameView::Tetris { paused: true } => {
                self.last_main_menu = MainMenuLayout::default();
                self.last_pause_menu = draw_pause_menu_with_ui(
                    renderer,
                    size.width,
                    size.height,
                    &mut self.ui_tree,
                );
                self.last_skilltree = SkillTreeLayout::default();
                self.last_game_over_menu = GameOverMenuLayout::default();
            }
            GameView::Tetris { paused: false } => {
                self.last_main_menu = MainMenuLayout::default();
                self.last_pause_menu = PauseMenuLayout::default();
                self.last_skilltree = SkillTreeLayout::default();
                self.last_game_over_menu = GameOverMenuLayout::default();
            }
            GameView::GameOver => {
                self.last_main_menu = MainMenuLayout::default();
                self.last_pause_menu = PauseMenuLayout::default();
                self.last_skilltree = SkillTreeLayout::default();
                self.last_game_over_menu = draw_game_over_menu_with_ui(
                    renderer,
                    size.width,
                    size.height,
                    &mut self.ui_tree,
                );
            }
        }
        self.debug_hud
            .draw_overlay(renderer, size.width, size.height);
        let overlay_dt = overlay_start.elapsed();

        let present_start = Instant::now();
        let present_dt = Duration::ZERO;
        let frame_total_dt = frame_start.elapsed();
        self.debug_hud.on_frame(
            self.last_frame_dt,
            board_dt,
            draw_dt,
            overlay_dt,
            present_dt,
            frame_total_dt,
        );

    }

    fn handle_event(
        &mut self,
        event: &Event<()>,
        runner: &mut Self::State,
        _input: &mut InputFrame,
        ctx: &mut AppContext,
        control_flow: &mut ControlFlow,
    ) -> bool {
        *control_flow = ControlFlow::WaitUntil(self.next_redraw);

        if self.exit_requested {
            *control_flow = ControlFlow::Exit;
            return true;
        }

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return true;
                }
                WindowEvent::Focused(false) => {
                    self.horizontal_repeat.clear();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let new_x = position.x.max(0.0) as u32;
                    let new_y = position.y.max(0.0) as u32;

                    let view = runner.state().view;
                    if matches!(view, GameView::SkillTree)
                        && self.skilltree_cam_input.left_down
                        && self.skilltree_cam_input.drag_started_in_view
                        && self.last_skilltree.grid_cell > 0
                    {
                        let dx = new_x as i32 - self.skilltree_cam_input.last_x as i32;
                        let dy = new_y as i32 - self.skilltree_cam_input.last_y as i32;

                        if !self.skilltree_cam_input.drag_started {
                            let total_dx = new_x as f32 - self.skilltree_cam_input.down_x as f32;
                            let total_dy = new_y as f32 - self.skilltree_cam_input.down_y as f32;
                            if total_dx * total_dx + total_dy * total_dy
                                >= SKILLTREE_DRAG_THRESHOLD_PX * SKILLTREE_DRAG_THRESHOLD_PX
                            {
                                self.skilltree_cam_input.drag_started = true;
                            }
                        }

                        if self.skilltree_cam_input.drag_started {
                            let mut cam_min = Vec2f::new(
                                self.last_skilltree.grid_cam_min_x as f32,
                                self.last_skilltree.grid_cam_min_y as f32,
                            );
                            cam_min.x -= dx as f32 / self.last_skilltree.grid_cell as f32;
                            cam_min.y += dy as f32 / self.last_skilltree.grid_cell as f32;

                            let view_size_cells = Vec2f::new(
                                self.last_skilltree.grid_cols as f32,
                                self.last_skilltree.grid_rows as f32,
                            );
                            if let Some(bounds) = skilltree_world_bounds(&runner.state().skilltree.def)
                            {
                                cam_min = clamp_camera_min_to_bounds(
                                    cam_min,
                                    view_size_cells,
                                    bounds,
                                    SKILLTREE_CAMERA_BOUNDS_PAD_CELLS,
                                );
                            }

                            let default_cam_min_x = -(self.last_skilltree.grid_cols as i32) / 2;
                            let default_cam_min_y = 0i32;
                            let cam = &mut runner.state_mut().skilltree.camera;
                            cam.pan.x = cam_min.x - default_cam_min_x as f32;
                            cam.pan.y = cam_min.y - default_cam_min_y as f32;
                            cam.target_pan = cam.pan;
                        }

                        self.skilltree_cam_input.last_x = new_x;
                        self.skilltree_cam_input.last_y = new_y;
                    }

                    self.mouse_x = new_x;
                    self.mouse_y = new_y;
                    let _ = self.ui_tree.process_input(UiInput {
                        mouse_pos: Some((self.mouse_x, self.mouse_y)),
                        mouse_down: false,
                        mouse_up: false,
                    });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    let view = runner.state().view;
                    if !matches!(view, GameView::SkillTree) {
                        return false;
                    }
                    let scroll_y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => *y,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y as f32) / 120.0,
                    };
                    self.skilltree_cam_input.pending_scroll_y += scroll_y;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    let size = ctx.renderer.size();
                    if self
                        .debug_hud
                        .handle_click(self.mouse_x, self.mouse_y, size.width, size.height)
                    {
                        self.consume_next_mouse_up = true;
                        return true;
                    }
                    let _ = self.ui_tree.process_input(UiInput {
                        mouse_pos: Some((self.mouse_x, self.mouse_y)),
                        mouse_down: true,
                        mouse_up: false,
                    });
                    let view = runner.state().view;
                    if matches!(view, GameView::SkillTree) {
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
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    self.mouse_release_was_drag = self.skilltree_cam_input.drag_started;
                    self.skilltree_cam_input.left_down = false;
                    self.skilltree_cam_input.drag_started = false;
                    self.skilltree_cam_input.drag_started_in_view = false;
                }
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            state: key_state,
                            virtual_keycode: Some(key),
                            ..
                        },
                    ..
                } => match key_state {
                    ElementState::Pressed => {
                        if *key == VirtualKeyCode::F3 {
                            self.debug_hud.toggle();
                        }
                        if *key == VirtualKeyCode::M {
                            if let Some(sfx) = self.sfx.as_ref() {
                                sfx.toggle_music();
                            }
                            return true;
                        }

                        let mut view = runner.state().view;
                        match view {
                            GameView::MainMenu => {
                                if *key == VirtualKeyCode::Return || *key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            runner,
                                            &self.base_logic,
                                            self.base_round_limit,
                                            self.base_gravity_interval,
                                            &mut self.horizontal_repeat,
                                        );
                                    }
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if *key == VirtualKeyCode::K {
                                    let (next, _) = view.handle(GameViewEvent::OpenSkillTreeEditor);
                                    view = next;
                                    runner.state_mut().view = view;
                                    self.horizontal_repeat.clear();
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if !skilltree.editor.enabled {
                                        skilltree.editor_toggle();
                                    }
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if *key == VirtualKeyCode::Escape {
                                    *control_flow = ControlFlow::Exit;
                                }
                                return true;
                            }
                            GameView::SkillTree => {
                                if *key == VirtualKeyCode::F4 {
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    skilltree.editor_toggle();
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                    return true;
                                }

                                let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
                                if skilltree_editor_enabled {
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    match *key {
                                        VirtualKeyCode::Escape => {
                                            skilltree.editor_toggle();
                                            let (next, _) = view.handle(GameViewEvent::Back);
                                            view = next;
                                            runner.state_mut().view = view;
                                            self.horizontal_repeat.clear();
                                            if let Some(sfx) = self.sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                        }
                                        VirtualKeyCode::Tab => {
                                            skilltree.editor_cycle_tool();
                                        }
                                        VirtualKeyCode::Left => {
                                            skilltree.camera.pan.x -= 1.0;
                                            skilltree.camera.target_pan = skilltree.camera.pan;
                                        }
                                        VirtualKeyCode::Right => {
                                            skilltree.camera.pan.x += 1.0;
                                            skilltree.camera.target_pan = skilltree.camera.pan;
                                        }
                                        VirtualKeyCode::Up => {
                                            skilltree.camera.pan.y += 1.0;
                                            skilltree.camera.target_pan = skilltree.camera.pan;
                                        }
                                        VirtualKeyCode::Down => {
                                            skilltree.camera.pan.y -= 1.0;
                                            skilltree.camera.target_pan = skilltree.camera.pan;
                                        }
                                        VirtualKeyCode::Minus => {
                                            skilltree.camera.cell_px = (skilltree.camera.cell_px - 2.0)
                                                .max(SKILLTREE_CAMERA_MIN_CELL_PX);
                                            skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                                        }
                                        VirtualKeyCode::Equals => {
                                            skilltree.camera.cell_px = (skilltree.camera.cell_px + 2.0)
                                                .min(SKILLTREE_CAMERA_MAX_CELL_PX);
                                            skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                                        }
                                        VirtualKeyCode::N => {
                                            if let Some(world) = skilltree_world_cell_at_screen(
                                                skilltree,
                                                self.last_skilltree,
                                                self.mouse_x,
                                                self.mouse_y,
                                            ) {
                                                skilltree.editor_create_node_at(world);
                                            }
                                        }
                                        VirtualKeyCode::Delete => {
                                            let _ = skilltree.editor_delete_selected();
                                        }
                                        VirtualKeyCode::S => {
                                            match skilltree.save_def() {
                                                Ok(()) => {
                                                    skilltree.editor.dirty = false;
                                                    skilltree.editor.status = Some("SAVED".to_string());
                                                }
                                                Err(e) => {
                                                    skilltree.editor.status =
                                                        Some(format!("SAVE FAILED: {e}"));
                                                }
                                            }
                                        }
                                        VirtualKeyCode::R => {
                                            skilltree.reload_def();
                                            skilltree.editor.dirty = false;
                                            skilltree.editor.status = Some("RELOADED".to_string());
                                        }
                                        _ => {}
                                    }
                                    return true;
                                }

                                if *key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::Back);
                                    view = next;
                                    runner.state_mut().view = view;
                                    self.horizontal_repeat.clear();
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if *key == VirtualKeyCode::Return || *key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            runner,
                                            &self.base_logic,
                                            self.base_round_limit,
                                            self.base_gravity_interval,
                                            &mut self.horizontal_repeat,
                                        );
                                    }
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                }
                                return true;
                            }
                            GameView::GameOver => {
                                if *key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::Back);
                                    view = next;
                                    runner.state_mut().view = view;
                                    self.horizontal_repeat.clear();
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if *key == VirtualKeyCode::Return || *key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            runner,
                                            &self.base_logic,
                                            self.base_round_limit,
                                            self.base_gravity_interval,
                                            &mut self.horizontal_repeat,
                                        );
                                    }
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if *key == VirtualKeyCode::K {
                                    let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                                    view = next;
                                    runner.state_mut().view = view;
                                    self.horizontal_repeat.clear();
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                }
                                return true;
                            }
                            GameView::Tetris { paused } => {
                                if *key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::TogglePause);
                                    view = next;
                                    {
                                        let state = runner.state_mut();
                                        state.view = view;
                                        state.gravity_elapsed = Duration::ZERO;
                                    }
                                    self.horizontal_repeat.clear();
                                    if let Some(sfx) = self.sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                    return true;
                                }

                                if paused {
                                    return true;
                                }
                            }
                        }

                        let horizontal = match *key {
                            VirtualKeyCode::Left => Some(HorizontalDir::Left),
                            VirtualKeyCode::Right | VirtualKeyCode::D => Some(HorizontalDir::Right),
                            _ => None,
                        };

                        if let Some(dir) = horizontal {
                            if self.horizontal_repeat.on_press(dir, now) {
                                apply_action(
                                    runner,
                                    self.sfx.as_ref(),
                                    &mut self.debug_hud,
                                    match dir {
                                        HorizontalDir::Left => InputAction::MoveLeft,
                                        HorizontalDir::Right => InputAction::MoveRight,
                                    },
                                );
                            }
                            return true;
                        }

                        if let Some(action) = map_key_to_action(*key) {
                            apply_action(runner, self.sfx.as_ref(), &mut self.debug_hud, action);
                        }
                        return true;
                    }
                    ElementState::Released => {
                        let view = runner.state().view;
                        if !view.is_tetris() {
                            return true;
                        }
                        let now = Instant::now();
                        match key {
                            VirtualKeyCode::Left => {
                                self.horizontal_repeat.on_release(HorizontalDir::Left, now)
                            }
                            VirtualKeyCode::Right | VirtualKeyCode::D => {
                                self.horizontal_repeat.on_release(HorizontalDir::Right, now)
                            }
                            _ => {}
                        }
                        return true;
                    }
                },
                _ => {}
            },
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
    // Simple, deterministic conversion from in-run performance to meta-currency.
    // Tunable later; for now it makes the buy-loop visible quickly.
    let score = state.tetris.score();
    let lines = state.tetris.lines_cleared();
    score / 10 + lines.saturating_mul(5)
}

fn skilltree_world_cell_at_screen(
    skilltree: &SkillTreeRuntime,
    layout: SkillTreeLayout,
    sx: u32,
    sy: u32,
) -> Option<Vec2i> {
    let view = skilltree_grid_viewport(layout)?;
    if !view.contains(sx, sy) {
        return None;
    }

    let cell = layout.grid_cell as f32;
    if cell <= 0.0 {
        return None;
    }

    // Camera min in world cells (float).
    let default_cam_min_x = -(layout.grid_cols as i32) / 2;
    let cam_min_x = default_cam_min_x as f32 + skilltree.camera.pan.x;
    let cam_min_y = skilltree.camera.pan.y;

    // Pixel centers (avoid boundary edge-cases).
    let sx = sx as f32 + 0.5;
    let sy = sy as f32 + 0.5;

    let col_f = (sx - layout.grid_origin_x as f32) / cell;
    let row_from_top_f = (sy - layout.grid_origin_y as f32) / cell;

    let world_x = cam_min_x + col_f;
    let world_y = cam_min_y + (layout.grid_rows as f32) - row_from_top_f;

    Some(Vec2i::new(world_x.floor() as i32, world_y.floor() as i32))
}

fn skilltree_node_at_world<'a>(skilltree: &'a SkillTreeRuntime, world: Vec2i) -> Option<&'a str> {
    for node in &skilltree.def.nodes {
        for rel in &node.shape {
            let wx = node.pos.x + rel.x;
            let wy = node.pos.y + rel.y;
            if wx == world.x && wy == world.y {
                return Some(node.id.as_str());
            }
        }
    }
    None
}

const SKILLTREE_CAMERA_MIN_CELL_PX: f32 = 8.0;
const SKILLTREE_CAMERA_MAX_CELL_PX: f32 = 64.0;
const SKILLTREE_CAMERA_PAN_LERP_RATE: f32 = 18.0; // 1/s
const SKILLTREE_CAMERA_ZOOM_LERP_RATE: f32 = 16.0; // 1/s
const SKILLTREE_EDGE_PAN_MARGIN_PX: f32 = 28.0;
const SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S: f32 = 900.0;
const SKILLTREE_DRAG_THRESHOLD_PX: f32 = 4.0;
const SKILLTREE_CAMERA_BOUNDS_PAD_CELLS: f32 = 6.0;

#[derive(Debug, Default, Clone, Copy)]
struct SkillTreeCameraInput {
    left_down: bool,
    drag_started: bool,
    drag_started_in_view: bool,
    down_x: u32,
    down_y: u32,
    last_x: u32,
    last_y: u32,
    pending_scroll_y: f32,
}

fn lerp_f32(current: f32, target: f32, t: f32) -> f32 {
    current + (target - current) * t.clamp(0.0, 1.0)
}

fn smooth_t_from_rate(dt_s: f32, rate: f32) -> f32 {
    if dt_s <= 0.0 || rate <= 0.0 {
        return 0.0;
    }
    // Exponential smoothing factor (frame-rate independent).
    1.0 - (-dt_s * rate).exp()
}

fn skilltree_grid_viewport(layout: SkillTreeLayout) -> Option<Rect> {
    if layout.grid_cell == 0 || layout.grid_cols == 0 || layout.grid_rows == 0 {
        return None;
    }
    let w = layout.grid_cols.saturating_mul(layout.grid_cell);
    let h = layout.grid_rows.saturating_mul(layout.grid_cell);
    if w == 0 || h == 0 {
        return None;
    }
    Some(Rect::new(layout.grid_origin_x, layout.grid_origin_y, w, h))
}

fn clamp_skilltree_camera_to_bounds(skilltree: &mut SkillTreeRuntime, grid_cols: u32, grid_rows: u32) {
    if grid_cols == 0 || grid_rows == 0 {
        return;
    }
    let Some(bounds) = skilltree_world_bounds(&skilltree.def) else {
        return;
    };

    let default_cam_min_x = -(grid_cols as i32) / 2;
    let default_cam_min_y = 0i32;
    let view = Vec2f::new(grid_cols as f32, grid_rows as f32);

    let cam_min_target = Vec2f::new(
        default_cam_min_x as f32 + skilltree.camera.target_pan.x,
        default_cam_min_y as f32 + skilltree.camera.target_pan.y,
    );
    let cam_min_target =
        clamp_camera_min_to_bounds(cam_min_target, view, bounds, SKILLTREE_CAMERA_BOUNDS_PAD_CELLS);
    skilltree.camera.target_pan.x = cam_min_target.x - default_cam_min_x as f32;
    skilltree.camera.target_pan.y = cam_min_target.y - default_cam_min_y as f32;

    let cam_min = Vec2f::new(
        default_cam_min_x as f32 + skilltree.camera.pan.x,
        default_cam_min_y as f32 + skilltree.camera.pan.y,
    );
    let cam_min = clamp_camera_min_to_bounds(cam_min, view, bounds, SKILLTREE_CAMERA_BOUNDS_PAD_CELLS);
    skilltree.camera.pan.x = cam_min.x - default_cam_min_x as f32;
    skilltree.camera.pan.y = cam_min.y - default_cam_min_y as f32;
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

fn run_tuning_from_mods(base_round_limit: Duration, base_gravity_interval: Duration, mods: SkillTreeRunMods) -> RunTuning {
    RunTuning {
        round_limit: base_round_limit.saturating_add(Duration::from_secs(mods.extra_round_time_seconds as u64)),
        gravity_interval: gravity_interval_from_percent(base_gravity_interval, mods.gravity_faster_percent),
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
    std::env::var(name).ok().and_then(|v| v.parse::<usize>().ok())
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.parse::<u32>().ok())
}

fn env_u16(name: &str) -> Option<u16> {
    std::env::var(name).ok().and_then(|v| v.parse::<u16>().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name).ok().and_then(|v| match v.to_ascii_lowercase().as_str() {
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

#[derive(Debug, Default, Clone)]
struct HeadfulCli {
    help: bool,
    record: bool,
    record_path: Option<PathBuf>,
    replay_path: Option<PathBuf>,
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

fn default_recording_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    PathBuf::from("target")
        .join("recordings")
        .join(format!("headful_{nanos}.json"))
}

fn parse_headful_cli() -> io::Result<HeadfulCli> {
    let mut cli = HeadfulCli::default();
    let mut args = std::env::args().skip(1).peekable();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                cli.help = true;
            }
            "--record" => {
                cli.record = true;
                if let Some(next) = args.peek() {
                    if !next.starts_with("--") {
                        cli.record_path = Some(PathBuf::from(
                            args.next().expect("peeked Some means next() is Some"),
                        ));
                    }
                }
            }
            "--replay" => {
                let Some(path) = args.next() else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--replay requires a path",
                    ));
                };
                cli.replay_path = Some(PathBuf::from(path));
            }
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {other} (try --help)"),
                ));
            }
        }
    }

    if cli.record && cli.replay_path.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --record and --replay",
        ));
    }

    Ok(cli)
}

#[derive(Debug, Clone, Copy)]
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
            click_wav: include_bytes!("../../assets/sfx/click.wav"),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HorizontalDir {
    Left,
    Right,
}

#[derive(Debug, Default)]
struct HorizontalRepeat {
    left_down: bool,
    right_down: bool,
    active: Option<HorizontalDir>,
    next_repeat_at: Option<Instant>,
}

impl HorizontalRepeat {
    // Roughly "DAS/ARR"-ish defaults, but the key property is that repeating is driven by our
    // own timer, not the OS key-repeat (so it won't get interrupted by other keypresses).
    const REPEAT_DELAY: Duration = Duration::from_millis(170);
    const REPEAT_INTERVAL: Duration = Duration::from_millis(50);

    fn clear(&mut self) {
        self.left_down = false;
        self.right_down = false;
        self.active = None;
        self.next_repeat_at = None;
    }

    fn on_press(&mut self, dir: HorizontalDir, now: Instant) -> bool {
        let was_down = match dir {
            HorizontalDir::Left => self.left_down,
            HorizontalDir::Right => self.right_down,
        };
        if was_down {
            // Ignore OS key-repeat "Pressed" events; repeating is handled by `next_repeat_action`.
            return false;
        }

        match dir {
            HorizontalDir::Left => self.left_down = true,
            HorizontalDir::Right => self.right_down = true,
        }

        self.active = Some(dir);
        self.next_repeat_at = Some(now + Self::REPEAT_DELAY);
        true
    }

    fn on_release(&mut self, dir: HorizontalDir, now: Instant) {
        match dir {
            HorizontalDir::Left => self.left_down = false,
            HorizontalDir::Right => self.right_down = false,
        }

        if self.active != Some(dir) {
            return;
        }

        // If the active direction was released, fall back to the other one if still held.
        let new_active = match dir {
            HorizontalDir::Left if self.right_down => Some(HorizontalDir::Right),
            HorizontalDir::Right if self.left_down => Some(HorizontalDir::Left),
            _ => None,
        };

        self.active = new_active;
        self.next_repeat_at = new_active.map(|_| now + Self::REPEAT_DELAY);
    }

    fn next_repeat_action(&mut self, now: Instant) -> Option<InputAction> {
        let dir = self.active?;
        let next_at = self.next_repeat_at?;
        if now < next_at {
            return None;
        }

        self.next_repeat_at = Some(now + Self::REPEAT_INTERVAL);
        Some(match dir {
            HorizontalDir::Left => InputAction::MoveLeft,
            HorizontalDir::Right => InputAction::MoveRight,
        })
    }
}

fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    match key {
        VirtualKeyCode::Left => Some(InputAction::MoveLeft),
        VirtualKeyCode::Right | VirtualKeyCode::D => Some(InputAction::MoveRight),
        VirtualKeyCode::Down | VirtualKeyCode::S => Some(InputAction::SoftDrop),
        VirtualKeyCode::Up | VirtualKeyCode::W => Some(InputAction::RotateCw),
        VirtualKeyCode::Z => Some(InputAction::RotateCcw),
        VirtualKeyCode::X => Some(InputAction::RotateCw),
        VirtualKeyCode::A => Some(InputAction::Rotate180),
        VirtualKeyCode::Space => Some(InputAction::HardDrop),
        VirtualKeyCode::C => Some(InputAction::Hold),
        _ => None,
    }
}


fn should_play_action_sfx(action: InputAction) -> bool {
    // Gameplay actions happen very frequently; only hard drop gets a click SFX.
    matches!(action, InputAction::HardDrop)
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
