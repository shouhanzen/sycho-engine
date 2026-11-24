use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fs;
use std::hash::Hash;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use serde::{Serialize, de::DeserializeOwned};

use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::dpi::PhysicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use crate::embedded::{
    EditorEvent, GameLink, PREVIEW_BYTES, PREVIEW_HEIGHT, PREVIEW_WIDTH, TimelineStatus,
};
use crate::graphics::{CpuRenderer, Renderer2d};
use crate::pixels_renderer::PixelsRenderer2d;
use crate::reload::{self, ProcessCommand, RELOAD_EXIT_CODE};
use crate::surface::SurfaceSize;
use crate::timeline::{Frame, StateSchema, Timeline, TimelineCommand, TimelineWorker};
use crate::ui_tree::UiInput;
use crate::view_tree::{ViewTree, hit_test_actions};

pub struct AppConfig {
    pub title: String,
    pub desired_size: PhysicalSize<u32>,
    pub clamp_to_monitor: bool,
    pub vsync: Option<bool>,
    pub present_mode: Option<pixels::wgpu::PresentMode>,
}

#[derive(Debug, Clone)]
pub struct TimelineConfig {
    pub directory: PathBuf,
    pub checkpoint_interval: Duration,
    pub playback: bool,
}

#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub target_frames: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineCli {
    pub help: bool,
    pub timeline_path: Option<PathBuf>,
    pub fresh: bool,
    pub playback: bool,
    pub export_video: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Normal,
    Timeline,
    Profile,
}

pub fn parse_timeline_cli() -> io::Result<TimelineCli> {
    let mut cli = TimelineCli::default();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        match arg.as_ref() {
            "-h" | "--help" => cli.help = true,
            "--fresh" => cli.fresh = true,
            "--playback" => cli.playback = true,
            "--timeline" | "--export-video" => {
                let path = args
                    .next()
                    .filter(|path| !path.to_string_lossy().starts_with('-'))
                    .map(PathBuf::from)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{arg} requires a path"),
                        )
                    })?;
                if arg == "--timeline" {
                    cli.timeline_path = Some(path);
                } else {
                    cli.export_video = Some(path);
                }
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown argument: {arg} (try --help)"),
                ));
            }
        }
    }
    if (cli.fresh && cli.playback) || (cli.export_video.is_some() && (cli.fresh || cli.playback)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--fresh, --playback, and --export-video are mutually exclusive",
        ));
    }
    Ok(cli)
}

/// The game-facing context shared by windowed and embedded rendering.
pub struct AppContext {
    pub surface_size: SurfaceSize,
}

/// Native window and presentation state for host-level applications.
pub struct WindowContext {
    pub window: Window,
    pub renderer: PixelsRenderer2d,
    pub app: AppContext,
}

impl WindowContext {
    fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        let size = SurfaceSize::new(size.width, size.height);
        self.app.surface_size = size;
        if let Err(err) = self.renderer.resize(size) {
            eprintln!("resize failed: {err}");
        }
        self.window.request_redraw();
    }
}

#[derive(Debug, Clone)]
pub struct InputFrame {
    pub mouse_pos: Option<(u32, u32)>,
    pub mouse_down: bool,
    pub mouse_up: bool,
    pub mouse_buttons_down: HashSet<MouseButton>,
    pub mouse_buttons_pressed: HashSet<MouseButton>,
    pub mouse_buttons_released: HashSet<MouseButton>,
    pub keys_down: HashSet<VirtualKeyCode>,
    pub keys_pressed: HashSet<VirtualKeyCode>,
    pub keys_released: HashSet<VirtualKeyCode>,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub window_focused: bool,
}

impl Default for InputFrame {
    fn default() -> Self {
        Self {
            mouse_pos: None,
            mouse_down: false,
            mouse_up: false,
            mouse_buttons_down: HashSet::new(),
            mouse_buttons_pressed: HashSet::new(),
            mouse_buttons_released: HashSet::new(),
            keys_down: HashSet::new(),
            keys_pressed: HashSet::new(),
            keys_released: HashSet::new(),
            scroll_x: 0.0,
            scroll_y: 0.0,
            // Avoid gating controls before the OS sends an initial focus event.
            window_focused: true,
        }
    }
}

impl InputFrame {
    fn apply_key_state(&mut self, key: VirtualKeyCode, state: ElementState) {
        apply_button_transition(
            &mut self.keys_down,
            &mut self.keys_pressed,
            &mut self.keys_released,
            key,
            state,
        );
    }

