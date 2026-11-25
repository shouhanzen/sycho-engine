use crate::profiling::{
    FrameOutput, FrameTimings, PreviewProfile, ProfileConfig, Span, TraceCapture, micros,
};
use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::hash::Hash;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::TryRecvError;
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
    ADDRESS_ENV, DEFAULT_PREVIEW_SIZE, EditorEvent, GPU_PREVIEW_EXIT_CODE, GameLink, MasterVolume,
    PreviewMetadata, TimelineStatus, valid_preview_size,
};
use crate::graphics::Renderer2d;
use crate::offscreen::{GpuPreview, PreviewWorker};
use crate::pixels_renderer::PixelsRenderer2d;
use crate::reload::{self, ProcessCommand, RELOAD_EXIT_CODE};
use crate::surface::SurfaceSize;
use crate::timeline::{
    Frame, StateSchema, Timeline, TimelineCommand, TimelineWorker, session_directory,
};

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
    pub playback: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TimelineCli {
    pub help: bool,
    pub profile: bool,
    pub no_recording: bool,
    pub timeline_path: Option<PathBuf>,
    pub fresh: bool,
    pub playback: bool,
    pub export_video: Option<PathBuf>,
}

/// Options supplied by the host when launching a game.
pub enum RunConfig {
    Live { timeline: Option<TimelineConfig> },
    Profile(ProfileConfig),
}

impl TimelineCli {
    pub fn recording(&self, default_directory: PathBuf) -> io::Result<Option<TimelineConfig>> {
        if self.no_recording {
            return Ok(None);
        }
        Ok(Some(TimelineConfig {
            directory: session_directory(
                default_directory,
                self.timeline_path.clone(),
                self.fresh,
            )?,
            playback: self.playback,
        }))
    }
}

pub fn parse_timeline_cli() -> io::Result<TimelineCli> {
    parse_timeline_args(env::args_os().skip(1))
}

fn parse_timeline_args(mut args: impl Iterator<Item = OsString>) -> io::Result<TimelineCli> {
    let mut cli = TimelineCli::default();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        match arg.as_ref() {
            "-h" | "--help" => cli.help = true,
            "--profile" => cli.profile = true,
            "--fresh" => cli.fresh = true,
            "--no-recording" => cli.no_recording = true,
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
    let timeline_options =
        cli.fresh || cli.playback || cli.timeline_path.is_some() || cli.export_video.is_some();
    if cli.profile && timeline_options {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--profile cannot be combined with timeline options",
        ));
    }
    if cli.no_recording && (timeline_options || cli.profile) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--no-recording cannot be combined with timeline options or --profile",
        ));
    }
    Ok(cli)
}

/// The game-facing context shared by windowed and embedded rendering.
pub struct AppContext {
    pub surface_size: SurfaceSize,
}

/// Native window and presentation state for host-level applications.
struct WindowContext {
    window: Window,
    renderer: PixelsRenderer2d,
    app: AppContext,
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
        apply_button_transition(
            &mut self.mouse_buttons_down,
            &mut self.mouse_buttons_pressed,
            &mut self.mouse_buttons_released,
            button,
            state,
        );
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
            EditorEvent::Resize(_)
            | EditorEvent::Profile(_)
            | EditorEvent::MasterVolume { .. }
            | EditorEvent::Timeline(_)
            | EditorEvent::ReleaseSurface(_)
            | EditorEvent::UseRgba => {}
        }
    }

    fn clear_frame_transients(&mut self) {
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
        WindowEvent::CursorLeft { .. } => input.mouse_pos = None,
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

    fn init_state(&mut self, _ctx: &mut AppContext) -> Self::State;

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: &InputFrame,
        dt: Duration,
        _ctx: &mut AppContext,
    );

    fn render(&mut self, state: &Self::State, renderer: &mut dyn Renderer2d);

    /// Receives the engine's timings once the frame's CPU render work has completed.
    fn on_frame_timings(&mut self, _timings: FrameTimings) {}

    /// Live master volume advertised to the editor in percent.
    fn master_volume(&self) -> Option<u8> {
        None
    }

    /// Apply a master volume adjustment, saving it when the user releases the slider.
    fn set_master_volume(&mut self, _percent: u8, _commit: bool) {}

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

/// Hooks for a game whose simulation can continue from a recorded snapshot.
pub trait TimelineGame: GameApp {
    type Snapshot: Clone + Send + Serialize + DeserializeOwned + 'static;
    type PreviewSnapshot: Send + 'static;
    const SCHEMA: StateSchema;

