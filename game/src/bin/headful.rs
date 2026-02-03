use std::{
    cell::Cell,
    fs,
    io::{self, Write},
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use engine::{HeadlessRunner, TimeMachine};
use engine::profiling::{Profiler, StepTimings};
use engine::surface::{Surface, SurfaceSize};
use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, Event, KeyboardInput, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use game::debug::{draw_text, DebugHud};
use game::gpu_renderer::{BufferMode, GpuTetrisRenderer};
use game::playtest::{InputAction, TetrisLogic};
use game::round_timer::RoundTimer;
use game::sfx::{ACTION_SFX_VOLUME, LINE_CLEAR_SFX_VOLUME, MOVE_PIECE_SFX_VOLUME, MUSIC_VOLUME};
use game::tetris_core::{Piece, TetrisCore};
use game::tetris_ui::{
    draw_game_over_menu, draw_game_over_menu_with_cursor, draw_main_menu, draw_main_menu_with_cursor, draw_pause_menu,
    draw_pause_menu_with_cursor, draw_skilltree, draw_skilltree_with_cursor, draw_tetris_hud_with_cursor,
    draw_tetris_world, GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, SkillTreeLayout, UiLayout,
};
use game::view::{GameView, GameViewEffect, GameViewEvent};

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse::<usize>().ok())
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.parse::<u32>().ok())
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

    engine_total: DurationAgg,
    draw: DurationAgg,
    overlay: DurationAgg,
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
            events: Vec::with_capacity(target_frames.saturating_mul(8)),
            engine_total: DurationAgg::default(),
            draw: DurationAgg::default(),
            overlay: DurationAgg::default(),
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

    fn record_frame_samples(&mut self, engine: Duration, draw: Duration, overlay: Duration, present: Duration, frame: Duration) {
        self.engine_total.push(engine);
        self.draw.push(draw);
        self.overlay.push(overlay);
        self.present.push(present);
        self.frame_total.push(frame);
        self.captured_frames = self.captured_frames.saturating_add(1);
    }

    fn default_trace_dir() -> PathBuf {
        // `CARGO_MANIFEST_DIR` is `.../rollout_engine/game`; the workspace `target/` lives at `..`.
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
        println!("headful profile summary (ms; lower is better)");
        println!(
            "engine(avg/max)  {:>7.3} / {:>7.3}",
            self.engine_total.avg_ms(),
            self.engine_total.max_ms()
        );
        println!(
            "draw  (avg/max)  {:>7.3} / {:>7.3}",
            self.draw.avg_ms(),
            self.draw.max_ms()
        );
        println!(
            "overlay(avg/max) {:>7.3} / {:>7.3}",
            self.overlay.avg_ms(),
            self.overlay.max_ms()
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

#[derive(Debug, Default)]
struct StepCapture {
    last: Option<(usize, StepTimings)>,
}

impl Profiler for StepCapture {
    fn on_step(&mut self, frame: usize, timings: StepTimings) {
        self.last = Some((frame, timings));
    }
}

struct PixelsSurface {
    pixels: Pixels,
    size: SurfaceSize,
    buffer_mode: BufferMode,
}

impl PixelsSurface {
    fn new(pixels: Pixels, size: SurfaceSize) -> Self {
        Self {
            pixels,
            size,
            buffer_mode: BufferMode::CpuMatchesSurface,
        }
    }

    fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    fn buffer_mode(&self) -> BufferMode {
        self.buffer_mode
    }

    fn set_buffer_mode(&mut self, mode: BufferMode) -> Result<(), pixels::Error> {
        if mode == self.buffer_mode {
            return Ok(());
        }

        match mode {
            BufferMode::CpuMatchesSurface => {
                self.pixels.resize_buffer(self.size.width, self.size.height)?;
            }
            BufferMode::GpuTiny => {
                self.pixels.resize_buffer(1, 1)?;
            }
        }

        self.buffer_mode = mode;
        Ok(())
    }
}

impl Surface for PixelsSurface {
    type Error = pixels::Error;

    fn size(&self) -> SurfaceSize {
        self.size
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        self.pixels.frame_mut()
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), Self::Error> {
        self.size = size;
        self.pixels.resize_surface(size.width, size.height)?;
        if self.buffer_mode == BufferMode::CpuMatchesSurface {
            self.pixels.resize_buffer(size.width, size.height)?;
        }
        Ok(())
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        self.pixels.render()
    }
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
        println!("controls: Space play/pause, Left/Right step, Home/End jump, Up/Down speed, Esc quit");
    }

    let event_loop = EventLoop::new();

    // If set, run a fixed-length headful perf capture:
    // - captures N frames
    // - emits a Chrome/Perfetto trace JSON to `target/perf_traces/`
    // - exits automatically (useful for CI-ish profiling runs)
    let profile_frames = env_usize("ROLLOUT_HEADFUL_PROFILE_FRAMES").unwrap_or(0);
    if replay_mode && profile_frames > 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --replay with ROLLOUT_HEADFUL_PROFILE_FRAMES (profiling mode)",
        )
        .into());
    }
    let profiling = profile_frames > 0;

    // Default to a larger window, but don't exceed the primary monitor's resolution.
    let desired = if let (Some(w), Some(h)) = (env_u32("ROLLOUT_HEADFUL_PROFILE_WIDTH"), env_u32("ROLLOUT_HEADFUL_PROFILE_HEIGHT"))
    {
        PhysicalSize::new(w.max(1), h.max(1))
    } else {
        PhysicalSize::new(1920u32, 1080u32)
    };
    let monitor_size = event_loop
        .primary_monitor()
        .map(|m| m.size())
        .unwrap_or(desired);
    let initial_size = PhysicalSize::new(
        desired.width.min(monitor_size.width),
        desired.height.min(monitor_size.height),
    );

    let window = WindowBuilder::new()
        .with_title("Tetree Headful")
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let initial_surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let vsync = env_bool("ROLLOUT_HEADFUL_VSYNC");
    let requested_present_mode = env_present_mode("ROLLOUT_HEADFUL_PRESENT_MODE");

    let build_pixels = |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
        let surface_texture = SurfaceTexture::new(
            initial_surface_size.width,
            initial_surface_size.height,
            &window,
        );
        let mut pixels_builder = PixelsBuilder::new(
            initial_surface_size.width,
            initial_surface_size.height,
            surface_texture,
        );
        if let Some(vsync) = vsync {
            pixels_builder = pixels_builder.enable_vsync(vsync);
        }
        if let Some(mode) = present_mode {
            pixels_builder = pixels_builder.present_mode(mode);
        }
        pixels_builder.build()
    };

    let pixels = if let Some(mode) = requested_present_mode {
        // Some present modes aren't supported on all platforms/surfaces; wgpu currently panics on
        // invalid present mode selection. Catch it so we can fall back instead of crashing.
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
            Ok(res) => res?,
            Err(_) => {
                eprintln!(
                    "warning: requested present mode {:?} via ROLLOUT_HEADFUL_PRESENT_MODE was not supported; falling back",
                    mode
                );
                build_pixels(None)?
            }
        }
    } else {
        build_pixels(None)?
    };
    let mut surface = PixelsSurface::new(pixels, initial_surface_size);

    // Default to GPU rendering (opt-out via ROLLOUT_HEADFUL_GPU=0).
    let gpu_enabled = env_bool("ROLLOUT_HEADFUL_GPU").unwrap_or(true);
    let mut gpu_renderer = gpu_enabled.then(|| {
        let p = surface.pixels();
        GpuTetrisRenderer::new(&p.context().device, p.surface_texture_format())
    });

    let logic = TetrisLogic::new(0, Piece::all());
    let mut runner = if let Some(path) = replay_path.as_ref() {
        let tm = TimeMachine::<TetrisCore>::load_json_file(path)?;
        let mut runner = HeadlessRunner::from_timemachine(logic.clone(), tm);
        runner.seek(0);
        runner
    } else {
        HeadlessRunner::new(logic.clone())
    };
    let sfx = Sfx::new().ok();
    let mut debug_hud = DebugHud::new();
    let mut last_layout = UiLayout::default();
    let mut last_main_menu = MainMenuLayout::default();
    let mut last_pause_menu = PauseMenuLayout::default();
    let mut last_skilltree = SkillTreeLayout::default();
    let mut last_game_over_menu = GameOverMenuLayout::default();
    let mut view = if profiling || replay_mode {
        GameView::Tetris { paused: false }
    } else {
        GameView::MainMenu
    };
    let mut mouse_x: u32 = 0;
    let mut mouse_y: u32 = 0;

    let mut last_gravity = Instant::now();
    let gravity_interval = Duration::from_millis(500);
    let mut last_frame = Instant::now();
    let mut horizontal_repeat = HorizontalRepeat::default();
    let mut round_timer = RoundTimer::new(Duration::from_secs(20));

    // Cap redraw rate to avoid burning CPU in a busy loop (ControlFlow::Poll).
    // Rendering is cheap in release; uncapped redraw mostly wastes CPU/power.
    let target_fps: u32 = 60;
    let frame_interval = Duration::from_secs_f64(1.0 / (target_fps as f64));
    let mut next_redraw = Instant::now();

    let mut trace = profiling.then(|| TraceCapture::new(profile_frames));
    let mut step_capture = StepCapture::default();

    let mut replay_playing = replay_mode;
    let mut replay_fps: u32 = 15;
    let mut replay_next_step = Instant::now();

    let recording_saved = Cell::new(false);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(next_redraw);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                WindowEvent::Focused(false) => {
                    horizontal_repeat.clear();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    mouse_x = position.x.max(0.0) as u32;
                    mouse_y = position.y.max(0.0) as u32;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if replay_mode {
                        return;
                    }
                    match view {
                        GameView::MainMenu => {
                            if last_main_menu.start_button.contains(mouse_x, mouse_y) {
                                let (next, effect) = view.handle(GameViewEvent::StartGame);
                                view = next;
                                if matches!(effect, GameViewEffect::ResetTetris) {
                                    runner = HeadlessRunner::new(logic.clone());
                                    last_gravity = Instant::now();
                                    round_timer.reset();
                                }
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            } else if last_main_menu.quit_button.contains(mouse_x, mouse_y) {
                                *control_flow = ControlFlow::Exit;
                            }
                        }
                        GameView::SkillTree => {
                            if last_skilltree.start_new_game_button.contains(mouse_x, mouse_y) {
                                let (next, effect) = view.handle(GameViewEvent::StartGame);
                                view = next;
                                if matches!(effect, GameViewEffect::ResetTetris) {
                                    runner = HeadlessRunner::new(logic.clone());
                                    last_gravity = Instant::now();
                                    horizontal_repeat.clear();
                                    round_timer.reset();
                                }
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            }
                        }
                        GameView::Tetris { paused } => {
                            if last_layout.pause_button.contains(mouse_x, mouse_y) {
                                let (next, _) = view.handle(GameViewEvent::TogglePause);
                                view = next;
                                last_gravity = Instant::now();
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            } else if paused {
                                if last_pause_menu.resume_button.contains(mouse_x, mouse_y) {
                                    let (next, _) = view.handle(GameViewEvent::TogglePause);
                                    view = next;
                                    last_gravity = Instant::now();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if last_pause_menu.end_run_button.contains(mouse_x, mouse_y) {
                                    let (next, _) = view.handle(GameViewEvent::GameOver);
                                    view = next;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                }
                            } else if last_layout.hold_panel.contains(mouse_x, mouse_y) {
                                apply_action(
                                    &mut runner,
                                    sfx.as_ref(),
                                    &mut debug_hud,
                                    InputAction::Hold,
                                );
                            }
                        }
                        GameView::GameOver => {
                            if last_game_over_menu.restart_button.contains(mouse_x, mouse_y) {
                                let (next, effect) = view.handle(GameViewEvent::StartGame);
                                view = next;
                                if matches!(effect, GameViewEffect::ResetTetris) {
                                    runner = HeadlessRunner::new(logic.clone());
                                    last_gravity = Instant::now();
                                    horizontal_repeat.clear();
                                    round_timer.reset();
                                }
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            } else if last_game_over_menu.skilltree_button.contains(mouse_x, mouse_y) {
                                let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                                view = next;
                                horizontal_repeat.clear();
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            } else if last_game_over_menu.quit_button.contains(mouse_x, mouse_y) {
                                *control_flow = ControlFlow::Exit;
                            }
                        }
                    }
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        let _ = surface.resize(SurfaceSize::new(size.width, size.height));
                    }
                }
                WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    if new_inner_size.width > 0 && new_inner_size.height > 0 {
                        let _ = surface.resize(SurfaceSize::new(new_inner_size.width, new_inner_size.height));
                    }
                }
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            state,
                            virtual_keycode: Some(key),
                            ..
                        },
                    ..
                } => match state {
                    ElementState::Pressed => {
                        let now = Instant::now();

                        if replay_mode {
                            match key {
                                VirtualKeyCode::Escape => {
                                    *control_flow = ControlFlow::Exit;
                                }
                                VirtualKeyCode::Space => {
                                    replay_playing = !replay_playing;
                                    replay_next_step = now;
                                }
                                VirtualKeyCode::Left => {
                                    runner.rewind(1);
                                    replay_playing = false;
                                }
                                VirtualKeyCode::Right => {
                                    runner.forward(1);
                                    replay_playing = false;
                                }
                                VirtualKeyCode::Home => {
                                    runner.seek(0);
                                    replay_playing = false;
                                }
                                VirtualKeyCode::End => {
                                    let last = runner.history().len().saturating_sub(1);
                                    runner.seek(last);
                                    replay_playing = false;
                                }
                                VirtualKeyCode::Up => {
                                    replay_fps = replay_fps.saturating_mul(2).min(240).max(1);
                                    replay_next_step = now;
                                }
                                VirtualKeyCode::Down => {
                                    replay_fps = (replay_fps / 2).max(1);
                                    replay_next_step = now;
                                }
                                _ => {}
                            }
                            return;
                        }

                        if key == VirtualKeyCode::F3 {
                            debug_hud.toggle();
                        }
                        if key == VirtualKeyCode::M {
                            if let Some(sfx) = sfx.as_ref() {
                                sfx.toggle_music();
                            }
                            return;
                        }

                        match view {
                            GameView::MainMenu => {
                                if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        runner = HeadlessRunner::new(logic.clone());
                                        last_gravity = Instant::now();
                                        horizontal_repeat.clear();
                                        round_timer.reset();
                                    }
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::Escape {
                                    *control_flow = ControlFlow::Exit;
                                }
                                return;
                            }
                            GameView::SkillTree => {
                                if key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::Back);
                                    view = next;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        runner = HeadlessRunner::new(logic.clone());
                                        last_gravity = Instant::now();
                                        horizontal_repeat.clear();
                                        round_timer.reset();
                                    }
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                }
                                return;
                            }
                            GameView::GameOver => {
                                if key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::Back);
                                    view = next;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        runner = HeadlessRunner::new(logic.clone());
                                        last_gravity = Instant::now();
                                        horizontal_repeat.clear();
                                        round_timer.reset();
                                    }
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::K {
                                    let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                                    view = next;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                }
                                return;
                            }
                            GameView::Tetris { paused } => {
                                if key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::TogglePause);
                                    view = next;
                                    last_gravity = Instant::now();
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                    return;
                                }

                                if paused {
                                    return;
                                }
                            }
                        }

                        // Horizontal movement: handle "held key repeat" ourselves so movement
                        // doesn't stop when the OS key-repeat gets redirected by other keypresses.
                        let horizontal = match key {
                            VirtualKeyCode::Left => Some(HorizontalDir::Left),
                            VirtualKeyCode::Right | VirtualKeyCode::D => Some(HorizontalDir::Right),
                            _ => None,
                        };

                        if let Some(dir) = horizontal {
                            if horizontal_repeat.on_press(dir, now) {
                                apply_action(
                                    &mut runner,
                                    sfx.as_ref(),
                                    &mut debug_hud,
                                    match dir {
                                        HorizontalDir::Left => InputAction::MoveLeft,
                                        HorizontalDir::Right => InputAction::MoveRight,
                                    },
                                );
                            }
                            return;
                        }

                        if let Some(action) = map_key_to_action(key) {
                            apply_action(
                                &mut runner,
                                sfx.as_ref(),
                                &mut debug_hud,
                                action,
                            );
                        }
                    }
                    ElementState::Released => {
                        if replay_mode {
                            return;
                        }
                        if !view.is_tetris() {
                            return;
                        }
                        let now = Instant::now();
                        match key {
                            VirtualKeyCode::Left => horizontal_repeat.on_release(HorizontalDir::Left, now),
                            VirtualKeyCode::Right | VirtualKeyCode::D => {
                                horizontal_repeat.on_release(HorizontalDir::Right, now)
                            }
                            _ => {}
                        }
                    }
                },
                _ => {}
            },
            Event::MainEventsCleared => {
                if replay_mode {
                    let now = Instant::now();
                    if replay_playing && now >= replay_next_step {
                        let max_frame = runner.history().len().saturating_sub(1);
                        if runner.frame() < max_frame {
                            runner.forward(1);
                        } else {
                            replay_playing = false;
                        }
                        let interval = Duration::from_secs_f64(1.0 / (replay_fps.max(1) as f64));
                        replay_next_step = now + interval;
                    }

                    if now >= next_redraw {
                        window.request_redraw();
                        next_redraw = now + frame_interval;
                    }
                    return;
                }

                // In profiling mode, we keep the run deterministic by stepping once per frame
                // (in RedrawRequested) and ignoring real-time gravity / key repeat.
                if trace.is_none() {
                    if view.is_tetris_playing() {
                        let now = Instant::now();
                        if let Some(action) = horizontal_repeat.next_repeat_action(now) {
                            apply_action(
                                &mut runner,
                                sfx.as_ref(),
                                &mut debug_hud,
                                action,
                            );
                        }
                    }
                    if view.is_tetris_playing() && last_gravity.elapsed() >= gravity_interval {
                        let gravity_start = Instant::now();
                        runner.step_profiled(InputAction::SoftDrop, &mut debug_hud);
                        debug_hud.record_gravity(gravity_start.elapsed());
                        last_gravity = Instant::now();
                    }
                }
                let now = Instant::now();
                if now >= next_redraw {
                    window.request_redraw();
                    next_redraw = now + frame_interval;
                }
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let frame_dt = now.duration_since(last_frame);
                last_frame = now;

                if !profiling && !replay_mode {
                    round_timer.tick_if_running(frame_dt, view.is_tetris_playing());
                    if view.is_tetris_playing() && round_timer.is_up() {
                        let (next, _) = view.handle(GameViewEvent::GameOver);
                        view = next;
                        horizontal_repeat.clear();
                    }
                }

                let frame_start = Instant::now();

                // Headful profiling mode: step once per rendered frame so `engine.*` spans appear
                // in the trace alongside draw/present costs.
                let mut engine_total_dt_for_frame = Duration::ZERO;
                if let Some(trace) = trace.as_mut() {
                    let engine_total_start = Instant::now();
                    runner.step_profiled(InputAction::Noop, &mut step_capture);
                    if let Some((frame, timings)) = step_capture.last.take() {
                        engine_total_dt_for_frame = timings.total;
                        debug_hud.on_step(frame, timings);
                        trace.record("engine.step", engine_total_start, timings.step);
                        trace.record("engine.record", engine_total_start + timings.step, timings.record);
                        trace.record("engine.total", engine_total_start, timings.total);
                    }
                }

                let board_start = Instant::now();
                let state = runner.state();
                let board_dt = board_start.elapsed();

                let size = surface.size();
                let want_gpu = gpu_enabled;
                let (draw_start, draw_dt, overlay_start, overlay_dt, present_start, present_dt) = if want_gpu {
                    if surface.buffer_mode() != BufferMode::GpuTiny {
                        if let Err(e) = surface.set_buffer_mode(BufferMode::GpuTiny) {
                            eprintln!("failed switching pixels buffer to GPU mode: {e}");
                            *control_flow = ControlFlow::Exit;
                            return;
                        }
                    }

                    let Some(gpu) = gpu_renderer.as_mut() else {
                        eprintln!("gpu renderer requested but not initialized");
                        *control_flow = ControlFlow::Exit;
                        return;
                    };

                    // Update UI layouts for hit-testing without CPU-rendering full frames.
                    let mut scratch = [0u8; 4];
                    match view {
                        GameView::MainMenu => {
                            last_main_menu = draw_main_menu(&mut scratch, size.width, size.height);
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::SkillTree => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = draw_skilltree(&mut scratch, size.width, size.height);
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::Tetris { paused: true } => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = draw_pause_menu(&mut scratch, size.width, size.height);
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::Tetris { paused: false } => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::GameOver => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu =
                                draw_game_over_menu(&mut scratch, size.width, size.height);
                        }
                    }

                    let timer_text = if view.is_tetris() && !replay_mode {
                        let remaining_s = round_timer.remaining().as_secs_f32();
                        Some(format!("TIME {remaining_s:>4.1}"))
                    } else {
                        None
                    };

                    let draw_start = Instant::now();
                    gpu.begin_frame();
                    last_layout = match view {
                        // Skilltree is its own scene: do not render the Tetris world beneath it.
                        GameView::SkillTree => {
                                gpu.push_skilltree(size.width, size.height, last_skilltree, Some((mouse_x, mouse_y)));
                            UiLayout::default()
                        }
                        // Main menu is its own scene: do not render the Tetris world beneath it.
                        GameView::MainMenu => UiLayout::default(),
                        _ => {
                            if view.is_tetris() {
                                gpu.push_tetris(
                                    size.width,
                                    size.height,
                                    state,
                                    timer_text.as_deref(),
                                        Some((mouse_x, mouse_y)),
                                )
                            } else {
                                gpu.push_tetris_world(size.width, size.height, state)
                            }
                        }
                    };
                    let draw_dt = draw_start.elapsed();

                    let overlay_start = Instant::now();
                    match view {
                        GameView::MainMenu => {
                            gpu.push_main_menu(size.width, size.height, last_main_menu, Some((mouse_x, mouse_y)))
                        }
                        GameView::SkillTree => {}
                        GameView::Tetris { paused: true } => {
                            gpu.push_pause_menu(size.width, size.height, last_pause_menu, Some((mouse_x, mouse_y)))
                        }
                        GameView::GameOver => {
                            gpu.push_game_over_menu(size.width, size.height, last_game_over_menu, Some((mouse_x, mouse_y)))
                        }
                        GameView::Tetris { paused: false } => {}
                    }

                    if debug_hud.is_enabled() {
                        let lines = debug_hud.lines();
                        gpu.push_debug_hud(size.width, size.height, &lines);
                    }

                    let overlay_dt = overlay_start.elapsed();

                    let present_start = Instant::now();
                    if let Err(e) = surface.pixels().render_with(|encoder, render_target, ctx| {
                        gpu.render(encoder, render_target, ctx, size.width, size.height);
                        Ok(())
                    }) {
                        eprintln!("gpu present failed: {e}");
                        *control_flow = ControlFlow::Exit;
                    }
                    let present_dt = present_start.elapsed();

                    (draw_start, draw_dt, overlay_start, overlay_dt, present_start, present_dt)
                } else {
                    if surface.buffer_mode() != BufferMode::CpuMatchesSurface {
                        if let Err(e) = surface.set_buffer_mode(BufferMode::CpuMatchesSurface) {
                            eprintln!("failed switching pixels buffer to CPU mode: {e}");
                            *control_flow = ControlFlow::Exit;
                            return;
                        }
                    }

                    let draw_start = Instant::now();
                    if matches!(view, GameView::SkillTree | GameView::MainMenu) {
                        // SkillTree and MainMenu are their own scenes; do not render the Tetris world beneath them.
                        last_layout = UiLayout::default();
                    } else {
                        let tetris_layout =
                            draw_tetris_world(surface.frame_mut(), size.width, size.height, state);
                        if view.is_tetris() {
                            draw_tetris_hud_with_cursor(
                                surface.frame_mut(),
                                size.width,
                                size.height,
                                state,
                                tetris_layout,
                                Some((mouse_x, mouse_y)),
                            );
                        }
                        last_layout = tetris_layout;
                    }

                    // Round timer HUD: show remaining time near the existing score/lines HUD.
                    if view.is_tetris() && !replay_mode {
                        let hud_x = last_layout.pause_button.x.saturating_sub(180);
                        let hud_y = last_layout.pause_button.y.saturating_add(6).saturating_add(28);
                        let remaining_s = round_timer.remaining().as_secs_f32();
                        let timer_text = format!("TIME {remaining_s:>4.1}");
                        draw_text(
                            surface.frame_mut(),
                            size.width,
                            size.height,
                            hud_x,
                            hud_y,
                            &timer_text,
                            [235, 235, 245, 255],
                        );
                    }

                    let draw_dt = draw_start.elapsed();

                    let overlay_start = Instant::now();
                    match view {
                        GameView::MainMenu => {
                            last_main_menu =
                                draw_main_menu_with_cursor(surface.frame_mut(), size.width, size.height, Some((mouse_x, mouse_y)));
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::SkillTree => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree =
                                draw_skilltree_with_cursor(surface.frame_mut(), size.width, size.height, Some((mouse_x, mouse_y)));
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::Tetris { paused: true } => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu =
                                draw_pause_menu_with_cursor(surface.frame_mut(), size.width, size.height, Some((mouse_x, mouse_y)));
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::Tetris { paused: false } => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::GameOver => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = draw_game_over_menu_with_cursor(
                                surface.frame_mut(),
                                size.width,
                                size.height,
                                Some((mouse_x, mouse_y)),
                            );
                        }
                    }
                    debug_hud.draw_overlay(surface.frame_mut(), size.width, size.height);
                    let overlay_dt = overlay_start.elapsed();

                    let present_start = Instant::now();
                    if surface.present().is_err() {
                        *control_flow = ControlFlow::Exit;
                    }
                    let present_dt = present_start.elapsed();

                    (draw_start, draw_dt, overlay_start, overlay_dt, present_start, present_dt)
                };

                let frame_total_dt = frame_start.elapsed();
                debug_hud.on_frame(
                    frame_dt,
                    board_dt,
                    draw_dt,
                    overlay_dt,
                    present_dt,
                    frame_total_dt,
                );

                if let Some(trace) = trace.as_mut() {
                    trace.record("render.draw_tetris", draw_start, draw_dt);
                    trace.record("render.debug_overlay", overlay_start, overlay_dt);
                    trace.record("render.present", present_start, present_dt);
                    trace.record("frame.total", frame_start, frame_total_dt);

                    // Summary samples (easy to read without opening the trace).
                    trace.record_frame_samples(
                        engine_total_dt_for_frame,
                        draw_dt,
                        overlay_dt,
                        present_dt,
                        frame_total_dt,
                    );

                    if trace.captured_frames >= trace.target_frames {
                        let size = surface.size();
                        match trace.write(size) {
                            Ok(path) => {
                                println!("trace written: {}", path.display());
                                trace.print_summary();
                            }
                            Err(e) => eprintln!("failed writing trace: {e}"),
                        }
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::LoopDestroyed => {
                if recording_saved.get() {
                    return;
                }
                let Some(path) = record_path.as_ref() else {
                    return;
                };

                match runner.timemachine().save_json_file(path) {
                    Ok(()) => {
                        println!("state recording saved: {}", path.display());
                    }
                    Err(e) => {
                        eprintln!("failed saving state recording to {}: {e}", path.display());
                    }
                }
                recording_saved.set(true);
            }
            _ => {}
        }
    });
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