    fn apply_mouse_button_state(&mut self, button: MouseButton, state: ElementState) {
        let was_down = self.mouse_buttons_down.contains(&button);
        apply_button_transition(
            &mut self.mouse_buttons_down,
            &mut self.mouse_buttons_pressed,
            &mut self.mouse_buttons_released,
            button,
            state,
        );

        if button == MouseButton::Left {
            if !was_down && matches!(state, ElementState::Pressed) {
                self.mouse_down = true;
            } else if was_down && matches!(state, ElementState::Released) {
                self.mouse_up = true;
            }
        }
    }

    fn apply_scroll_delta(&mut self, delta: &MouseScrollDelta) {
        match delta {
            MouseScrollDelta::LineDelta(x, y) => {
                self.scroll_x += *x;
                self.scroll_y += *y;
            }
            MouseScrollDelta::PixelDelta(pos) => {
                self.scroll_x += (pos.x as f32) / 120.0;
                self.scroll_y += (pos.y as f32) / 120.0;
            }
        }
    }

    fn set_window_focus(&mut self, focused: bool) {
        self.window_focused = focused;
        if !focused {
            self.keys_down.clear();
            self.keys_pressed.clear();
            self.keys_released.clear();
            self.mouse_buttons_down.clear();
            self.mouse_buttons_pressed.clear();
            self.mouse_buttons_released.clear();
            self.mouse_down = false;
            self.mouse_up = false;
        }
    }

    fn apply_editor_event(&mut self, event: EditorEvent) {
        match event {
            EditorEvent::Key(key, state) => self.apply_key_state(key, state),
            EditorEvent::Button(button, state) => self.apply_mouse_button_state(button, state),
            EditorEvent::Pointer(position) => self.mouse_pos = position,
            EditorEvent::Scroll(x, y) => {
                self.scroll_x += x;
                self.scroll_y += y;
            }
            EditorEvent::Focus(focused) => self.set_window_focus(focused),
            EditorEvent::ReleaseInput => {
                let focused = self.window_focused;
                self.set_window_focus(false);
                self.window_focused = focused;
                self.mouse_pos = None;
            }
            EditorEvent::Timeline(_) => {}
        }
    }

    fn clear_frame_transients(&mut self) {
        self.mouse_down = false;
        self.mouse_up = false;
        self.mouse_buttons_pressed.clear();
        self.mouse_buttons_released.clear();
        self.keys_pressed.clear();
        self.keys_released.clear();
        self.scroll_x = 0.0;
        self.scroll_y = 0.0;
    }
}

fn apply_button_transition<T>(
    down: &mut HashSet<T>,
    pressed: &mut HashSet<T>,
    released: &mut HashSet<T>,
    button: T,
    state: ElementState,
) where
    T: Copy + Eq + Hash,
{
    match state {
        ElementState::Pressed => {
            if down.insert(button) {
                pressed.insert(button);
            }
        }
        ElementState::Released => {
            if down.remove(&button) {
                released.insert(button);
            }
        }
    }
}

fn apply_window_event_to_input(input: &mut InputFrame, event: &WindowEvent) {
    match event {
        WindowEvent::CursorMoved { position, .. } => {
            let new_x = position.x.max(0.0) as u32;
            let new_y = position.y.max(0.0) as u32;
            input.mouse_pos = Some((new_x, new_y));
        }
        WindowEvent::MouseInput {
            state: mouse_state,
            button,
            ..
        } => {
            input.apply_mouse_button_state(*button, *mouse_state);
        }
        WindowEvent::KeyboardInput {
            input:
                KeyboardInput {
                    state: key_state,
                    virtual_keycode: Some(key),
                    ..
                },
            ..
        } => {
            input.apply_key_state(*key, *key_state);
        }
        WindowEvent::MouseWheel { delta, .. } => {
            input.apply_scroll_delta(delta);
        }
        WindowEvent::Focused(focused) => {
            input.set_window_focus(*focused);
        }
        _ => {}
    }
}

pub trait GameApp {
    type State;
    type Action: Clone;
    type Effect;

    fn init_state(&mut self, _ctx: &mut AppContext) -> Self::State;

    fn build_view(&self, state: &Self::State, _ctx: &AppContext) -> ViewTree<Self::Action>;

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: InputFrame,
        dt: Duration,
        actions: &[Self::Action],
        _ctx: &mut AppContext,
    ) -> Vec<Self::Effect>;

    fn render(
        &mut self,
        state: &Self::State,
        view: &ViewTree<Self::Action>,
        renderer: &mut dyn Renderer2d,
    );

    fn handle_effects(&mut self, _effects: Vec<Self::Effect>, _ctx: &mut AppContext) {}

    fn on_run_mode(&mut self, _mode: RunMode, _state: &mut Self::State, _ctx: &mut AppContext) {}

    fn exit_requested(&self) -> bool {
        false
    }

    fn shutdown(&mut self) {}

    fn handle_event(
        &mut self,
        _event: &Event<()>,
        _state: &mut Self::State,
        _input: &mut InputFrame,
        _ctx: &mut AppContext,
        _control_flow: &mut ControlFlow,
    ) -> bool {
        // Lifecycle-only hook: use this for close/exit flow, redraw cadence, and loop teardown.
        // Gameplay/UI keyboard+mouse decisions should read from `InputFrame` in `update_state`.
        false
    }
}