    fn snapshot(state: &Self::State) -> &Self::Snapshot;
    fn restore_state(&mut self, snapshot: Self::Snapshot) -> Self::State;

    /// Capture the data that the editor preview can draw independently of gameplay.
    fn preview_snapshot(&self, state: &Self::State) -> Self::PreviewSnapshot;

    fn draw_preview(snapshot: Self::PreviewSnapshot, renderer: &mut dyn Renderer2d);

    fn set_timeline_paused(&mut self, _paused: bool) {}

    fn timeline_command(&self, _input: &InputFrame) -> Option<TimelineCommand> {
        None
    }
}

pub fn run_game<G>(config: AppConfig, game: G, run: RunConfig) -> Result<(), Box<dyn Error>>
where
    G: TimelineGame + 'static,
{
    let (timeline, trace) = match run {
        RunConfig::Live { timeline } => (timeline, None),
        RunConfig::Profile(profile) => (None, Some(TraceCapture::new(profile))),
    };
    let Some(options) = timeline else {
        if env::var_os(ADDRESS_ENV).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the editor requires timeline recording",
            )
            .into());
        }
        return run_windowed(config, game, None, trace);
    };
    let (mut timeline, mut snapshot) = Timeline::open(&options.directory, G::SCHEMA)?;
    if options.playback {
        snapshot = Some(timeline.pause_at_start()?);
    }
    println!("timeline: {}", options.directory.display());
    if let Some(cursor) = timeline.cursor() {
        println!("restored checkpoint {cursor}");
    }
    let editor = match GameLink::connect()? {
        Some((link, publisher)) => {
            match GpuPreview::new(publisher).and_then(|gpu| Ok(gpu.spawn(G::draw_preview)?)) {
                Ok(preview) => Some((link, preview)),
                Err(err) => {
                    eprintln!("could not initialize the editor GPU preview: {err}");
                    drop(link);
                    drop(timeline);
                    std::process::exit(GPU_PREVIEW_EXIT_CODE);
                }
            }
        }
        None => None,
    };
    let session = TimelineSession {
        paused: timeline.is_paused(),
        cursor: timeline.position(),
        tip: timeline.tip_position()?,
        clock_position: timeline.position().map_or(0, |frame| frame.at_ms),
        clock_start: Instant::now(),
        playback: None,
        writer: TimelineWorker::start(timeline)?,
        snapshot,
        capture_failed: false,
    };
    if let Some((editor, preview)) = editor {
        run_embedded(game, session, editor, preview)
    } else {
        run_windowed(config, game, Some(session), None)
    }
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

struct TimelineSession<G: TimelineGame> {
    writer: TimelineWorker<G::Snapshot>,
    capture_failed: bool,
    paused: bool,
    cursor: Option<Frame>,
    tip: Option<Frame>,
    clock_position: u64,
    clock_start: Instant,
    playback: Option<Playback>,
    snapshot: Option<G::Snapshot>,
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

impl<G: TimelineGame> TimelineSession<G> {
    fn live_time_ms(&self) -> u64 {
        self.clock_position
            .saturating_add(elapsed_ms(self.clock_start))
    }

    fn poll_capture(&mut self) {
        for result in self.writer.commits() {
            match result {
                Ok(frame) if !self.paused => {
                    self.cursor = Some(frame);
                    self.tip = Some(frame);
                }
                Ok(_) => {}
                Err(err) => {
                    eprintln!("could not record timeline checkpoint: {err}");
                    self.capture_failed = true;
                }
            }
        }
    }

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
        game.set_timeline_paused(self.paused);
        self.clock_start = Instant::now();
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
            let command = match command {
                TimelineCommand::TogglePlayPause
                    if self.paused
                        && self.playback.is_none()
                        && self.tip.is_some()
                        && self.cursor == self.tip =>
                {
                    TimelineCommand::ResumeLive
                }
                TimelineCommand::TogglePlayPause => TimelineCommand::TogglePlayback,
                command => command,
            };
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
                self.poll_capture();
                self.paused = paused;
                game.set_timeline_paused(paused);
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
        false
    }

    fn status(&self) -> TimelineStatus {
        TimelineStatus {
            cursor: self.cursor,
            tip: self.tip,
            paused: self.paused,
            playing: self.playback.is_some(),
        }
    }

