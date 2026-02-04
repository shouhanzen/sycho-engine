use std::{
    cell::Cell,
    fs,
    io::{self, Write},
    io::Cursor,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use engine::{HeadlessRunner, TimeMachine};
use engine::editor::{EditorSnapshot, EditorTimeline};
use engine::profiling::{Profiler, StepTimings};
use engine::pixels_renderer::PixelsRenderer2d;
use engine::surface::SurfaceSize;
use engine::ui_tree::{UiEvent, UiInput, UiTree};
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
    draw_skilltree_runtime_with_ui, draw_tetris_hud_with_ui, draw_tetris_world, GameOverMenuLayout,
    MainMenuLayout, PauseMenuLayout, Rect, SkillTreeLayout, UiLayout,
};
use game::ui_ids::*;
use game::view::{GameView, GameViewEffect, GameViewEvent};

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

    // Optional: expose the running headful game's timemachine over an HTTP API so the Tauri editor
    // can attach its timeline to this external game instance.
    let mut remote_editor_api = match env_u16("ROLLOUT_HEADFUL_EDITOR_PORT").unwrap_or(0) {
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

    // Backend selection is done once at startup (env-configurable) and hidden behind `Renderer2d`.
    let mut renderer = PixelsRenderer2d::new_auto(pixels, initial_surface_size)?;

    let base_logic = TetrisLogic::new(0, Piece::all());
    let mut runner = if let Some(path) = replay_path.as_ref() {
        let tm = TimeMachine::<GameState>::load_json_file(path)?;
        let mut runner = HeadlessRunner::from_timemachine(base_logic.clone(), tm);
        runner.seek(0);
        runner
    } else {
        HeadlessRunner::new(base_logic.clone())
    };
    if let Some(record_every) = env_usize("ROLLOUT_RECORD_EVERY_N_FRAMES") {
        runner.set_record_every_n_frames(record_every.max(1));
    }
    let sfx = Sfx::new().ok();
    let mut debug_hud = DebugHud::new();
    let mut ui_tree = UiTree::new();
    let mut last_layout = UiLayout::default();
    let mut last_main_menu = MainMenuLayout::default();
    let mut last_pause_menu = PauseMenuLayout::default();
    let mut last_skilltree = SkillTreeLayout::default();
    let mut last_game_over_menu = GameOverMenuLayout::default();
    let mut mouse_x: u32 = 0;
    let mut mouse_y: u32 = 0;
    let mut skilltree_cam_input = SkillTreeCameraInput::default();

    let base_gravity_interval = DEFAULT_GRAVITY_INTERVAL;
    let mut last_frame = Instant::now();
    let mut horizontal_repeat = HorizontalRepeat::default();
    let base_round_limit = DEFAULT_ROUND_LIMIT;

    if !replay_mode {
        let state = runner.state_mut();
        state.view = if profiling {
            GameView::Tetris { paused: false }
        } else {
            GameView::MainMenu
        };
        state.skilltree = SkillTreeRuntime::load_default();
        state.round_timer = RoundTimer::new(base_round_limit);
        state.gravity_interval = base_gravity_interval;
        state.gravity_elapsed = Duration::ZERO;
    }

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
                    let new_x = position.x.max(0.0) as u32;
                    let new_y = position.y.max(0.0) as u32;

                    let view = runner.state().view;
                    if matches!(view, GameView::SkillTree)
                        && skilltree_cam_input.left_down
                        && skilltree_cam_input.drag_started_in_view
                        && last_skilltree.grid_cell > 0
                    {
                        let dx = new_x as i32 - skilltree_cam_input.last_x as i32;
                        let dy = new_y as i32 - skilltree_cam_input.last_y as i32;

                        if !skilltree_cam_input.drag_started {
                            let total_dx = new_x as f32 - skilltree_cam_input.down_x as f32;
                            let total_dy = new_y as f32 - skilltree_cam_input.down_y as f32;
                            if total_dx * total_dx + total_dy * total_dy
                                >= SKILLTREE_DRAG_THRESHOLD_PX * SKILLTREE_DRAG_THRESHOLD_PX
                            {
                                skilltree_cam_input.drag_started = true;
                            }
                        }

                        if skilltree_cam_input.drag_started && (dx != 0 || dy != 0) {
                            let cell_px = (last_skilltree.grid_cell as f32).max(1.0);
                            // "Grab" drag: dragging right moves the world right (camera pans left).
                            let skilltree = &mut runner.state_mut().skilltree;
                            skilltree.camera.pan.x -= dx as f32 / cell_px;
                            skilltree.camera.pan.y += dy as f32 / cell_px;
                            skilltree.camera.target_pan = skilltree.camera.pan;

                            clamp_skilltree_camera_to_bounds(
                                skilltree,
                                last_skilltree.grid_cols,
                                last_skilltree.grid_rows,
                            );
                        }

                        skilltree_cam_input.last_x = new_x;
                        skilltree_cam_input.last_y = new_y;
                    }

                    mouse_x = new_x;
                    mouse_y = new_y;
                    let _ = ui_tree.process_input(UiInput {
                        mouse_pos: Some((mouse_x, mouse_y)),
                        mouse_down: false,
                        mouse_up: false,
                    });
                }
                WindowEvent::MouseWheel { delta, .. } => {
                    if replay_mode {
                        return;
                    }
                    let view = runner.state().view;
                    if !matches!(view, GameView::SkillTree) {
                        return;
                    }

                    let scroll_y = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(pos) => (pos.y as f32) / 120.0,
                    };
                    skilltree_cam_input.pending_scroll_y += scroll_y;
                }
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button: MouseButton::Left,
                    ..
                } => {
                    if replay_mode {
                        return;
                    }
                    let size = renderer.size();
                    if debug_hud.handle_click(mouse_x, mouse_y, size.width, size.height) {
                        return;
                    }
                    let _ = ui_tree.process_input(UiInput {
                        mouse_pos: Some((mouse_x, mouse_y)),
                        mouse_down: true,
                        mouse_up: false,
                    });
                    let view = runner.state().view;
                    if matches!(view, GameView::SkillTree) {
                        // SkillTree uses left-button drag panning. We defer click actions until
                        // release so we can distinguish click vs drag.
                        skilltree_cam_input.left_down = true;
                        skilltree_cam_input.drag_started = false;
                        skilltree_cam_input.drag_started_in_view = skilltree_grid_viewport(last_skilltree)
                            .map(|r| r.contains(mouse_x, mouse_y))
                            .unwrap_or(false);
                        skilltree_cam_input.down_x = mouse_x;
                        skilltree_cam_input.down_y = mouse_y;
                        skilltree_cam_input.last_x = mouse_x;
                        skilltree_cam_input.last_y = mouse_y;
                    }
                }
                WindowEvent::MouseInput {
                    state: ElementState::Released,
                    button: MouseButton::Left,
                    ..
                } => {
                    if replay_mode {
                        return;
                    }

                    let was_drag = skilltree_cam_input.drag_started;
                    skilltree_cam_input.left_down = false;
                    skilltree_cam_input.drag_started = false;
                    skilltree_cam_input.drag_started_in_view = false;

                    let ui_events = ui_tree.process_input(UiInput {
                        mouse_pos: Some((mouse_x, mouse_y)),
                        mouse_down: false,
                        mouse_up: true,
                    });
                    let mut ui_handled = false;
                    for event in ui_events {
                        if let UiEvent::Click {
                            action: Some(action),
                            ..
                        } = event
                        {
                            match action {
                                ACTION_MAIN_MENU_START => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::MainMenu) {
                                        let (next, effect) = view.handle(GameViewEvent::StartGame);
                                        runner.state_mut().view = next;
                                        if matches!(effect, GameViewEffect::ResetTetris) {
                                            reset_run(
                                                &mut runner,
                                                &base_logic,
                                                base_round_limit,
                                                base_gravity_interval,
                                                &mut horizontal_repeat,
                                            );
                                        }
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_MAIN_MENU_SKILLTREE_EDITOR => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::MainMenu) {
                                        let (next, _) = view.handle(GameViewEvent::OpenSkillTreeEditor);
                                        runner.state_mut().view = next;
                                        horizontal_repeat.clear();
                                        let skilltree = &mut runner.state_mut().skilltree;
                                        if !skilltree.editor.enabled {
                                            skilltree.editor_toggle();
                                        }
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_MAIN_MENU_QUIT => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::MainMenu) {
                                        *control_flow = ControlFlow::Exit;
                                        ui_handled = true;
                                    }
                                }
                                ACTION_TETRIS_TOGGLE_PAUSE => {
                                    let view = runner.state().view;
                                    if view.is_tetris() {
                                        let (next, _) = view.handle(GameViewEvent::TogglePause);
                                        let state = runner.state_mut();
                                        state.view = next;
                                        state.gravity_elapsed = Duration::ZERO;
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_TETRIS_HOLD => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::Tetris { paused: false }) {
                                        apply_action(
                                            &mut runner,
                                            sfx.as_ref(),
                                            &mut debug_hud,
                                            InputAction::Hold,
                                        );
                                        ui_handled = true;
                                    }
                                }
                                ACTION_PAUSE_RESUME => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::Tetris { paused: true }) {
                                        let (next, _) = view.handle(GameViewEvent::TogglePause);
                                        let state = runner.state_mut();
                                        state.view = next;
                                        state.gravity_elapsed = Duration::ZERO;
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_PAUSE_END_RUN => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::Tetris { paused: true }) {
                                        let earned = money_earned_from_run(runner.state());
                                        if earned > 0 {
                                            runner.state_mut().skilltree.add_money(earned);
                                        }
                                        let (next, _) = view.handle(GameViewEvent::GameOver);
                                        runner.state_mut().view = next;
                                        horizontal_repeat.clear();
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_GAME_OVER_RESTART => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::GameOver) {
                                        let (next, effect) = view.handle(GameViewEvent::StartGame);
                                        runner.state_mut().view = next;
                                        if matches!(effect, GameViewEffect::ResetTetris) {
                                            reset_run(
                                                &mut runner,
                                                &base_logic,
                                                base_round_limit,
                                                base_gravity_interval,
                                                &mut horizontal_repeat,
                                            );
                                        }
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_GAME_OVER_SKILLTREE => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::GameOver) {
                                        let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                                        runner.state_mut().view = next;
                                        horizontal_repeat.clear();
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_GAME_OVER_QUIT => {
                                    let view = runner.state().view;
                                    if matches!(view, GameView::GameOver) {
                                        *control_flow = ControlFlow::Exit;
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_START_RUN => {
                                    let view = runner.state().view;
                                    let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
                                    if matches!(view, GameView::SkillTree) && !skilltree_editor_enabled {
                                        let (next, effect) = view.handle(GameViewEvent::StartGame);
                                        runner.state_mut().view = next;
                                        if matches!(effect, GameViewEffect::ResetTetris) {
                                            reset_run(
                                                &mut runner,
                                                &base_logic,
                                                base_round_limit,
                                                base_gravity_interval,
                                                &mut horizontal_repeat,
                                            );
                                        }
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_TOOL_SELECT => {
                                    let view = runner.state().view;
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                                        skilltree.editor_set_tool(SkillTreeEditorTool::Select);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_TOOL_MOVE => {
                                    let view = runner.state().view;
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                                        skilltree.editor_set_tool(SkillTreeEditorTool::Move);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_TOOL_ADD_CELL => {
                                    let view = runner.state().view;
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                                        skilltree.editor_set_tool(SkillTreeEditorTool::AddCell);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_TOOL_REMOVE_CELL => {
                                    let view = runner.state().view;
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                                        skilltree.editor_set_tool(SkillTreeEditorTool::RemoveCell);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                ACTION_SKILLTREE_TOOL_LINK => {
                                    let view = runner.state().view;
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if matches!(view, GameView::SkillTree) && skilltree.editor.enabled {
                                        skilltree.editor_set_tool(SkillTreeEditorTool::ConnectPrereqs);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        ui_handled = true;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    if ui_handled {
                        return;
                    }

                    let view = runner.state().view;
                    if !matches!(view, GameView::SkillTree) {
                        return;
                    }
                    if was_drag {
                        return;
                    }

                    let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
                    if skilltree_editor_enabled {
                        if let Some(world) = skilltree_world_cell_at_screen(
                            &runner.state().skilltree,
                            last_skilltree,
                            mouse_x,
                            mouse_y,
                        ) {
                            let skilltree = &mut runner.state_mut().skilltree;
                            let hit_id =
                                skilltree_node_at_world(skilltree, world).map(|s| s.to_string());

                            match skilltree.editor.tool {
                                SkillTreeEditorTool::Select => {
                                    if let Some(id) = hit_id {
                                        skilltree.editor_select(&id, None);
                                        if let Some(sfx) = sfx.as_ref() {
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
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    } else {
                                        let grab = skilltree
                                            .editor
                                            .move_grab_offset
                                            .unwrap_or(Vec2i::new(0, 0));
                                        let new_pos = Vec2i::new(world.x - grab.x, world.y - grab.y);
                                        if skilltree.editor_move_selected_to(new_pos) {
                                            if let Some(sfx) = sfx.as_ref() {
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
                                            if let Some(sfx) = sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                            return;
                                        }
                                    }
                                    if skilltree.editor_add_cell_at_world(world) {
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    }
                                }
                                SkillTreeEditorTool::RemoveCell => {
                                    if let Some(id) = hit_id {
                                        let already =
                                            skilltree.editor.selected.as_deref() == Some(id.as_str());
                                        if !already {
                                            skilltree.editor_select(&id, None);
                                            if let Some(sfx) = sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                            return;
                                        }
                                    }
                                    if skilltree.editor_remove_cell_at_world(world) {
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                    }
                                }
                                SkillTreeEditorTool::ConnectPrereqs => {
                                    let Some(id) = hit_id else {
                                        return;
                                    };
                                    if skilltree.editor.connect_from.is_none() {
                                        skilltree.editor.connect_from = Some(id.clone());
                                        skilltree.editor_select(&id, None);
                                        if let Some(sfx) = sfx.as_ref() {
                                            sfx.play_click(ACTION_SFX_VOLUME);
                                        }
                                        return;
                                    }

                                    let from = skilltree.editor.connect_from.clone().unwrap_or_default();
                                    skilltree.editor.connect_from = None;
                                    if !from.is_empty() && from != id {
                                        if skilltree.editor_toggle_prereq(&from, &id) {
                                            if let Some(sfx) = sfx.as_ref() {
                                                sfx.play_click(ACTION_SFX_VOLUME);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        return;
                    }

                    if let Some(world) = skilltree_world_cell_at_screen(
                        &runner.state().skilltree,
                        last_skilltree,
                        mouse_x,
                        mouse_y,
                    ) {
                        let skilltree = &mut runner.state_mut().skilltree;
                        if let Some(node_id) =
                            skilltree_node_at_world(skilltree, world).map(|s| s.to_string())
                        {
                            if skilltree.try_buy(&node_id) {
                                if let Some(sfx) = sfx.as_ref() {
                                    sfx.play_click(ACTION_SFX_VOLUME);
                                }
                            }
                        }
                    }
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        let _ = renderer.resize(SurfaceSize::new(size.width, size.height));
                    }
                }
                WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    if new_inner_size.width > 0 && new_inner_size.height > 0 {
                        let _ = renderer.resize(SurfaceSize::new(new_inner_size.width, new_inner_size.height));
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

                        let mut view = runner.state().view;
                        match view {
                            GameView::MainMenu => {
                                if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            &mut runner,
                                            &base_logic,
                                            base_round_limit,
                                            base_gravity_interval,
                                            &mut horizontal_repeat,
                                        );
                                    }
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::K {
                                    let (next, _) = view.handle(GameViewEvent::OpenSkillTreeEditor);
                                    view = next;
                                    runner.state_mut().view = view;
                                    horizontal_repeat.clear();
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    if !skilltree.editor.enabled {
                                        skilltree.editor_toggle();
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
                                if key == VirtualKeyCode::F4 {
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    skilltree.editor_toggle();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                    return;
                                }

                                let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
                                if skilltree_editor_enabled {
                                    let skilltree = &mut runner.state_mut().skilltree;
                                    match key {
                                        VirtualKeyCode::Escape => {
                                            skilltree.editor_toggle();
                                            let (next, _) = view.handle(GameViewEvent::Back);
                                            view = next;
                                            runner.state_mut().view = view;
                                            horizontal_repeat.clear();
                                            if let Some(sfx) = sfx.as_ref() {
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
                                                last_skilltree,
                                                mouse_x,
                                                mouse_y,
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
                                                    skilltree.editor.status = Some(format!("SAVE FAILED: {e}"));
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
                                    return;
                                }

                                if key == VirtualKeyCode::Escape {
                                    let (next, _) = view.handle(GameViewEvent::Back);
                                    view = next;
                                    runner.state_mut().view = view;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            &mut runner,
                                            &base_logic,
                                            base_round_limit,
                                            base_gravity_interval,
                                            &mut horizontal_repeat,
                                        );
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
                                    runner.state_mut().view = view;
                                    horizontal_repeat.clear();
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                                    let (next, effect) = view.handle(GameViewEvent::StartGame);
                                    view = next;
                                    runner.state_mut().view = view;
                                    if matches!(effect, GameViewEffect::ResetTetris) {
                                        reset_run(
                                            &mut runner,
                                            &base_logic,
                                            base_round_limit,
                                            base_gravity_interval,
                                            &mut horizontal_repeat,
                                        );
                                    }
                                    if let Some(sfx) = sfx.as_ref() {
                                        sfx.play_click(ACTION_SFX_VOLUME);
                                    }
                                } else if key == VirtualKeyCode::K {
                                    let (next, _) = view.handle(GameViewEvent::OpenSkillTree);
                                    view = next;
                                    runner.state_mut().view = view;
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
                                    {
                                        let state = runner.state_mut();
                                        state.view = view;
                                        state.gravity_elapsed = Duration::ZERO;
                                    }
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
                        let view = runner.state().view;
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
                // Drain remote editor commands (if enabled) so external tools can query/seek the
                // live timemachine while the game is running.
                if let Some(remote) = remote_editor_api.as_mut() {
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
                                let _ = respond.send(snapshot(&runner));
                            }
                            RemoteCmd::GetTimeline { respond } => {
                                let _ = respond.send(timeline(&runner));
                            }
                            RemoteCmd::Step { action_id, respond } => {
                                match game::editor_api::action_from_id(&action_id) {
                                    Some(action) => {
                                        runner.step(action);
                                        let _ = respond.send(Ok(snapshot(&runner)));
                                    }
                                    None => {
                                        let _ = respond.send(Err(format!("unknown actionId: {action_id}")));
                                    }
                                }
                            }
                            RemoteCmd::Rewind { frames, respond } => {
                                runner.rewind(frames);
                                let _ = respond.send(snapshot(&runner));
                            }
                            RemoteCmd::Forward { frames, respond } => {
                                runner.forward(frames);
                                let _ = respond.send(snapshot(&runner));
                            }
                            RemoteCmd::Seek { frame, respond } => {
                                runner.seek(frame);
                                let _ = respond.send(snapshot(&runner));
                            }
                            RemoteCmd::Reset { respond } => {
                                reset_run(
                                    &mut runner,
                                    &base_logic,
                                    base_round_limit,
                                    base_gravity_interval,
                                    &mut horizontal_repeat,
                                );
                                let _ = respond.send(snapshot(&runner));
                            }
                        }
                    }
                }

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
                    let view = runner.state().view;
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
                    let mut trigger_game_over = false;
                    {
                        let state = runner.state_mut();
                        state
                            .round_timer
                            .tick_if_running(frame_dt, state.view.is_tetris_playing());
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
                        horizontal_repeat.clear();
                    }
                }

                if trace.is_none() && !replay_mode {
                    let mut gravity_steps = 0usize;
                    {
                        let state = runner.state_mut();
                        if state.view.is_tetris_playing() {
                            state.gravity_elapsed = state.gravity_elapsed.saturating_add(frame_dt);
                            while state.gravity_elapsed >= state.gravity_interval {
                                state.gravity_elapsed = state.gravity_elapsed.saturating_sub(state.gravity_interval);
                                gravity_steps = gravity_steps.saturating_add(1);
                            }
                        } else {
                            state.gravity_elapsed = Duration::ZERO;
                        }
                    }
                    for _ in 0..gravity_steps {
                        let gravity_start = Instant::now();
                        runner.step_profiled(InputAction::SoftDrop, &mut debug_hud);
                        debug_hud.record_gravity(gravity_start.elapsed());
                    }
                }

                let view = runner.state().view;

                // SkillTree camera controller (mouse drag / edge pan / scroll zoom).
                if matches!(view, GameView::SkillTree) {
                    let skilltree = &mut runner.state_mut().skilltree;
                    let dt_s = frame_dt.as_secs_f32();

                    // Consume scroll input once per frame (if the cursor is over the grid viewport).
                    let scroll_y = skilltree_cam_input.pending_scroll_y;
                    skilltree_cam_input.pending_scroll_y = 0.0;
                    if scroll_y != 0.0 {
                        let zoom_factor = 1.12f32.powf(scroll_y);
                        skilltree.camera.target_cell_px = (skilltree.camera.target_cell_px * zoom_factor)
                            .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX);

                        if let Some(viewport) = skilltree_grid_viewport(last_skilltree) {
                            if viewport.contains(mouse_x, mouse_y) && last_skilltree.grid_cell > 0 {
                                // World position under cursor before zoom (use the *current* camera).
                                let old_cell = last_skilltree.grid_cell.max(1) as f32;
                                let sx = mouse_x as f32 + 0.5;
                                let sy = mouse_y as f32 + 0.5;

                                let default_cam_min_x_old = -(last_skilltree.grid_cols as i32) / 2;
                                let cam_min_x_old = default_cam_min_x_old as f32 + skilltree.camera.pan.x;
                                let cam_min_y_old = skilltree.camera.pan.y;

                                let col_f = (sx - last_skilltree.grid_origin_x as f32) / old_cell;
                                let row_from_top_f = (sy - last_skilltree.grid_origin_y as f32) / old_cell;
                                let world_x = cam_min_x_old + col_f;
                                let world_y =
                                    cam_min_y_old + (last_skilltree.grid_rows as f32) - row_from_top_f;

                                // Predict the target viewport for the new zoom so we can zoom around the cursor.
                                let grid_cell_new = skilltree
                                    .camera
                                    .target_cell_px
                                    .round()
                                    .clamp(SKILLTREE_CAMERA_MIN_CELL_PX, SKILLTREE_CAMERA_MAX_CELL_PX)
                                    as u32;
                                if grid_cell_new > 0 {
                                    let grid = last_skilltree.grid;
                                    let grid_cols_new = grid.w / grid_cell_new;
                                    let grid_rows_new = grid.h / grid_cell_new;
                                    if grid_cols_new > 0 && grid_rows_new > 0 {
                                        let grid_pixel_w_new = grid_cols_new.saturating_mul(grid_cell_new);
                                        let grid_pixel_h_new = grid_rows_new.saturating_mul(grid_cell_new);
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

                    // Edge-of-screen panning (lerped via target-pan).
                    if !skilltree_cam_input.left_down {
                        if let Some(viewport) = skilltree_grid_viewport(last_skilltree) {
                            if viewport.contains(mouse_x, mouse_y) && last_skilltree.grid_cell > 0 {
                                let mx = mouse_x as f32;
                                let my = mouse_y as f32;
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
                                    let cell_px = (last_skilltree.grid_cell as f32).max(1.0);
                                    let dx_cells = (vx * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s)
                                        / cell_px;
                                    let dy_cells = (vy * SKILLTREE_EDGE_PAN_MAX_SPEED_PX_PER_S * dt_s)
                                        / cell_px;
                                    skilltree.camera.target_pan.x += dx_cells;
                                    skilltree.camera.target_pan.y += dy_cells;
                                    clamp_skilltree_camera_to_bounds(
                                        skilltree,
                                        last_skilltree.grid_cols,
                                        last_skilltree.grid_rows,
                                    );
                                }
                            }
                        }
                    }

                    // Snap current to target (no lerp).
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
                        last_skilltree.grid_cols,
                        last_skilltree.grid_rows,
                    );
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

                let size = renderer.size();

                ui_tree.begin_frame();
                ui_tree.ensure_canvas(UI_CANVAS, Rect::from_size(size.width, size.height));
                ui_tree.add_root(UI_CANVAS);
                let (draw_start, draw_dt, overlay_start, overlay_dt) = match renderer.draw_frame(|gfx| {
                    let draw_start = Instant::now();
                    if matches!(view, GameView::SkillTree | GameView::MainMenu) {
                        // SkillTree and MainMenu are their own scenes; do not render the Tetris world beneath them.
                        last_layout = UiLayout::default();
                    } else {
                        let tetris_layout =
                            draw_tetris_world(gfx, size.width, size.height, state.tetris());
                        if view.is_tetris() {
                            draw_tetris_hud_with_ui(
                                gfx,
                                size.width,
                                size.height,
                                state.tetris(),
                                tetris_layout,
                                &mut ui_tree,
                            );
                        }
                        last_layout = tetris_layout;
                    }

                    // Round timer HUD: show remaining time near the existing score/lines HUD.
                    if view.is_tetris() && !replay_mode {
                        let hud_x = last_layout.pause_button.x.saturating_sub(180);
                        let hud_y = last_layout
                            .pause_button
                            .y
                            .saturating_add(6)
                            .saturating_add(28);
                        let remaining_s = state.round_timer.remaining().as_secs_f32();
                        let timer_text = format!("TIME {remaining_s:>4.1}");
                        gfx.draw_text(hud_x, hud_y, &timer_text, [235, 235, 245, 255]);
                    }

                    let draw_dt = draw_start.elapsed();

                    let overlay_start = Instant::now();
                    match view {
                        GameView::MainMenu => {
                            last_main_menu = draw_main_menu_with_ui(
                                gfx,
                                size.width,
                                size.height,
                                &mut ui_tree,
                            );
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = SkillTreeLayout::default();
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::SkillTree => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = PauseMenuLayout::default();
                            last_skilltree = draw_skilltree_runtime_with_ui(
                                gfx,
                                size.width,
                                size.height,
                                &mut ui_tree,
                                &state.skilltree,
                            );
                            last_game_over_menu = GameOverMenuLayout::default();
                        }
                        GameView::Tetris { paused: true } => {
                            last_main_menu = MainMenuLayout::default();
                            last_pause_menu = draw_pause_menu_with_ui(
                                gfx,
                                size.width,
                                size.height,
                                &mut ui_tree,
                            );
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
                            last_game_over_menu = draw_game_over_menu_with_ui(
                                gfx,
                                size.width,
                                size.height,
                                &mut ui_tree,
                            );
                        }
                    }
                    debug_hud.draw_overlay(gfx, size.width, size.height);
                    let overlay_dt = overlay_start.elapsed();

                    (draw_start, draw_dt, overlay_start, overlay_dt)
                }) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("draw failed: {e}");
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                };

                let present_start = Instant::now();
                if let Err(e) = renderer.present() {
                    eprintln!("present failed: {e}");
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                let present_dt = present_start.elapsed();

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
                        let size = renderer.size();
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
                if let Some(remote) = remote_editor_api.as_mut() {
                    remote.shutdown();
                }

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
    fn only_hard_drop_plays_gameplay_action_sfx() {
        for action in [
            InputAction::Noop,
            InputAction::MoveLeft,
            InputAction::MoveRight,
            InputAction::SoftDrop,
            InputAction::RotateCw,
            InputAction::RotateCcw,
            InputAction::Rotate180,
            InputAction::Hold,
        ] {
            assert!(
                !should_play_action_sfx(action),
                "expected no action sfx for {action:?}"
            );
        }

        assert!(should_play_action_sfx(InputAction::HardDrop));
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