pub trait AppHandler {
    fn init(&mut self, _ctx: &mut WindowContext) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn handle_event(
        &mut self,
        event: Event<()>,
        control_flow: &mut ControlFlow,
        ctx: &mut WindowContext,
    );
}

fn create_app_context(
    config: &AppConfig,
    event_loop: &EventLoop<()>,
) -> Result<WindowContext, Box<dyn Error>> {
    let monitor_size = if config.clamp_to_monitor {
        event_loop.primary_monitor().map(|m| m.size())
    } else {
        None
    };
    let initial_size = if let Some(monitor) = monitor_size {
        PhysicalSize::new(
            config.desired_size.width.min(monitor.width),
            config.desired_size.height.min(monitor.height),
        )
    } else {
        config.desired_size
    };
    let window = WindowBuilder::new()
        .with_title(config.title.clone())
        .with_inner_size(initial_size)
        .build(event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels =
        |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
            let surface_texture =
                SurfaceTexture::new(surface_size.width, surface_size.height, &window);
            let mut pixels_builder =
                PixelsBuilder::new(surface_size.width, surface_size.height, surface_texture);
            if let Some(vsync) = config.vsync {
                pixels_builder = pixels_builder.enable_vsync(vsync);
            }
            if let Some(mode) = present_mode {
                pixels_builder = pixels_builder.present_mode(mode);
            }
            pixels_builder.build()
        };

    let pixels = if let Some(mode) = config.present_mode {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_pixels(Some(mode)))) {
            Ok(res) => res?,
            Err(_) => {
                eprintln!(
                    "warning: requested present mode {:?} was not supported; falling back",
                    mode
                );
                build_pixels(None)?
            }
        }
    } else {
        build_pixels(None)?
    };

    let renderer = PixelsRenderer2d::new_auto(pixels, surface_size)?;
    Ok(WindowContext {
        window,
        renderer,
        app: AppContext { surface_size },
    })
}

pub fn run_app<H: AppHandler + 'static>(
    config: AppConfig,
    mut handler: H,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
    let mut ctx = create_app_context(&config, &event_loop)?;
    handler.init(&mut ctx)?;

    event_loop.run(move |event, _, control_flow| {
        handler.handle_event(event, control_flow, &mut ctx);
    });

    #[allow(unreachable_code)]
    Ok(())
}

/// Hooks for a game whose simulation can continue from a recorded snapshot.
pub trait TimelineGame: GameApp {
    type Snapshot: Clone + Send + Serialize + DeserializeOwned + 'static;
    const SCHEMA: StateSchema;

    fn snapshot(state: &Self::State) -> &Self::Snapshot;
    fn restore_state(&mut self, snapshot: Self::Snapshot) -> Self::State;

    fn timeline_command(&self, _input: &InputFrame) -> Option<TimelineCommand> {
        None
    }
}

pub fn run_game<G: GameApp + 'static>(config: AppConfig, game: G) -> Result<(), Box<dyn Error>> {
    run_live(config, game, NormalSession)
}

pub fn run_game_with_timeline<G>(
    config: AppConfig,
    game: G,
    options: TimelineConfig,
) -> Result<(), Box<dyn Error>>
where
    G: TimelineGame + 'static,
{
    let (mut timeline, mut snapshot) = Timeline::open(&options.directory, G::SCHEMA)?;
    if options.playback {
        snapshot = Some(timeline.pause_at_start()?);
    }
    println!("timeline: {}", options.directory.display());
    if let Some(cursor) = timeline.cursor() {
        println!("restored checkpoint {cursor}");
    }
    let editor = GameLink::connect()?;
    let session = TimelineSession {
        paused: timeline.is_paused(),
        cursor: timeline.position(),
        tip: timeline.tip_position()?,
        clock_position: timeline.position().map_or(0, |frame| frame.at_ms),
        clock_start: Instant::now(),
        playback: None,
        writer: TimelineWorker::start(timeline)?,
        snapshot,
        pending: None,
        interval: options.checkpoint_interval,
        last_checkpoint: Instant::now(),
        overseer: reload::listen_for_overseer(),
    };
    if let Some(editor) = editor {
        run_embedded(game, session, editor)
    } else {
        run_live(config, game, session)
    }
}

trait LiveSession<G: GameApp> {
    const MODE: RunMode;

    fn start(&mut self, game: &mut G, ctx: &mut AppContext) -> io::Result<G::State> {
        Ok(game.init_state(ctx))
    }

