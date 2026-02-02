use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use engine::HeadlessRunner;
use engine::surface::{Surface, SurfaceSize};
use pixels::{Pixels, SurfaceTexture};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
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
use game::tetris_ui::{
    draw_main_menu, draw_pause_menu, draw_tetris_with_fx, hard_drop_pulse_intensity, HardDropPulseFx, MainMenuLayout,
    PauseMenuLayout, UiLayout,
};

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
    click_wav: &'static [u8],
}

impl Sfx {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let (stream, handle) = OutputStream::try_default()?;
        Ok(Self {
            _stream: stream,
            handle,
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

#[derive(Debug, Default)]
struct HardDropPulse {
    start: Option<Instant>,
    lines_cleared: u32,
}

impl HardDropPulse {
    const DURATION: Duration = Duration::from_millis(240);

    fn trigger(&mut self, now: Instant, lines_cleared: u32) {
        self.start = Some(now);
        self.lines_cleared = lines_cleared.min(4);
    }

    fn fx(&mut self, now: Instant) -> Option<HardDropPulseFx> {
        let start = self.start?;
        let elapsed = now.duration_since(start);
        if elapsed >= Self::DURATION {
            self.start = None;
            return None;
        }

        let progress = elapsed.as_secs_f32() / Self::DURATION.as_secs_f32();
        Some(HardDropPulseFx {
            progress,
            intensity: hard_drop_pulse_intensity(self.lines_cleared),
        })
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
    let mut runner = HeadlessRunner::new(logic.clone());
    let sfx = Sfx::new().ok();
    let mut debug_hud = DebugHud::new();
    let mut last_layout = UiLayout::default();
    let mut last_main_menu = MainMenuLayout::default();
    let mut last_pause_menu = PauseMenuLayout::default();
    let mut show_main_menu = true;
    let mut paused = false;
    let mut hard_drop_pulse = HardDropPulse::default();
    let mut mouse_x: u32 = 0;
    let mut mouse_y: u32 = 0;

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
                    if show_main_menu {
                        if last_main_menu.start_button.contains(mouse_x, mouse_y) {
                            show_main_menu = false;
                            paused = false;
                            runner = HeadlessRunner::new(logic.clone());
                            last_gravity = Instant::now();
                            if let Some(sfx) = sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                        } else if last_main_menu.quit_button.contains(mouse_x, mouse_y) {
                            *control_flow = ControlFlow::Exit;
                        }
                    } else if last_layout.pause_button.contains(mouse_x, mouse_y) {
                        paused = !paused;
                        last_gravity = Instant::now();
                        if let Some(sfx) = sfx.as_ref() {
                            sfx.play_click(ACTION_SFX_VOLUME);
                        }
                    } else if paused {
                        if last_pause_menu.resume_button.contains(mouse_x, mouse_y) {
                            paused = false;
                            last_gravity = Instant::now();
                            if let Some(sfx) = sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                        }
                    } else if last_layout.hold_panel.contains(mouse_x, mouse_y) {
                        apply_action(
                            &mut runner,
                            sfx.as_ref(),
                            &mut debug_hud,
                            &mut hard_drop_pulse,
                            InputAction::Hold,
                        );
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
                            state: ElementState::Pressed,
                            virtual_keycode: Some(key),
                            ..
                        },
                    ..
                } => {
                    if key == VirtualKeyCode::F3 {
                        debug_hud.toggle();
                    }

                    if show_main_menu {
                        if key == VirtualKeyCode::Return || key == VirtualKeyCode::Space {
                            show_main_menu = false;
                            paused = false;
                            runner = HeadlessRunner::new(logic.clone());
                            last_gravity = Instant::now();
                            if let Some(sfx) = sfx.as_ref() {
                                sfx.play_click(ACTION_SFX_VOLUME);
                            }
                        } else if key == VirtualKeyCode::Escape {
                            *control_flow = ControlFlow::Exit;
                        }
                    } else if key == VirtualKeyCode::Escape {
                        paused = !paused;
                        last_gravity = Instant::now();
                        if let Some(sfx) = sfx.as_ref() {
                            sfx.play_click(ACTION_SFX_VOLUME);
                        }
                    } else if !paused {
                        if let Some(action) = map_key_to_action(key) {
                            apply_action(
                                &mut runner,
                                sfx.as_ref(),
                                &mut debug_hud,
                                &mut hard_drop_pulse,
                                action,
                            );
                        }
                    }
                }
                _ => {}
            },
            Event::MainEventsCleared => {
                if !show_main_menu && !paused && last_gravity.elapsed() >= gravity_interval {
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
                let pulse_fx = hard_drop_pulse.fx(now);
                last_layout = draw_tetris_with_fx(surface.frame_mut(), size.width, size.height, state, pulse_fx);
                let draw_dt = draw_start.elapsed();

                let overlay_start = Instant::now();
                let size = surface.size();
                if show_main_menu {
                    last_main_menu = draw_main_menu(surface.frame_mut(), size.width, size.height);
                    last_pause_menu = PauseMenuLayout::default();
                } else if paused {
                    last_main_menu = MainMenuLayout::default();
                    last_pause_menu = draw_pause_menu(surface.frame_mut(), size.width, size.height);
                } else {
                    last_main_menu = MainMenuLayout::default();
                    last_pause_menu = PauseMenuLayout::default();
                }
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
    hard_drop_pulse: &mut HardDropPulse,
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
    if action == InputAction::HardDrop {
        let cleared = after_lines.saturating_sub(before_lines);
        hard_drop_pulse.trigger(input_start, cleared);
    }

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
}