fn apply_action(
    runner: &mut HeadlessRunner<TetrisLogic>,
    sfx: Option<&Sfx>,
    debug_hud: &mut DebugHud,
    action: InputAction,
) {
    let input_start = Instant::now();

    let before_pos = runner.state().current_piece_pos();
    let before_rot = runner.state().current_piece_rotation();
    let before_lines = runner.state().lines_cleared();
    let before_piece = runner.state().current_piece();
    let before_held = runner.state().held_piece();
    let before_can_hold = runner.state().can_hold();

    runner.step_profiled(action, debug_hud);

    let after_lines = runner.state().lines_cleared();

    if let Some(sfx) = sfx {
        let after_pos = runner.state().current_piece_pos();
        let after_rot = runner.state().current_piece_rotation();
        let after_piece = runner.state().current_piece();
        let after_held = runner.state().held_piece();
        let after_can_hold = runner.state().can_hold();

        let play_action_sfx = match action {
            InputAction::MoveLeft | InputAction::MoveRight | InputAction::SoftDrop => after_pos != before_pos,
            InputAction::RotateCw | InputAction::RotateCcw | InputAction::Rotate180 => after_rot != before_rot,
            InputAction::HardDrop => true,
            InputAction::Hold => {
                after_piece != before_piece || after_held != before_held || after_can_hold != before_can_hold
            }
            InputAction::Noop => false,
        };

        if play_action_sfx {
            let volume = match action {
                InputAction::MoveLeft | InputAction::MoveRight | InputAction::SoftDrop => MOVE_PIECE_SFX_VOLUME,
                InputAction::RotateCw
                | InputAction::RotateCcw
                | InputAction::Rotate180
                | InputAction::HardDrop
                | InputAction::Hold => ACTION_SFX_VOLUME,
                InputAction::Noop => ACTION_SFX_VOLUME,
            };
            sfx.play_click(volume);
        }

        if after_lines > before_lines {
            sfx.play_click(LINE_CLEAR_SFX_VOLUME);
        }
    }

    debug_hud.record_input(input_start.elapsed());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_a_maps_to_rotate_180() {
        assert_eq!(
            map_key_to_action(VirtualKeyCode::A),
            Some(InputAction::Rotate180)
        );
    }

    #[test]
    fn left_arrow_maps_to_move_left() {
        assert_eq!(
            map_key_to_action(VirtualKeyCode::Left),
            Some(InputAction::MoveLeft)
        );
    }

    #[test]
    fn horizontal_repeat_ignores_os_repeat_pressed_events_and_repeats_on_timer() {
        let mut repeat = HorizontalRepeat::default();
        let t0 = Instant::now();

        // Initial press should be accepted.
        assert!(repeat.on_press(HorizontalDir::Left, t0));

        // A repeated "Pressed" event (OS key-repeat) should be ignored and not reset the timer.
        assert!(!repeat.on_press(HorizontalDir::Left, t0 + Duration::from_millis(10)));

        // Before the repeat delay expires: no repeat.
        assert_eq!(
            repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY - Duration::from_millis(1)),
            None
        );

        // Once the delay expires: we should get a move action even without any further key events.
        assert_eq!(
            repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY),
            Some(InputAction::MoveLeft)
        );

        // And again at the interval.
        assert_eq!(
            repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY + HorizontalRepeat::REPEAT_INTERVAL),
            Some(InputAction::MoveLeft)
        );
    }
}