    fn should_update(
        &mut self,
        _game: &mut G,
        _state: &mut G::State,
        _input: &InputFrame,
        _command: Option<TimelineCommand>,
    ) -> bool {
        true
    }

    fn timeline_status(&self) -> Option<TimelineStatus> {
        None
    }

    fn after_update(&mut self, _state: &G::State, _now: Instant) {}

    fn process_command(&mut self) -> Option<ProcessCommand> {
        None
    }

    fn save(&mut self, _state: &G::State) -> io::Result<()> {
        Ok(())
    }
}

struct NormalSession;

impl<G: GameApp> LiveSession<G> for NormalSession {
    const MODE: RunMode = RunMode::Normal;
}

struct Playback {
    from_ms: u64,
    started: Instant,
    next_ms: u64,
}

enum TimelineAction {
    Control(TimelineCommand),
    PlayUntil(u64),
}

struct TimelineSession<S> {
    writer: TimelineWorker,
    pending: Option<Receiver<io::Result<Option<Frame>>>>,
    paused: bool,
    cursor: Option<Frame>,
    tip: Option<Frame>,
    clock_position: u64,
    clock_start: Instant,
    playback: Option<Playback>,
    snapshot: Option<S>,
    interval: Duration,
    last_checkpoint: Instant,
    overseer: Option<Receiver<ProcessCommand>>,
}

impl<S> TimelineSession<S> {
    fn live_time_ms(&self) -> u64 {
        self.clock_position
            .saturating_add(elapsed_ms(self.clock_start))
    }

    fn poll_capture(&mut self) {
        let Some(pending) = &self.pending else { return };
        match pending.try_recv() {
            Ok(Ok(frame)) if !self.paused => {
                self.cursor = frame;
                self.tip = frame;
            }
            Ok(Ok(_)) => {}
            Ok(Err(err)) => eprintln!("could not record timeline checkpoint: {err}"),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => eprintln!("timeline writer stopped"),
        }
        self.pending = None;
    }
}