    fn after_update(&mut self, state: &G::State) {
        if self.capture_failed {
            return;
        }
        let at_ms = self.live_time_ms();
        if let Err(err) = self.writer.capture(G::snapshot(state).clone(), at_ms) {
            self.poll_capture();
            if !self.capture_failed {
                eprintln!("could not queue timeline checkpoint: {err}");
                self.capture_failed = true;
            }
        }
    }

    fn save(&mut self, state: &G::State) -> io::Result<()> {
        let snapshot = G::snapshot(state).clone();
        let at_ms = self.live_time_ms();
        self.writer
            .request(move |timeline| timeline.checkpoint_at(&snapshot, at_ms))
    }
}

fn exit_for_command<G: TimelineGame>(
    session: Option<&mut TimelineSession<G>>,
    state: &G::State,
    command: ProcessCommand,
) -> Option<i32> {
    match (
        command,
        session.map_or(Ok(()), |session| session.save(state)),
    ) {
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
    mut session: TimelineSession<G>,
    editor: GameLink,
    preview: PreviewWorker<G::PreviewSnapshot>,
) -> Result<(), Box<dyn Error>> {
    let mut ctx = AppContext {
        surface_size: DEFAULT_PREVIEW_SIZE,
    };
    let mut state = session.start(&mut game, &mut ctx)?;
    let overseer = reload::listen_for_overseer();
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut deadline = last_frame;
    let interval = Duration::from_secs_f64(1.0 / 60.0);
    let mut profile_enabled = false;
    let mut last_volume_request = None;
    let mut frame = 0;
    let mut tick = 0;
    let exit_code = 'game: loop {
        if let Some(command) = overseer
            .as_ref()
            .and_then(|commands| commands.try_recv().ok())
        {
            if let Some(status) = exit_for_command::<G>(Some(&mut session), &state, command) {
                break status;
            }
        }
        if game.exit_requested() {
            break exit_for_command::<G>(Some(&mut session), &state, ProcessCommand::Stop)
                .unwrap_or(0);
        }

        let now = Instant::now();
        let dt = now.saturating_duration_since(last_frame);
        last_frame = now;
        let mut commands = Vec::new();
        let mut volume_change = None;
        let mut commit_volume = false;
        loop {
            let event = match editor.events.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    break 'game exit_for_command::<G>(
                        Some(&mut session),
                        &state,
                        ProcessCommand::Stop,
                    )
                    .unwrap_or(0);
                }
            };
            match event {
                EditorEvent::Timeline(command) => {
                    if matches!(command, TimelineCommand::Seek(_))
                        && matches!(commands.last(), Some(TimelineCommand::Seek(_)))
                    {
                        commands.pop();
                    }
                    commands.push(command);
                }
                EditorEvent::Resize(size) if valid_preview_size(size) => ctx.surface_size = size,
                EditorEvent::Profile(enabled) => profile_enabled = enabled,
                EditorEvent::MasterVolume {
                    percent,
                    request,
                    commit,
                } => {
                    volume_change = Some((percent.min(100), request));
                    commit_volume |= commit;
                }
                _ => input.apply_editor_event(event),
            }
        }
        if let Some((percent, request)) = volume_change {
            game.set_master_volume(percent, commit_volume);
            last_volume_request = Some(request);
        }
        match preview.take_feedback() {
            Ok(Some((size, timings))) if size == ctx.surface_size => {
                game.on_frame_timings(timings);
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("could not draw the editor GPU preview: {err}");
                exit_for_command::<G>(Some(&mut session), &state, ProcessCommand::Stop);
                break GPU_PREVIEW_EXIT_CODE;
            }
        }
        let mut profile = profile_enabled.then(PreviewProfile::default);
        let mut update = Duration::ZERO;
        if commands.is_empty() {
            if session.should_update(&mut game, &mut state, &input, None) {
                let (_, span) =
                    Span::measure(|| game.update_state(&mut state, &input, dt, &mut ctx));
                update = span.elapsed;
                tick += 1;
                let start = profile_enabled.then(Instant::now);
                session.after_update(&state);
                if let (Some(profile), Some(start)) = (&mut profile, start) {
                    profile.timeline_us = micros(start.elapsed());
                }
            } else {
                last_frame = Instant::now();
            }
        } else {
            for command in commands {
                session.should_update(&mut game, &mut state, &input, Some(command));
            }
            last_frame = Instant::now();
        }
        let metadata = PreviewMetadata {
            status: session.status(),
            master_volume: game.master_volume().map(|percent| MasterVolume {
                percent: percent.min(100),
                request: last_volume_request,
            }),
        };
        frame += 1;
        let start = profile_enabled.then(Instant::now);
        let snapshot = game.preview_snapshot(&state);
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            profile.frame = frame;
            profile.tick = tick;
            profile.snapshot_us = micros(start.elapsed());
        }
        preview.publish(ctx.surface_size, metadata, snapshot, update, profile);
        input.clear_frame_transients();
        // Keep oversleep from accumulating across frames, and skip missed deadlines.
        let finished = Instant::now();
        deadline = (deadline + interval).max(finished);
        std::thread::sleep(deadline.saturating_duration_since(finished));
    };
    game.shutdown();
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn run_windowed<G: TimelineGame + 'static>(
    config: AppConfig,
    mut game: G,
    mut session: Option<TimelineSession<G>>,
    mut trace: Option<TraceCapture>,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = match &mut session {
        Some(session) => session.start(&mut game, &mut ctx.app)?,
        None => game.init_state(&mut ctx.app),
    };
    let overseer = reload::listen_for_overseer();
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

        if let Some(command) = overseer
            .as_ref()
            .and_then(|commands| commands.try_recv().ok())
        {
            if let Some(code) = exit_for_command(session.as_mut(), &state, command) {
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
            if let Some(session) = &mut session
                && let Err(err) = session.save(&state)
            {
                eprintln!("could not save final state: {err}");
            }
            if let Some(trace) = trace.take() {
                trace.finish(ctx.renderer.size());
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
                let update = if session.as_mut().is_none_or(|session| {
                    session.should_update(&mut game, &mut state, &input, None)
                }) {
                    let (_, update) =
                        Span::measure(|| game.update_state(&mut state, &input, dt, &mut ctx.app));
                    if let Some(session) = &mut session {
                        session.after_update(&state);
                    }
                    Some(update)
                } else {
                    last_frame = Instant::now();
                    None
                };
                if let Some(session) = &session {
                    let status = session.status();
                    if previous_status != Some(status) {
                        if status.paused || previous_status.is_none_or(|previous| previous.paused) {
                            update_timeline_title(&ctx.window, &config.title, status);
                        }
                        previous_status = Some(status);
                    }
                }
                let draw = match ctx
                    .renderer
                    .draw_frame(|gfx| Span::measure(|| game.render(&state, gfx)).1)
                {
                    Ok(draw) => Some(draw),
                    Err(err) => {
                        eprintln!("draw failed: {err}");
                        None
                    }
                };
                let (result, present) = Span::measure(|| ctx.renderer.present());
                if let Err(err) = result {
                    eprintln!("present failed: {err}");
                }
                let timings = FrameTimings {
                    interval: dt,
                    update: update.map_or(Duration::ZERO, |span| span.elapsed),
                    draw: draw.map_or(Duration::ZERO, |span| span.elapsed),
                    output: FrameOutput::Present(present.elapsed),
                };
                if let Some(capture) = &mut trace
                    && capture.record_frame(now, update, draw, present, timings)
                {
                    let completed = trace
                        .take()
                        .expect("completed trace")
                        .finish(ctx.renderer.size());
                    *control_flow = ControlFlow::ExitWithCode(if completed { 0 } else { 1 });
                    exiting = true;
                }
                game.on_frame_timings(timings);
                input.clear_frame_transients();
            }
            Event::MainEventsCleared => ctx.window.request_redraw(),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_defaults_on_and_can_be_disabled_for_standalone_play() {
        let default = PathBuf::from("target/timelines/example");
        let cli = parse_timeline_args(std::iter::empty()).unwrap();
        assert_eq!(
            cli.recording(default.clone()).unwrap().unwrap().directory,
            default
        );

        let cli = parse_timeline_args(["--no-recording"].into_iter().map(OsString::from)).unwrap();
        assert!(cli.recording(default).unwrap().is_none());

        for conflict in [
            "--fresh",
            "--playback",
            "--timeline",
            "--export-video",
            "--profile",
        ] {
            let args = ["--no-recording", conflict]
                .into_iter()
                .chain(matches!(conflict, "--timeline" | "--export-video").then_some("example"))
                .map(OsString::from);
            assert_eq!(
                parse_timeline_args(args).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

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
    fn mouse_button_edges_reset_between_ticks() {
        let mut input = InputFrame::default();

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Pressed);
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_pressed.contains(&MouseButton::Left));

        input.clear_frame_transients();
        assert!(input.mouse_buttons_down.contains(&MouseButton::Left));
        assert!(input.mouse_buttons_pressed.is_empty());

        input.apply_mouse_button_state(MouseButton::Left, ElementState::Released);
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
    }
}
