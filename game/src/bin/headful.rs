use std::{
    f32::consts::TAU,
    io::Cursor,
    time::{Duration, Instant},
};

use engine::HeadlessRunner;
use engine::surface::{Surface, SurfaceSize};
use pixels::{Pixels, SurfaceTexture};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use winit::{
    dpi::PhysicalSize,
    event::{ElementState, Event, KeyboardInput, MouseButton, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

use game::debug::DebugHud;
use game::playtest::{InputAction, TetrisLogic};
use game::sfx::{ACTION_SFX_VOLUME, LINE_CLEAR_SFX_VOLUME, MOVE_PIECE_SFX_VOLUME};
use game::tetris_core::Piece;
use game::tetris_ui::{draw_tetris, UiLayout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LrDir {
    Left,
    Right,
}

// Horizontal auto-shift (DAS/ARR) so holding left/right keeps moving even if you
// press other keys (e.g. rotate). Avoids relying on OS key-repeat.
#[derive(Debug, Clone)]
struct HorizontalAutoShift {
    left_held: bool,
    right_held: bool,
    last_pressed: Option<LrDir>,
    active: Option<LrDir>,
    next_repeat_at: Option<Duration>,
}

impl HorizontalAutoShift {
    const DAS: Duration = Duration::from_millis(170);
    const ARR: Duration = Duration::from_millis(50);

    fn new() -> Self {
        Self {
            left_held: false,
            right_held: false,
            last_pressed: None,
            active: None,
            next_repeat_at: None,
        }
    }

    fn active_dir(&self) -> Option<LrDir> {
        match (self.left_held, self.right_held) {
            (true, false) => Some(LrDir::Left),
            (false, true) => Some(LrDir::Right),
            (true, true) => self.last_pressed,
            (false, false) => None,
        }
    }

    fn on_key_down(&mut self, dir: LrDir, now: Duration) -> Option<InputAction> {
        // Ignore repeats (winit may emit Pressed repeatedly due to OS repeat).
        match dir {
            LrDir::Left if self.left_held => return None,
            LrDir::Right if self.right_held => return None,
            _ => {}
        }

        match dir {
            LrDir::Left => self.left_held = true,
            LrDir::Right => self.right_held = true,
        }
        self.last_pressed = Some(dir);

        let prev_active = self.active;
        let active = self.active_dir();
        self.active = active;

        if active != prev_active {
            self.next_repeat_at = active.map(|_| now + Self::DAS);
        }

        if active == Some(dir) {
            Some(match dir {
                LrDir::Left => InputAction::MoveLeft,
                LrDir::Right => InputAction::MoveRight,
            })
        } else {
            None
        }
    }

    fn on_key_up(&mut self, dir: LrDir, now: Duration) {
        match dir {
            LrDir::Left => self.left_held = false,
            LrDir::Right => self.right_held = false,
        }

        let prev_active = self.active;
        let active = self.active_dir();
        self.active = active;

        if active != prev_active {
            self.next_repeat_at = active.map(|_| now + Self::DAS);
        }
        if active.is_none() {
            self.next_repeat_at = None;
        }
    }

    fn tick(&mut self, now: Duration) -> Vec<InputAction> {
        let mut out = Vec::new();

        let Some(active) = self.active else {
            self.next_repeat_at = None;
            return out;
        };

        let Some(mut next_at) = self.next_repeat_at else {
            self.next_repeat_at = Some(now + Self::DAS);
            return out;
        };

        while now >= next_at {
            out.push(match active {
                LrDir::Left => InputAction::MoveLeft,
                LrDir::Right => InputAction::MoveRight,
            });
            next_at += Self::ARR;
        }
        self.next_repeat_at = Some(next_at);
        out
    }
}

#[derive(Debug, Clone)]
struct InputController {
    auto_shift: HorizontalAutoShift,
}

impl InputController {
    fn new() -> Self {
        Self {
            auto_shift: HorizontalAutoShift::new(),
        }
    }

    fn on_key(&mut self, key: VirtualKeyCode, state: ElementState, now: Duration) -> Vec<InputAction> {
        let Some(action) = map_key_to_action(key) else {
            return Vec::new();
        };

        match state {
            ElementState::Pressed => match action {
                InputAction::MoveLeft => self
                    .auto_shift
                    .on_key_down(LrDir::Left, now)
                    .into_iter()
                    .collect(),
                InputAction::MoveRight => self
                    .auto_shift
                    .on_key_down(LrDir::Right, now)
                    .into_iter()
                    .collect(),
                _ => vec![action],
            },
            ElementState::Released => {
                match action {
                    InputAction::MoveLeft => self.auto_shift.on_key_up(LrDir::Left, now),
                    InputAction::MoveRight => self.auto_shift.on_key_up(LrDir::Right, now),
                    _ => {}
                }
                Vec::new()
            }
        }
    }

    fn tick(&mut self, now: Duration) -> Vec<InputAction> {
        self.auto_shift.tick(now)
    }
}


const BGM_VOLUME: f32 = 0.08;
const BGM_SAMPLE_RATE: u32 = 44_100;
const BGM_CHANNELS: u16 = 2;
const BGM_AMPLITUDE: f32 = 0.25;

// (frequency_hz, duration_ms). freq=0.0 means "rest".
const DEFAULT_BGM_NOTES: &[(f32, u32)] = &[
    (220.0, 250), // A3
    (262.0, 250), // C4
    (330.0, 250), // E4
    (440.0, 400), // A4
    (392.0, 250), // G4
    (330.0, 250), // E4
    (262.0, 250), // C4
    (0.0, 150),   // rest
];

/// A tiny looping "default tune" background music source (no asset files).
///
/// This is intentionally very simple (a sine-ish beep melody) so we can ship BGM
/// without introducing new dependencies or binary assets.
#[derive(Debug, Clone)]
struct LoopingBgm {
    note_idx: usize,
    frame_in_note: u32,
    channel_in_frame: u16,

    note_freq_hz: f32,
    note_total_frames: u32,

    phase: f32,
    phase_step: f32,
    frame_sample: f32,
}

impl LoopingBgm {
    fn new() -> Self {
        let (freq_hz, total_frames) = Self::note_at(0);
        let phase_step = Self::phase_step(freq_hz);
        Self {
            note_idx: 0,
            frame_in_note: 0,
            channel_in_frame: 0,
            note_freq_hz: freq_hz,
            note_total_frames: total_frames,
            phase: 0.0,
            phase_step,
            frame_sample: 0.0,
        }
    }

    fn note_at(idx: usize) -> (f32, u32) {
        let (freq, ms) = DEFAULT_BGM_NOTES[idx % DEFAULT_BGM_NOTES.len()];
        let frames = ((ms as u64 * BGM_SAMPLE_RATE as u64) / 1000).max(1) as u32;
        (freq, frames)
    }

    fn phase_step(freq_hz: f32) -> f32 {
        if freq_hz <= 0.0 {
            0.0
        } else {
            TAU * freq_hz / BGM_SAMPLE_RATE as f32
        }
    }

    fn advance_note(&mut self) {
        self.note_idx = (self.note_idx + 1) % DEFAULT_BGM_NOTES.len();
        let (freq_hz, total_frames) = Self::note_at(self.note_idx);

        self.note_freq_hz = freq_hz;
        self.note_total_frames = total_frames;
        self.frame_in_note = 0;
        self.phase = 0.0;
        self.phase_step = Self::phase_step(freq_hz);
    }

    fn compute_frame_sample(&mut self) -> f32 {
        if self.note_freq_hz <= 0.0 {
            return 0.0;
        }

        // Gentle envelope to avoid clicks when switching notes or looping.
        let fade_frames = 200u32.min(self.note_total_frames / 4).max(1);
        let frames_left = self.note_total_frames.saturating_sub(self.frame_in_note);

        let env = if self.frame_in_note < fade_frames {
            self.frame_in_note as f32 / fade_frames as f32
        } else if frames_left <= fade_frames {
            frames_left as f32 / fade_frames as f32
        } else {
            1.0
        };

        // Slightly richer than a pure sine (still very cheap).
        let s1 = self.phase.sin();
        let s2 = (self.phase * 2.0).sin() * 0.30;
        let sample = (s1 + s2) * BGM_AMPLITUDE * env;

        self.phase += self.phase_step;
        if self.phase >= TAU {
            self.phase -= TAU;
        }

        sample
    }

    fn advance_frame(&mut self) {
        self.frame_in_note += 1;
        if self.frame_in_note >= self.note_total_frames {
            self.advance_note();
        }
    }
}

impl Iterator for LoopingBgm {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.channel_in_frame == 0 {
            self.frame_sample = self.compute_frame_sample();
        }

        let out = self.frame_sample;
        self.channel_in_frame += 1;
        if self.channel_in_frame >= BGM_CHANNELS {
            self.channel_in_frame = 0;
            self.advance_frame();
        }

        Some(out)
    }
}

impl Source for LoopingBgm {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        BGM_CHANNELS
    }

    fn sample_rate(&self) -> u32 {
        BGM_SAMPLE_RATE
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}


struct PixelsSurface {
    pixels: Pixels,
    size: SurfaceSize,
}

impl PixelsSurface {
    fn new(pixels: Pixels, size: SurfaceSize) -> Self {
        Self { pixels, size }
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
        self.pixels.resize_buffer(size.width, size.height)?;
        Ok(())
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        self.pixels.render()
    }
}

struct Sfx {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    _bgm: Option<Sink>,
    click_wav: &'static [u8],
}

impl Sfx {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (stream, handle) = OutputStream::try_default()?;

        // Best-effort background music; if it fails we still want click SFX.
        let bgm = Sink::try_new(&handle).ok().map(|sink| {
            sink.set_volume(BGM_VOLUME);
            sink.append(LoopingBgm::new());
            sink
        });

        Ok(Self {
            _stream: stream,
            handle,
            _bgm: bgm,
            click_wav: include_bytes!("../../assets/sfx/click.wav"),
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new();

    // Default to a larger window, but don't exceed the primary monitor's resolution.
    let desired = PhysicalSize::new(1920u32, 1080u32);
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

    let surface_texture = SurfaceTexture::new(
        initial_surface_size.width,
        initial_surface_size.height,
        &window,
    );
    let pixels = Pixels::new(
        initial_surface_size.width,
        initial_surface_size.height,
        surface_texture,
    )?;
    let mut surface = PixelsSurface::new(pixels, initial_surface_size);

    let logic = TetrisLogic::new(0, Piece::all());
    let mut runner = HeadlessRunner::new(logic);
    let sfx = Sfx::new().ok();
    let mut debug_hud = DebugHud::new();
    let mut last_layout = UiLayout::default();
    let mut mouse_x: u32 = 0;
    let mut mouse_y: u32 = 0;

    let start_time = Instant::now();
    let mut input_controller = InputController::new();

    let mut last_gravity = Instant::now();
    let gravity_interval = Duration::from_millis(500);
    let mut last_frame = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
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
                    if last_layout.hold_panel.contains(mouse_x, mouse_y) {
                        apply_action(&mut runner, sfx.as_ref(), &mut debug_hud, InputAction::Hold);
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
                } => {
                    let now = start_time.elapsed();

                    if key == VirtualKeyCode::F3 && state == ElementState::Pressed {
                        debug_hud.toggle();
                    }

                    for action in input_controller.on_key(key, state, now) {
                        apply_action(&mut runner, sfx.as_ref(), &mut debug_hud, action);
                    }
                }
                _ => {}
            },
            Event::MainEventsCleared => {
                let now = start_time.elapsed();
                for action in input_controller.tick(now) {
                    apply_action(&mut runner, sfx.as_ref(), &mut debug_hud, action);
                }

                if last_gravity.elapsed() >= gravity_interval {
                    let gravity_start = Instant::now();
                    runner.step_profiled(InputAction::SoftDrop, &mut debug_hud);
                    debug_hud.record_gravity(gravity_start.elapsed());
                    last_gravity = Instant::now();
                }
                window.request_redraw();
            }
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let frame_dt = now.duration_since(last_frame);
                last_frame = now;

                let frame_start = Instant::now();

                let board_start = Instant::now();
                let state = runner.state();
                let board_dt = board_start.elapsed();

                let draw_start = Instant::now();
                let size = surface.size();
                last_layout = draw_tetris(surface.frame_mut(), size.width, size.height, state);
                let draw_dt = draw_start.elapsed();

                let overlay_start = Instant::now();
                let size = surface.size();
                debug_hud.draw_overlay(surface.frame_mut(), size.width, size.height);
                let overlay_dt = overlay_start.elapsed();

                let present_start = Instant::now();
                if surface.present().is_err() {
                    *control_flow = ControlFlow::Exit;
                }
                let present_dt = present_start.elapsed();

                debug_hud.on_frame(
                    frame_dt,
                    board_dt,
                    draw_dt,
                    overlay_dt,
                    present_dt,
                    frame_start.elapsed(),
                );
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

    if let Some(sfx) = sfx {
        let after_pos = runner.state().current_piece_pos();
        let after_rot = runner.state().current_piece_rotation();
        let after_lines = runner.state().lines_cleared();
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

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

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
    fn holding_left_repeats_even_if_rotate_is_pressed() {
        let mut c = InputController::new();

        // Initial press should move immediately.
        assert_eq!(
            c.on_key(VirtualKeyCode::Left, ElementState::Pressed, ms(0)),
            vec![InputAction::MoveLeft]
        );

        // Rotate should not affect the held-left repeat.
        assert_eq!(
            c.on_key(VirtualKeyCode::Up, ElementState::Pressed, ms(10)),
            vec![InputAction::RotateCw]
        );

        // Before DAS: no auto shift yet.
        assert_eq!(c.tick(HorizontalAutoShift::DAS - ms(1)), Vec::<InputAction>::new());

        // At DAS: first repeat.
        assert_eq!(c.tick(HorizontalAutoShift::DAS), vec![InputAction::MoveLeft]);

        // Next repeat at DAS + ARR.
        assert_eq!(
            c.tick(HorizontalAutoShift::DAS + HorizontalAutoShift::ARR),
            vec![InputAction::MoveLeft]
        );
    }

    #[test]
    fn repeated_left_keydown_is_ignored_for_immediate_move_and_timers() {
        let mut c = InputController::new();

        assert_eq!(
            c.on_key(VirtualKeyCode::Left, ElementState::Pressed, ms(0)),
            vec![InputAction::MoveLeft]
        );

        // Simulate OS key-repeat emitting another Pressed without a Released.
        assert_eq!(
            c.on_key(VirtualKeyCode::Left, ElementState::Pressed, ms(30)),
            Vec::<InputAction>::new()
        );

        // We still get the first repeat at exactly DAS.
        assert_eq!(c.tick(HorizontalAutoShift::DAS), vec![InputAction::MoveLeft]);
    }

    #[test]
    fn bgm_source_is_infinite_and_produces_finite_samples() {
        let mut bgm = LoopingBgm::new();

        assert_eq!(bgm.channels(), BGM_CHANNELS);
        assert_eq!(bgm.sample_rate(), BGM_SAMPLE_RATE);
        assert!(bgm.total_duration().is_none());

        let samples: Vec<f32> = bgm.by_ref().take(10_000).collect();
        assert!(samples.iter().all(|s| s.is_finite()));
        assert!(samples.iter().any(|s| s.abs() > 1e-4));
    }
}