fn update_timeline_title(window: &Window, title: &str, status: TimelineStatus) {
    if let Some(cursor) = status.cursor.filter(|_| status.paused) {
        let mode = if status.playing { "playing" } else { "paused" };
        window.set_title(&format!(
            "{title} — timeline {mode} at {:.1}s (checkpoint {})",
            cursor.at_ms as f64 / 1000.0,
            cursor.id,
        ));
    } else {
        window.set_title(title);
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl<G: TimelineGame> LiveSession<G> for TimelineSession<G::Snapshot> {
    const MODE: RunMode = RunMode::Timeline;

    fn start(&mut self, game: &mut G, ctx: &mut AppContext) -> io::Result<G::State> {
        let state = if let Some(snapshot) = self.snapshot.take() {
            game.restore_state(snapshot)
        } else {
            let state = game.init_state(ctx);
            let initial = G::snapshot(&state).clone();
            let position = self.writer.request(move |timeline| {
                timeline.checkpoint_at(&initial, 0)?;
                Ok(timeline.position())
            })?;
            self.cursor = position;
            self.tip = position;
            state
        };
        self.last_checkpoint = Instant::now();
        self.clock_start = self.last_checkpoint;
        Ok(state)
    }

    fn should_update(
        &mut self,
        game: &mut G,
        state: &mut G::State,
        input: &InputFrame,
        command: Option<TimelineCommand>,
    ) -> bool {
        self.poll_capture();
        let action = if let Some(command) = command.or_else(|| game.timeline_command(input)) {
            let was_playing = self.playback.take().is_some();
            if command == TimelineCommand::TogglePlayback && was_playing {
                return false;
            }
            TimelineAction::Control(command)
        } else if let Some(playback) = &self.playback {
            let target = playback
                .from_ms
                .saturating_add(elapsed_ms(playback.started));
            if target < playback.next_ms {
                return false;
            }
            TimelineAction::PlayUntil(target)
        } else {
            return !self.paused;
        };

        let was_paused = self.paused;
        let live_state = (!was_paused).then(|| G::snapshot(state).clone());
        let at_ms = self.live_time_ms();
        let result = self.writer.request(move |timeline| {
            let changed = match action {
                TimelineAction::Control(command) => {
                    let prepared = if command == TimelineCommand::ResumeLive {
                        timeline.resume()
                    } else if let Some(snapshot) = live_state {
                        timeline.pause_at(&snapshot, at_ms)
                    } else if command == TimelineCommand::TogglePause {
                        timeline.resume()
                    } else {
                        Ok(())
                    };
                    prepared.and_then(|()| match command {
                        TimelineCommand::Rewind => timeline.rewind(),
                        TimelineCommand::Forward => timeline.forward(),
                        TimelineCommand::Seek(time) => timeline.seek(time),
                        _ => Ok(None),
                    })
                }
                TimelineAction::PlayUntil(target) => timeline.play_until(target),
            };
            let next = if matches!(
                action,
                TimelineAction::PlayUntil(_)
                    | TimelineAction::Control(TimelineCommand::TogglePlayback)
            ) && changed.is_ok()
            {
                timeline.next_frame()
            } else {
                Ok(None)
            };
            Ok((
                timeline.is_paused(),
                timeline.position(),
                timeline.tip_position(),
                changed,
                next,
            ))
        });
        match result {
            Ok((paused, cursor, tip, changed, next)) => {
                self.pending = None;
                self.paused = paused;
                self.cursor = cursor;
                match tip {
                    Ok(tip) => self.tip = tip,
                    Err(err) => eprintln!("timeline tip unavailable: {err}"),
                }
                if was_paused && !paused {
                    self.clock_position = cursor.map_or(0, |frame| frame.at_ms);
                    self.clock_start = Instant::now();
                }
                match changed {
                    Ok(Some(snapshot)) => *state = game.restore_state(snapshot),
                    Ok(None) => {}
                    Err(err) => eprintln!("timeline control failed: {err}"),
                }
                match next {
                    Ok(Some(next)) => match &mut self.playback {
                        Some(playback) => playback.next_ms = next.at_ms,
                        None => {
                            let from_ms = cursor.expect("playing requires a checkpoint").at_ms;
                            self.playback = Some(Playback {
                                from_ms,
                                started: Instant::now(),
                                next_ms: next.at_ms,
                            });
                        }
                    },
                    Ok(None) => self.playback = None,
                    Err(err) => {
                        self.playback = None;
                        eprintln!("timeline playback failed: {err}");
                    }
                }
            }
            Err(err) => {
                self.playback = None;
                eprintln!("timeline control failed: {err}");
            }
        }
        self.last_checkpoint = Instant::now();
        false
    }

    fn timeline_status(&self) -> Option<TimelineStatus> {
        Some(TimelineStatus {
            cursor: self.cursor,
            tip: self.tip,
            paused: self.paused,
            playing: self.playback.is_some(),
        })
    }

    fn after_update(&mut self, state: &G::State, now: Instant) {
        if self.pending.is_some() {
            return;
        }
        if now.saturating_duration_since(self.last_checkpoint) >= self.interval {
            self.last_checkpoint = now;
            let snapshot = G::snapshot(state).clone();
            let at_ms = self.live_time_ms();
            match self.writer.submit(move |timeline| {
                timeline.capture_at(&snapshot, at_ms)?;
                Ok(timeline.position())
            }) {
                Ok(pending) => self.pending = Some(pending),
                Err(err) => eprintln!("could not queue timeline checkpoint: {err}"),
            }
        }
    }

    fn process_command(&mut self) -> Option<ProcessCommand> {
        self.overseer.as_ref()?.try_recv().ok()
    }

    fn save(&mut self, state: &G::State) -> io::Result<()> {
        let snapshot = G::snapshot(state).clone();
        let at_ms = self.live_time_ms();
        self.writer
            .request(move |timeline| timeline.checkpoint_at(&snapshot, at_ms))
    }
}

fn update_game<G: GameApp>(
    game: &mut G,
    state: &mut G::State,
    input: &InputFrame,
    dt: Duration,
    ctx: &mut AppContext,
) -> Vec<G::Effect> {
    let view = game.build_view(state, ctx);
    let actions = hit_test_actions(
        &view,
        UiInput {
            mouse_pos: input.mouse_pos,
            mouse_down: input.mouse_down,
            mouse_up: input.mouse_up,
        },
    );
    game.update_state(state, input.clone(), dt, &actions, ctx)
}

fn exit_for_command<G: GameApp>(
    session: &mut impl LiveSession<G>,
    state: &G::State,
    command: ProcessCommand,
) -> Option<i32> {
    match (command, session.save(state)) {
        (ProcessCommand::Reload, Ok(())) => Some(RELOAD_EXIT_CODE),
        (ProcessCommand::Reload, Err(err)) => {
            reload::reject_reload(&err);
            None
        }
        (ProcessCommand::Stop, result) => {
            if let Err(err) = result {
                eprintln!("could not save final state: {err}");
            }
            Some(0)
        }
    }
}

fn run_embedded<G: TimelineGame>(
    mut game: G,
    mut session: TimelineSession<G::Snapshot>,
    editor: GameLink,
) -> Result<(), Box<dyn Error>> {
    let size = SurfaceSize::new(PREVIEW_WIDTH, PREVIEW_HEIGHT);
    let mut ctx = AppContext { surface_size: size };
    let mut state = session.start(&mut game, &mut ctx)?;
    game.on_run_mode(RunMode::Timeline, &mut state, &mut ctx);
    let mut input = InputFrame::default();
    let mut pixels = vec![0; PREVIEW_BYTES];
    let mut last_frame = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / 60.0);
    let exit_code = 'game: loop {
        if let Some(command) =
            <TimelineSession<G::Snapshot> as LiveSession<G>>::process_command(&mut session)
        {
            if let Some(status) = exit_for_command::<G>(&mut session, &state, command) {
                break status;
            }
        }
        if game.exit_requested() {
            break exit_for_command::<G>(&mut session, &state, ProcessCommand::Stop).unwrap_or(0);
        }

        let now = Instant::now();
        let dt = now.saturating_duration_since(last_frame);
        last_frame = now;
        let mut commands = Vec::new();
        loop {
            let event = match editor.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    break 'game exit_for_command::<G>(&mut session, &state, ProcessCommand::Stop)
                        .unwrap_or(0);
                }
            };
            if let EditorEvent::Timeline(command) = event {
                if matches!(command, TimelineCommand::Seek(_))
                    && matches!(commands.last(), Some(TimelineCommand::Seek(_)))
                {
                    commands.pop();
                }
                commands.push(command);
            } else {
                input.apply_editor_event(event);
            }
        }
        let effects = if commands.is_empty() {
            if session.should_update(&mut game, &mut state, &input, None) {
                let effects = update_game(&mut game, &mut state, &input, dt, &mut ctx);
                LiveSession::<G>::after_update(&mut session, &state, now);
                Some(effects)
            } else {
                last_frame = Instant::now();
                None
            }
        } else {
            for command in commands {
                session.should_update(&mut game, &mut state, &input, Some(command));
            }
            last_frame = Instant::now();
            None
        };
        pixels.fill(0);
        let view = game.build_view(&state, &ctx);
        game.render(&state, &view, &mut CpuRenderer::new(&mut pixels, size));
        let status = LiveSession::<G>::timeline_status(&session).unwrap_or_default();
        editor.publish(status, &mut pixels);
        if let Some(effects) = effects {
            game.handle_effects(effects, &mut ctx);
        }
        input.clear_frame_transients();
        std::thread::sleep(interval.saturating_sub(now.elapsed()));
    };
    game.shutdown();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn run_live<G, S>(config: AppConfig, mut game: G, mut session: S) -> Result<(), Box<dyn Error>>
