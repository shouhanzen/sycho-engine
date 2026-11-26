use crate::profiling::{
    FrameOutput, FrameTimings, PreviewProfile, ProfileConfig, Span, TraceCapture, micros,
};
use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Serialize, de::DeserializeOwned};

use pixels::{Pixels, PixelsBuilder, SurfaceTexture, wgpu::PresentMode};
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use crate::embedded::{
    ADDRESS_ENV, DEFAULT_PREVIEW_SIZE, DISPLAY_ENV, EditorEvent, GPU_PREVIEW_EXIT_CODE, GameLink,
    MasterVolume, PreviewMetadata, TimelineStatus, valid_display_metrics,
};
use crate::graphics::Renderer2d;
pub use crate::input::InputFrame;
pub use crate::launch::{
    RecordingOptions, RunConfig, TimelineCli, TimelineConfig, parse_timeline_cli,
};
use crate::offscreen::{GpuPreview, PreviewWorker};
use crate::pixels_renderer::{PixelsRenderer2d, env_bool};
use crate::reload::{self, ProcessCommand, RELOAD_EXIT_CODE};
use crate::surface::{DisplayMetrics, SurfaceSize};
use crate::tiles::{LoadedLevel, TileGrid, TileLevel};
use crate::tilesets::Tilesets;
use crate::timeline::{
    BookmarkEdit, ForkState, Frame, StateSchema, Timeline, TimelineCommand, TimelineWorker,
};

const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);

pub struct AppConfig {
    pub title: String,
    pub desired_size: SurfaceSize,
    pub clamp_to_monitor: bool,
}

/// The game-facing context shared by windowed and embedded rendering.
pub struct AppContext {
    /// The size in render pixels, kept for games that use pixel coordinates.
    pub surface_size: SurfaceSize,
    display: DisplayMetrics,
}

impl AppContext {
    pub fn new(display: DisplayMetrics) -> Self {
        Self {
            surface_size: display.render_size,
            display,
        }
    }

    pub fn for_pixels(size: SurfaceSize) -> Self {
        Self::new(DisplayMetrics::at_1x(size))
    }

    pub fn display(&self) -> DisplayMetrics {
        self.display
    }

    fn set_display(&mut self, display: DisplayMetrics, input: &mut InputFrame) {
        if self.display != display {
            input.mouse_pos = None;
        }
        *self = Self::new(display);
    }
}

/// Native window and presentation state for host-level applications.
struct WindowContext {
    window: Window,
    renderer: PixelsRenderer2d,
    app: AppContext,
}

impl WindowContext {
    fn resize(&mut self, size: PhysicalSize<u32>, scale_factor: f64, input: &mut InputFrame) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        let size = SurfaceSize::new(size.width, size.height);
        self.app
            .set_display(DisplayMetrics::for_surface(size, scale_factor), input);
        if size != self.renderer.size()
            && let Err(err) = self.renderer.resize(size)
        {
            eprintln!("resize failed: {err}");
        }
        self.window.request_redraw();
    }
}

pub trait GameApp {
    type State;

    /// Use logical points for pointer input and the requested standalone window size.
    const LOGICAL_DISPLAY: bool = false;

    fn init_state(&mut self, _ctx: &mut AppContext) -> Self::State;

    fn update_state(
        &mut self,
        state: &mut Self::State,
        input: &InputFrame,
        dt: Duration,
        _ctx: &mut AppContext,
    );

    fn render(&mut self, state: &Self::State, timeline_paused: bool, renderer: &mut dyn Renderer2d);

    /// Receives the engine's timings once the frame's CPU render work has completed.
    fn on_frame_timings(&mut self, _timings: FrameTimings) {}

    /// Live master volume advertised to the editor in percent.
    fn master_volume(&self, _state: &Self::State) -> Option<u8> {
        None
    }

    /// Apply a master volume adjustment, saving it when the user releases the slider.
    fn set_master_volume(&mut self, _state: &mut Self::State, _percent: u8, _commit: bool) {}

    fn exit_requested(&self) -> bool {
        false
    }

    fn shutdown(&mut self, _state: &Self::State) {}
}