where
    G: GameApp + 'static,
    S: LiveSession<G> + 'static,
{
    let event_loop = EventLoop::new();
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = session.start(&mut game, &mut ctx.app)?;
    game.on_run_mode(S::MODE, &mut state, &mut ctx.app);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut exiting = false;
    let mut previous_status = None;

    event_loop.run(move |event, _, control_flow| {
        if exiting {
            if matches!(event, Event::LoopDestroyed) {
                game.handle_event(&event, &mut state, &mut input, &mut ctx.app, control_flow);
            }
            return;
        }
        *control_flow = ControlFlow::Poll;
        if let Event::WindowEvent { event, .. } = &event {
            apply_window_event_to_input(&mut input, event);
        }

        if let Some(command) = session.process_command() {
            if let Some(code) = exit_for_command::<G>(&mut session, &state, command) {
                *control_flow = ControlFlow::ExitWithCode(code);
                exiting = true;
            }
            return;
        }

        let handled = game.handle_event(&event, &mut state, &mut input, &mut ctx.app, control_flow);
        if matches!(control_flow, ControlFlow::ExitWithCode(_))
            || matches!(event, Event::LoopDestroyed)
            || (!handled
                && matches!(
                    event,
                    Event::WindowEvent {
                        event: WindowEvent::CloseRequested,
                        ..
                    }
                ))
        {
            if let Err(err) = session.save(&state) {
                eprintln!("could not save final state: {err}");
            }
            exiting = true;
            if !matches!(control_flow, ControlFlow::ExitWithCode(_)) {
                *control_flow = ControlFlow::Exit;
            }
            return;
        }
        if handled {
            return;
        }

        match &event {
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                ctx.resize(*size);
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;
                let effects = if session.should_update(&mut game, &mut state, &input, None) {
                    let effects = update_game(&mut game, &mut state, &input, dt, &mut ctx.app);
                    session.after_update(&state, now);
                    Some(effects)
                } else {
                    last_frame = Instant::now();
                    None
                };
                let current_status = session.timeline_status();
                if current_status != previous_status
                    && let Some(status) = current_status
                {
                    if status.paused || previous_status.is_none_or(|previous| previous.paused) {
                        update_timeline_title(&ctx.window, &config.title, status);
                    }
                    previous_status = current_status;
                }
                draw_game(&mut game, &state, &mut ctx);
                if let Some(effects) = effects {
                    game.handle_effects(effects, &mut ctx.app);
                }
                input.clear_frame_transients();
            }
            Event::MainEventsCleared => ctx.window.request_redraw(),
            _ => {}
        }
    });
}

fn draw_game<G: GameApp>(game: &mut G, state: &G::State, ctx: &mut WindowContext) {
    let view = game.build_view(state, &ctx.app);
    if let Err(err) = ctx
        .renderer
        .draw_frame(|gfx| game.render(state, &view, gfx))
    {
        eprintln!("draw failed: {err}");
    }
    if let Err(err) = ctx.renderer.present() {
        eprintln!("present failed: {err}");
    }
}

pub fn run_game_with_profile<G>(
    config: AppConfig,
    mut game: G,
    profile: ProfileConfig,
) -> Result<(), Box<dyn Error>>
where
    G: GameApp + 'static,
{
    let event_loop = EventLoop::new();
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = game.init_state(&mut ctx.app);
    game.on_run_mode(RunMode::Profile, &mut state, &mut ctx.app);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut trace = TraceCapture::new(profile.target_frames);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Event::WindowEvent { event, .. } = &event {
            apply_window_event_to_input(&mut input, event);
        }

        if game.handle_event(&event, &mut state, &mut input, &mut ctx.app, control_flow) {
            return;
        }

        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::Resized(size) => {
                    ctx.resize(*size);
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let frame_start = Instant::now();
                let update_start = Instant::now();
                let frame_input = input.clone();
                let view_for_input = game.build_view(&state, &ctx.app);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: frame_input.mouse_pos,
                        mouse_down: frame_input.mouse_down,
                        mouse_up: frame_input.mouse_up,
                    },
                );
                let effects =
                    game.update_state(&mut state, frame_input, dt, &actions, &mut ctx.app);
                let update_dt = update_start.elapsed();
                trace.record("engine.update", update_start, update_dt);

                let draw_start = Instant::now();
                let view_for_render = game.build_view(&state, &ctx.app);
                let draw_res = ctx.renderer.draw_frame(|gfx| {
                    game.render(&state, &view_for_render, gfx);
                });
                let draw_dt = draw_start.elapsed();
                trace.record("render.draw", draw_start, draw_dt);
                if let Err(err) = draw_res {
                    eprintln!("draw failed: {err}");
                }

                let present_start = Instant::now();
                if let Err(err) = ctx.renderer.present() {
                    eprintln!("present failed: {err}");
                }
                let present_dt = present_start.elapsed();
                trace.record("render.present", present_start, present_dt);

                let frame_total_dt = frame_start.elapsed();
                trace.record("frame.total", frame_start, frame_total_dt);
                trace.record_frame_samples(update_dt, draw_dt, present_dt, frame_total_dt);

                game.handle_effects(effects, &mut ctx.app);
                input.clear_frame_transients();

                if trace.captured_frames >= trace.target_frames {
                    let size = ctx.renderer.size();
                    match trace.write(size) {
                        Ok(path) => {
                            println!("trace written: {}", path.display());
                            trace.print_summary();
                        }
                        Err(err) => eprintln!("failed writing trace: {err}"),
                    }
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::MainEventsCleared => {
                ctx.window.request_redraw();
            }
            _ => {}
        }
    });

    #[allow(unreachable_code)]
    Ok(())
}

#[derive(Debug, Default, Clone)]
struct DurationAgg {
    n: usize,
    sum: Duration,
    max: Duration,
}

impl DurationAgg {
    fn push(&mut self, d: Duration) {
        self.n = self.n.saturating_add(1);
        self.sum = self.sum.saturating_add(d);
        if d > self.max {
            self.max = d;
        }
    }

    fn avg_ms(&self) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        (self.sum.as_secs_f64() * 1000.0) / (self.n as f64)
    }

    fn max_ms(&self) -> f64 {
        self.max.as_secs_f64() * 1000.0
    }
}

#[derive(Debug, Clone, Copy)]
struct TraceEvent {
    name: &'static str,
    ts_us: u64,
    dur_us: u64,
}

#[derive(Debug)]
struct TraceCapture {
    target_frames: usize,
    captured_frames: usize,
    start: Instant,
    events: Vec<TraceEvent>,

    update: DurationAgg,
    draw: DurationAgg,
    present: DurationAgg,
    frame_total: DurationAgg,
}

impl TraceCapture {
    fn new(target_frames: usize) -> Self {
        let target_frames = target_frames.max(1);
        Self {
            target_frames,
            captured_frames: 0,
            start: Instant::now(),
            events: Vec::with_capacity(target_frames.saturating_mul(6)),
            update: DurationAgg::default(),
            draw: DurationAgg::default(),
            present: DurationAgg::default(),
            frame_total: DurationAgg::default(),
        }
    }

    fn ts_us(&self, t: Instant) -> u64 {
        t.duration_since(self.start).as_micros() as u64
    }