fn update_game<G: GameApp>(
    game: &mut G,
    state: &mut G::State,
    input: &mut InputFrame,
    dt: Duration,
    ctx: &mut AppContext,
) {
    let pointer = input.mouse_pos;
    if G::LOGICAL_DISPLAY {
        input.mouse_pos = pointer.and_then(|point| ctx.display().logical_pointer(point));
    }
    game.update_state(state, input, dt, ctx);
    input.mouse_pos = pointer;
}

fn parse_present_mode(value: &str) -> Option<PresentMode> {
    match value.to_ascii_lowercase().as_str() {
        "auto" | "auto_vsync" | "vsync" => Some(PresentMode::AutoVsync),
        "auto_no_vsync" | "auto_novsync" | "no_vsync" | "novsync" => Some(PresentMode::AutoNoVsync),
        "fifo" => Some(PresentMode::Fifo),
        "mailbox" => Some(PresentMode::Mailbox),
        "immediate" => Some(PresentMode::Immediate),
        _ => None,
    }
}

fn create_app_context(
    config: &AppConfig,
    event_loop: &EventLoop<()>,
    logical_window: bool,
) -> Result<WindowContext, Box<dyn Error>> {
    let mut initial_size = config.desired_size;
    if config.clamp_to_monitor
        && let Some(monitor) = event_loop.primary_monitor()
    {
        let mut limit = SurfaceSize::new(monitor.size().width, monitor.size().height);
        if logical_window {
            limit = limit.to_logical(monitor.scale_factor());
        }
        initial_size.width = initial_size.width.min(limit.width);
        initial_size.height = initial_size.height.min(limit.height);
    }
    let initial_size = if logical_window {
        Size::Logical(LogicalSize::new(
            f64::from(initial_size.width),
            f64::from(initial_size.height),
        ))
    } else {
        Size::Physical(PhysicalSize::new(initial_size.width, initial_size.height))
    };
    let window = WindowBuilder::new()
        .with_title(config.title.clone())
        .with_inner_size(initial_size)
        .build(event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let vsync = env_bool("ROLLOUT_HEADFUL_VSYNC");
    let present_mode = env::var("ROLLOUT_HEADFUL_PRESENT_MODE")
        .ok()
        .as_deref()
        .and_then(parse_present_mode);
    let build_pixels = |present_mode: Option<PresentMode>| -> Result<Pixels, pixels::Error> {
        let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
        let mut pixels_builder =
            PixelsBuilder::new(surface_size.width, surface_size.height, surface_texture);
        if let Some(vsync) = vsync {
            pixels_builder = pixels_builder.enable_vsync(vsync);
        }
        if let Some(mode) = present_mode {
            pixels_builder = pixels_builder.present_mode(mode);
        }
        pixels_builder.build()
    };

    let pixels = if let Some(mode) = present_mode {
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
    let display = DisplayMetrics::for_surface(surface_size, window.scale_factor());
    Ok(WindowContext {
        window,
        renderer,
        app: AppContext::new(display),
    })
}

/// Hooks for a game whose simulation can continue from a recorded snapshot.
pub trait TimelineGame: GameApp {
    type Snapshot: Clone + Send + Sync + Serialize + DeserializeOwned + 'static;
    type PreviewSnapshot: Send + 'static;
    const SCHEMA: StateSchema;

    fn snapshot(state: &Self::State) -> &Self::Snapshot;
    fn restore_state(&mut self, snapshot: Self::Snapshot) -> Self::State;

    fn tile_level(_state: &Self::State) -> Option<TileLevel> {
        None
    }

    fn set_tile_grid(&mut self, _grid: Option<Arc<TileGrid>>) {}

    fn set_tilesets(&mut self, _tilesets: Arc<Tilesets>) {}

    /// Attach any presentation data to the shared game snapshot for the editor renderer.
    fn preview_snapshot(
        &self,
        state: Arc<Self::Snapshot>,
        timeline_paused: bool,
    ) -> Self::PreviewSnapshot;

    fn draw_preview(snapshot: Self::PreviewSnapshot, renderer: &mut dyn Renderer2d);
}

pub fn run_game<G>(mut config: AppConfig, game: G, run: RunConfig) -> Result<(), Box<dyn Error>>
where
    G: TimelineGame + 'static,
{
    let embedded = env::var_os(ADDRESS_ENV).is_some();
    let (timeline, trace) = match run {
        RunConfig::Live { timeline } => (timeline, None),
        RunConfig::Profile => {
            let profile = ProfileConfig::from_env(config.desired_size);
            config.desired_size = profile.window_size;
            (None, Some(TraceCapture::new(profile)))
        }
    };
    let Some(options) = timeline else {
        if embedded {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the editor requires timeline recording",
            )
            .into());
        }
        return run_windowed(config, game, None, trace);
    };
    let (mut timeline, mut snapshot) = Timeline::open(&options.directory, G::SCHEMA)?;
    if !options.playback {
        timeline.warm_seek_index();
    }
    let playback = if options.playback {
        snapshot = Some(timeline.pause_at_start()?);
        let from_ms = timeline
            .position()
            .expect("playback starts at a checkpoint")
            .at_ms;
        timeline
            .next_frame()?
            .map(|next| Playback::new(from_ms, next))
    } else {
        if !embedded && let Some(latest) = timeline.resume_tip()? {
            snapshot = Some(latest);
        }
        None
    };
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
    if !options.playback {
        timeline.enable_retention()?;
    }
    let session = TimelineSession {
        paused: timeline.is_paused(),
        cursor: timeline.position(),
        tip: timeline.tip_position()?,
        clock_position: timeline.position().map_or(0, |frame| frame.at_ms),
        clock_start: Instant::now(),
        playback,
        bookmark_revision: timeline.bookmark_revision(),
        detached_bookmark: timeline.detached_bookmark(),
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

impl Playback {
    fn new(from_ms: u64, next: Frame) -> Self {
        Self {
            from_ms,
            started: Instant::now(),
            next_ms: next.at_ms,
        }
    }
}

enum TimelineAction {
    Control(TimelineCommand),
    PlayUntil(u64),
}

enum TimelineInput {
    Control(TimelineCommand),
    Bookmark(BookmarkEdit),
    Reset,
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
    bookmark_revision: u64,
    detached_bookmark: Option<u64>,
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
        self.clock_start = Instant::now();
        if let Some(playback) = &mut self.playback {
            playback.started = self.clock_start;
        }
        Ok(state)
    }

    fn reset(&mut self, game: &mut G, state: &mut G::State, ctx: &mut AppContext) -> bool {
        let initial = game.init_state(ctx);
        match self.writer.reset(G::snapshot(&initial).clone()) {
            Ok(position) => {
                self.poll_capture();
                *state = initial;
                self.capture_failed = false;
                self.paused = false;
                self.cursor = Some(position);
                self.tip = Some(position);
                self.clock_position = position.at_ms;
                self.clock_start = Instant::now();
                self.playback = None;
                self.detached_bookmark = None;
                true
            }
            Err(err) => {
                *state = game.restore_state(G::snapshot(state).clone());
                eprintln!("could not reset timeline: {err}");
                false
            }
        }
    }

    fn apply_command(&mut self, game: &mut G, state: &mut G::State, command: TimelineCommand) {
        self.poll_capture();
        let command = match command {
            TimelineCommand::TogglePause | TimelineCommand::TogglePlayPause
                if self.detached_bookmark.is_some() =>
            {
                TimelineCommand::ReturnFromBookmark
            }
            TimelineCommand::TogglePlayPause if self.status().play_resumes_live() => {
                TimelineCommand::ResumeLive
            }
            TimelineCommand::TogglePlayPause => TimelineCommand::TogglePlayback,
            command => command,
        };
        let was_playing = self.playback.take().is_some();
        if command == TimelineCommand::TogglePlayback && was_playing {
            return;
        }
        self.apply_action(game, state, TimelineAction::Control(command));
    }

    fn advance_playback(&mut self, game: &mut G, state: &mut G::State) {
        self.poll_capture();
        let Some(playback) = &self.playback else {
            return;
        };
        let target = playback
            .from_ms
            .saturating_add(elapsed_ms(playback.started));
        if target >= playback.next_ms {
            self.apply_action(game, state, TimelineAction::PlayUntil(target));
        }
    }

    fn is_live(&self) -> bool {
        !self.paused
    }

    fn apply_action(&mut self, game: &mut G, state: &mut G::State, action: TimelineAction) {
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
                        TimelineCommand::OpenBookmark(id) => timeline.open_bookmark(id).map(Some),
                        TimelineCommand::ReturnFromBookmark => timeline.return_from_bookmark(),
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
                timeline.detached_bookmark(),
            ))
        });
        match result {
            Ok((paused, cursor, tip, changed, next, detached_bookmark)) => {
                self.poll_capture();
                self.paused = paused;
                self.cursor = cursor;
                self.detached_bookmark = detached_bookmark;
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
                            self.playback = Some(Playback::new(from_ms, next));
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
    }

    fn status(&self) -> TimelineStatus {
        TimelineStatus {
            cursor: self.cursor,
            tip: self.tip,
            paused: self.paused,
            playing: self.playback.is_some(),
            bookmark_revision: self.bookmark_revision,
            detached_bookmark: self.detached_bookmark,
        }
    }

    fn edit_bookmark(&mut self, state: &G::State, edit: BookmarkEdit) {
        let snapshot = (!self.paused && matches!(edit, BookmarkEdit::Save(_)))
            .then(|| G::snapshot(state).clone());
        let at_ms = self.live_time_ms();
        let result = self.writer.request(move |timeline| {
            let edit = match edit {
                BookmarkEdit::Save(name) => (|| {
                    if let Some(state) = snapshot {
                        timeline.checkpoint_at(&state, at_ms)?;
                    }
                    timeline.save_bookmark(&name).map(|_| ())
                })(),
                BookmarkEdit::Remove(id) => timeline.remove_bookmark(id),
            };
            Ok((edit, timeline.bookmark_revision(), timeline.position()))
        });
        match result {
            Ok((edit, revision, position)) => {
                self.poll_capture();
                self.bookmark_revision = revision;
                if !self.paused {
                    self.cursor = position;
                    self.tip = position;
                }
                if let Err(err) = edit {
                    eprintln!("could not edit bookmark: {err}");
                }
            }
            Err(err) => eprintln!("could not edit bookmark: {err}"),
        }
    }

    fn capture_frame(&mut self, snapshot: impl FnOnce() -> Arc<G::Snapshot>) {
        if self.capture_failed {
            return;
        }
        let at_ms = self.live_time_ms();
        if let Err(err) = self.writer.capture(snapshot(), at_ms) {
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

fn sync_tiles<G: TimelineGame>(
    game: &mut G,
    state: &G::State,
    loaded: &mut Option<LoadedLevel>,
    reload: bool,
    embedded: bool,
) -> io::Result<()> {
    if let Some(spec) = G::tile_level(state) {
        if let Some(level) = loaded.as_mut().filter(|level| level.matches(spec)) {
            if reload {
                level.refresh()?;
                game.set_tile_grid(Some(level.grid()));
            }
            if level.poll_assets() {
                game.set_tilesets(Arc::clone(&level.tilesets));
            }
        } else {
            let level = LoadedLevel::load(spec, embedded)?;
            game.set_tilesets(Arc::clone(&level.tilesets));
            game.set_tile_grid(Some(level.grid()));
            *loaded = Some(level);
        }
    } else if loaded.take().is_some() {
        game.set_tile_grid(None);
        game.set_tilesets(Arc::default());
    }
    Ok(())
}

fn run_embedded<G: TimelineGame>(
    mut game: G,
    mut session: TimelineSession<G>,
    editor: GameLink,
    preview: PreviewWorker<G::PreviewSnapshot>,
) -> Result<(), Box<dyn Error>> {
    let display = env::var(DISPLAY_ENV)
        .ok()
        .and_then(|encoded| serde_json::from_str(&encoded).ok())
        .filter(|display| valid_display_metrics(*display))
        .unwrap_or_else(|| DisplayMetrics::at_1x(DEFAULT_PREVIEW_SIZE));
    let mut ctx = AppContext::new(display);
    let mut state = session.start(&mut game, &mut ctx)?;
    let mut tiles = None;
    sync_tiles(&mut game, &state, &mut tiles, false, true)?;
    let overseer = reload::listen_for_overseer();
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut deadline = last_frame;
    let mut profile_enabled = false;
    let mut last_volume_request = None;
    // Keep the same snapshots the preview renders, including live states
    // that have not reached the timeline writer yet.
    const FORK_FRAMES: usize = 180;
    let first_stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut fork_frames = VecDeque::<(u64, bool, Arc<G::Snapshot>)>::with_capacity(FORK_FRAMES);
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
        let mut reload_tiles = false;
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
                        && matches!(
                            commands.last(),
                            Some(TimelineInput::Control(TimelineCommand::Seek(_)))
                        )
                    {
                        commands.pop();
                    }
                    commands.push(TimelineInput::Control(command));
                }
                EditorEvent::Bookmark(edit) => commands.push(TimelineInput::Bookmark(edit)),
                EditorEvent::ResetTimeline => commands.push(TimelineInput::Reset),
                EditorEvent::Resize(display) if valid_display_metrics(display) => {
                    ctx.set_display(display, &mut input);
                }
                EditorEvent::Profile(enabled) => profile_enabled = enabled,
                EditorEvent::MasterVolume {
                    percent,
                    request,
                    commit,
                } => {
                    volume_change = Some((percent.min(100), request));
                    commit_volume |= commit;
                }
                EditorEvent::ReloadTiles => reload_tiles = true,
                EditorEvent::ForkPreview { stamp, reply } => {
                    let snapshot = fork_frames.iter().find(|(id, _, _)| *id == stamp).cloned();
                    std::thread::spawn(move || {
                        let result = snapshot.ok_or_else(|| "That preview is no longer available. Try duplicating the current frame.".into())
                            .and_then(|(_, paused, state)| ForkState::capture(G::SCHEMA, paused, state.as_ref()).map_err(|err| err.to_string()));
                        ForkState::reply(&reply, result);
                    });
                }
                _ => input.apply_editor_event(event),
            }
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
        let should_update = if commands.is_empty() {
            session.advance_playback(&mut game, &mut state);
            session.is_live()
        } else {
            for command in commands {
                match command {
                    TimelineInput::Control(command) => {
                        session.apply_command(&mut game, &mut state, command)
                    }
                    TimelineInput::Bookmark(edit) => session.edit_bookmark(&state, edit),
                    TimelineInput::Reset => {
                        if session.reset(&mut game, &mut state, &mut ctx) {
                            input.apply_editor_event(EditorEvent::ReleaseInput);
                            tick = 0;
                        }
                    }
                }
            }
            false
        };
        if let Some((percent, request)) = volume_change {
            game.set_master_volume(&mut state, percent, commit_volume);
            last_volume_request = Some(request);
        }
        if let Err(err) = sync_tiles(&mut game, &state, &mut tiles, reload_tiles, true) {
            eprintln!("could not refresh tile level: {err}");
        }
        if should_update {
            let (_, span) =
                Span::measure(|| update_game(&mut game, &mut state, &mut input, dt, &mut ctx));
            update = span.elapsed;
            tick += 1;
            if let Err(err) = sync_tiles(&mut game, &state, &mut tiles, false, true) {
                eprintln!("could not load active tile level: {err}");
            }
        } else {
            last_frame = Instant::now();
        }

        let start = profile_enabled.then(Instant::now);
        let snapshot = Arc::new(G::snapshot(&state).clone());
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            profile.snapshot_us = micros(start.elapsed());
        }
        if should_update {
            let start = profile_enabled.then(Instant::now);
            session.capture_frame(|| Arc::clone(&snapshot));
            if let (Some(profile), Some(start)) = (&mut profile, start) {
                profile.timeline_us = micros(start.elapsed());
            }
        }
        let stamp = first_stamp.wrapping_add(frame);
        if fork_frames.len() == FORK_FRAMES {
            fork_frames.pop_front();
        }
        fork_frames.push_back((stamp, session.paused, Arc::clone(&snapshot)));
        let metadata = PreviewMetadata {
            stamp: Some(stamp),
            tile_level: tiles.as_ref().and_then(|level| level.preview),
            display: Some(ctx.display()),
            status: session.status(),
            master_volume: game.master_volume(&state).map(|percent| MasterVolume {
                percent: percent.min(100),
                request: last_volume_request,
            }),
        };
        frame += 1;
        let start = profile_enabled.then(Instant::now);
        let snapshot = game.preview_snapshot(snapshot, metadata.status.paused);
        if let (Some(profile), Some(start)) = (&mut profile, start) {
            profile.frame = frame;
            profile.tick = tick;
            profile.snapshot_us += micros(start.elapsed());
        }
        preview.publish(ctx.surface_size, metadata, snapshot, update, profile);
        input.clear_frame_transients();
        // Keep oversleep from accumulating across frames, and skip missed deadlines.
        let finished = Instant::now();
        deadline = (deadline + FRAME_INTERVAL).max(finished);
        std::thread::sleep(deadline.saturating_duration_since(finished));
    };
    game.shutdown(&state);
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
    let mut ctx = create_app_context(&config, &event_loop, G::LOGICAL_DISPLAY && trace.is_none())?;
    let mut state = match &mut session {
        Some(session) => session.start(&mut game, &mut ctx.app)?,
        None => game.init_state(&mut ctx.app),
    };
    let mut tiles = None;
    sync_tiles(&mut game, &state, &mut tiles, false, false)?;
    let overseer = reload::listen_for_overseer();
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut next_redraw = last_frame;
    let mut exiting = false;
    let mut previous_status = None;

    event_loop.run(move |event, _, control_flow| {
        if matches!(event, Event::LoopDestroyed) {
            if !exiting {
                exit_for_command(session.as_mut(), &state, ProcessCommand::Stop);
            }
            game.shutdown(&state);
            if let Some(trace) = trace.take() {
                trace.finish(ctx.renderer.size());
            }
            return;
        }
        if exiting {
            return;
        }
        *control_flow = ControlFlow::WaitUntil(next_redraw);
        if let Event::WindowEvent { event, .. } = &event {
            let scroll_scale = if G::LOGICAL_DISPLAY {
                ctx.app.display().scale_factor
            } else {
                1.0
            };
            input.apply_window_event(event, scroll_scale);
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

        if game.exit_requested()
            || matches!(
                event,
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                }
            )
        {
            exit_for_command(session.as_mut(), &state, ProcessCommand::Stop);
            exiting = true;
            *control_flow = ControlFlow::Exit;
            return;
        }

        match &event {
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                ctx.resize(*size, ctx.window.scale_factor(), &mut input);
            }
            Event::WindowEvent {
                event:
                    WindowEvent::ScaleFactorChanged {
                        scale_factor,
                        new_inner_size,
                    },
                ..
            } => {
                ctx.resize(**new_inner_size, *scale_factor, &mut input);
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                if now < next_redraw {
                    return;
                }
                next_redraw = now + FRAME_INTERVAL;
                *control_flow = ControlFlow::WaitUntil(next_redraw);
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;
                if let Some(session) = &mut session {
                    session.advance_playback(&mut game, &mut state);
                }
                if let Err(err) = sync_tiles(&mut game, &state, &mut tiles, false, false) {
                    eprintln!("could not load active tile level: {err}");
                }
                let update = if session.as_ref().is_none_or(TimelineSession::is_live) {
                    let (_, update) = Span::measure(|| {
                        update_game(&mut game, &mut state, &mut input, dt, &mut ctx.app)
                    });
                    if let Some(session) = &mut session {
                        session.capture_frame(|| Arc::new(G::snapshot(&state).clone()));
                    }
                    Some(update)
                } else {
                    last_frame = Instant::now();
                    None
                };
                if update.is_some()
                    && let Err(err) = sync_tiles(&mut game, &state, &mut tiles, false, false)
                {
                    eprintln!("could not load active tile level: {err}");
                }
                let status = session.as_ref().map(TimelineSession::status);
                if let Some(status) = status {
                    if previous_status != Some(status) {
                        if status.paused || previous_status.is_none_or(|previous| previous.paused) {
                            update_timeline_title(&ctx.window, &config.title, status);
                        }
                        previous_status = Some(status);
                    }
                }
                let timeline_paused = status.is_some_and(|status| status.paused);
                let draw = match ctx
                    .renderer
                    .draw_frame(|gfx| Span::measure(|| game.render(&state, timeline_paused, gfx)).1)
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
            Event::MainEventsCleared if Instant::now() >= next_redraw => {
                ctx.window.request_redraw()
            }
            _ => {}
        }
    });
}