    fn record(&mut self, name: &'static str, start: Instant, dur: Duration) {
        self.events.push(TraceEvent {
            name,
            ts_us: self.ts_us(start),
            dur_us: dur.as_micros() as u64,
        });
    }

    fn record_frame_samples(
        &mut self,
        update: Duration,
        draw: Duration,
        present: Duration,
        frame: Duration,
    ) {
        self.update.push(update);
        self.draw.push(draw);
        self.present.push(present);
        self.frame_total.push(frame);
        self.captured_frames = self.captured_frames.saturating_add(1);
    }

    fn default_trace_dir() -> PathBuf {
        // `CARGO_MANIFEST_DIR` is `.../rollout_engine/engine`; the workspace `target/` lives at `..`.
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("perf_traces")
    }

    fn write(&self, size: SurfaceSize) -> io::Result<PathBuf> {
        let dir = Self::default_trace_dir();
        fs::create_dir_all(&dir)?;

        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let file_name = format!(
            "perf_trace_{epoch_ms}_{}x{}_{}f.json",
            size.width, size.height, self.captured_frames
        );
        let path = dir.join(file_name);

        let mut f = fs::File::create(&path)?;
        writeln!(f, "{{\"traceEvents\":[")?;
        for (i, e) in self.events.iter().enumerate() {
            if i > 0 {
                writeln!(f, ",")?;
            }
            write!(
                f,
                "  {{\"name\":\"{}\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":1}}",
                e.name, e.ts_us, e.dur_us
            )?;
        }
        writeln!(f, "\n]}}")?;
        Ok(path)
    }

    fn print_summary(&self) {
        println!("profile summary (ms; lower is better)");
        println!(
            "update(avg/max)  {:>7.3} / {:>7.3}",
            self.update.avg_ms(),
            self.update.max_ms()
        );
        println!(
            "draw  (avg/max)  {:>7.3} / {:>7.3}",
            self.draw.avg_ms(),
            self.draw.max_ms()
        );
        println!(
            "present(avg/max) {:>7.3} / {:>7.3}",
            self.present.avg_ms(),
            self.present.max_ms()
        );
        println!(
            "frame (avg/max)  {:>7.3} / {:>7.3}",
            self.frame_total.avg_ms(),
            self.frame_total.max_ms()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_transitions_are_frame_based() {
        let mut input = InputFrame::default();

        input.apply_key_state(VirtualKeyCode::A, ElementState::Pressed);
        assert!(input.keys_down.contains(&VirtualKeyCode::A));
        assert!(input.keys_pressed.contains(&VirtualKeyCode::A));
        assert!(!input.keys_released.contains(&VirtualKeyCode::A));

        input.clear_frame_transients();
        assert!(input.keys_down.contains(&VirtualKeyCode::A));
        assert!(!input.keys_pressed.contains(&VirtualKeyCode::A));

        input.apply_key_state(VirtualKeyCode::A, ElementState::Released);
        assert!(!input.keys_down.contains(&VirtualKeyCode::A));
        assert!(input.keys_released.contains(&VirtualKeyCode::A));
    }

    #[test]
    fn mouse_left_transitions_set_legacy_click_flags() {
        let mut input = InputFrame::default();

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Pressed);
        assert!(input.mouse_down);
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_pressed.contains(&MouseButton::Left));

        input.clear_frame_transients();
        assert!(!input.mouse_down);
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Released);
        assert!(input.mouse_up);
        assert!(!input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_released.contains(&MouseButton::Left));
    }

    #[test]
    fn scroll_accumulates_then_resets_between_frames() {
        let mut input = InputFrame::default();

        input.apply_scroll_delta(&MouseScrollDelta::LineDelta(1.0, -2.0));
        input.apply_scroll_delta(&MouseScrollDelta::PixelDelta(
            winit::dpi::PhysicalPosition::new(240.0, -120.0),
        ));
        assert!((input.scroll_x - 3.0).abs() < 0.0001);
        assert!((input.scroll_y - -3.0).abs() < 0.0001);

        input.clear_frame_transients();
        assert!((input.scroll_x - 0.0).abs() < 0.0001);
        assert!((input.scroll_y - 0.0).abs() < 0.0001);
    }

    #[test]
    fn focus_loss_clears_held_inputs() {
        let mut input = InputFrame::default();

        input.apply_key_state(VirtualKeyCode::Space, ElementState::Pressed);
        input.apply_mouse_button_state(MouseButton::Left, ElementState::Pressed);
        input.set_window_focus(false);

        assert!(!input.window_focused);
        assert!(input.keys_down.is_empty());
        assert!(input.mouse_buttons_down.is_empty());
        assert!(!input.mouse_down);
        assert!(!input.mouse_up);
    }
}
