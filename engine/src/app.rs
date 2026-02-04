use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, KeyboardInput, MouseButton, VirtualKeyCode, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use crate::graphics::Renderer2d;
use crate::pixels_renderer::PixelsRenderer2d;
use crate::surface::SurfaceSize;
use crate::ui_tree::UiInput;
use crate::view_tree::{hit_test_actions, ViewTree};
use crate::{RecordableState, ReplayableState};

pub struct AppConfig {
    pub title: String,
    pub desired_size: PhysicalSize<u32>,
    pub clamp_to_monitor: bool,
    pub vsync: Option<bool>,
    pub present_mode: Option<pixels::wgpu::PresentMode>,
}

#[derive(Debug, Clone)]
pub struct RecordingConfig {
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ReplayConfig {
    pub path: PathBuf,
    pub fps: u32,
}

#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub target_frames: usize,
}

pub struct AppContext {
    pub window: Window,
    pub renderer: PixelsRenderer2d,
    pub surface_size: SurfaceSize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct InputFrame {
    pub mouse_pos: Option<(u32, u32)>,
    pub mouse_down: bool,
    pub mouse_up: bool,
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
        view: &ViewTree<Self::Action>,
        renderer: &mut dyn Renderer2d,
    );

    fn handle_effects(&mut self, _effects: Vec<Self::Effect>, _ctx: &mut AppContext) {}

    fn handle_event(
        &mut self,
        _event: &Event<()>,
        _state: &mut Self::State,
        _input: &mut InputFrame,
        _ctx: &mut AppContext,
        _control_flow: &mut ControlFlow,
    ) -> bool {
        false
    }
}

pub trait AppHandler {
    fn init(&mut self, _ctx: &mut AppContext) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn handle_event(
        &mut self,
        event: Event<()>,
        control_flow: &mut ControlFlow,
        ctx: &mut AppContext,
    );
}

pub fn run_app<H: AppHandler + 'static>(config: AppConfig, mut handler: H) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
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
        .with_title(config.title)
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels = |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
        let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
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
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
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

    let mut ctx = AppContext {
        window,
        renderer,
        surface_size,
    };
    handler.init(&mut ctx)?;

    event_loop.run(move |event, _, control_flow| {
        handler.handle_event(event, control_flow, &mut ctx);
    });

    #[allow(unreachable_code)]
    Ok(())
}

pub fn run_game<G: GameApp + 'static>(
    config: AppConfig,
    mut game: G,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
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
        .with_title(config.title)
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels = |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
        let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
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
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
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

    let mut ctx = AppContext {
        window,
        renderer,
        surface_size,
    };
    let mut state = game.init_state(&mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if game.handle_event(&event, &mut state, &mut input, &mut ctx, control_flow) {
            return;
        }

        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::Resized(size) => {
                    ctx.surface_size = SurfaceSize::new(size.width, size.height);
                    if let Err(err) = ctx.renderer.resize(ctx.surface_size) {
                        eprintln!("resize failed: {err}");
                    }
                    ctx.window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let new_x = position.x.max(0.0) as u32;
                    let new_y = position.y.max(0.0) as u32;
                    input.mouse_pos = Some((new_x, new_y));
                }
                WindowEvent::MouseInput { state: mouse_state, button, .. } => {
                    if *button == MouseButton::Left {
                        match mouse_state {
                            ElementState::Pressed => {
                                input.mouse_down = true;
                            }
                            ElementState::Released => {
                                input.mouse_up = true;
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: input.mouse_pos,
                        mouse_down: input.mouse_down,
                        mouse_up: input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, input, dt, &actions, &mut ctx);

                let view_for_render = game.build_view(&state, &ctx);
                let draw_res = ctx.renderer.draw_frame(|gfx| {
                    game.render(&view_for_render, gfx);
                });
                if let Err(err) = draw_res {
                    eprintln!("draw failed: {err}");
                }
                if let Err(err) = ctx.renderer.present() {
                    eprintln!("present failed: {err}");
                }

                game.handle_effects(effects, &mut ctx);
                input.mouse_down = false;
                input.mouse_up = false;
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

pub fn run_game_with_recording<G>(
    config: AppConfig,
    mut game: G,
    recording: RecordingConfig,
) -> Result<(), Box<dyn Error>>
where
    G: GameApp + 'static,
    G::State: RecordableState,
{
    let event_loop = EventLoop::new();
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
        .with_title(config.title)
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels = |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
        let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
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
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
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

    let mut ctx = AppContext {
        window,
        renderer,
        surface_size,
    };
    let mut state = game.init_state(&mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut recording_saved = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if game.handle_event(&event, &mut state, &mut input, &mut ctx, control_flow) {
            return;
        }

        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    if !recording_saved {
                        if let Err(err) = state.save_recording(&recording.path) {
                            eprintln!(
                                "failed saving state recording to {}: {err}",
                                recording.path.display()
                            );
                        }
                        recording_saved = true;
                    }
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::Resized(size) => {
                    ctx.surface_size = SurfaceSize::new(size.width, size.height);
                    if let Err(err) = ctx.renderer.resize(ctx.surface_size) {
                        eprintln!("resize failed: {err}");
                    }
                    ctx.window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let new_x = position.x.max(0.0) as u32;
                    let new_y = position.y.max(0.0) as u32;
                    input.mouse_pos = Some((new_x, new_y));
                }
                WindowEvent::MouseInput { state: mouse_state, button, .. } => {
                    if *button == MouseButton::Left {
                        match mouse_state {
                            ElementState::Pressed => {
                                input.mouse_down = true;
                            }
                            ElementState::Released => {
                                input.mouse_up = true;
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: input.mouse_pos,
                        mouse_down: input.mouse_down,
                        mouse_up: input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, input, dt, &actions, &mut ctx);

                if !recording_saved && state.recording_frame() > 0 {
                    if let Err(err) = state.save_recording(&recording.path) {
                        eprintln!(
                            "failed saving state recording to {}: {err}",
                            recording.path.display()
                        );
                    } else {
                        println!("state recording saved: {}", recording.path.display());
                    }
                    recording_saved = true;
                }

                let view_for_render = game.build_view(&state, &ctx);
                let draw_res = ctx.renderer.draw_frame(|gfx| {
                    game.render(&view_for_render, gfx);
                });
                if let Err(err) = draw_res {
                    eprintln!("draw failed: {err}");
                }
                if let Err(err) = ctx.renderer.present() {
                    eprintln!("present failed: {err}");
                }

                game.handle_effects(effects, &mut ctx);
                input.mouse_down = false;
                input.mouse_up = false;
            }
            Event::MainEventsCleared => {
                ctx.window.request_redraw();
            }
            Event::LoopDestroyed => {
                if !recording_saved {
                    if let Err(err) = state.save_recording(&recording.path) {
                        eprintln!(
                            "failed saving state recording to {}: {err}",
                            recording.path.display()
                        );
                    } else {
                        println!("state recording saved: {}", recording.path.display());
                    }
                    recording_saved = true;
                }
            }
            _ => {}
        }
    });

    #[allow(unreachable_code)]
    Ok(())
}

pub fn run_game_with_replay<G>(
    config: AppConfig,
    mut game: G,
    replay: ReplayConfig,
) -> Result<(), Box<dyn Error>>
where
    G: GameApp + 'static,
    G::State: ReplayableState,
{
    let event_loop = EventLoop::new();
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
        .with_title(config.title)
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels =
        |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
            let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
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
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
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

    let mut ctx = AppContext {
        window,
        renderer,
        surface_size,
    };
    let initial_state = game.init_state(&mut ctx);
    let mut state = initial_state
        .replay_load(&replay.path)
        .map_err(|err| -> Box<dyn Error> { err.into() })?;
    let mut replay_playing = true;
    let mut replay_fps = replay.fps.max(1);
    let mut replay_next_step = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Event::WindowEvent {
            event:
                WindowEvent::KeyboardInput {
                    input:
                        KeyboardInput {
                            state: ElementState::Pressed,
                            virtual_keycode: Some(key),
                            ..
                        },
                    ..
                },
            ..
        } = &event
        {
            let now = Instant::now();
            match *key {
                VirtualKeyCode::Escape => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                VirtualKeyCode::Space => {
                    replay_playing = !replay_playing;
                    replay_next_step = now;
                    return;
                }
                VirtualKeyCode::Left => {
                    state.replay_rewind(1);
                    replay_playing = false;
                    return;
                }
                VirtualKeyCode::Right => {
                    state.replay_forward(1);
                    replay_playing = false;
                    return;
                }
                VirtualKeyCode::Home => {
                    state.replay_seek(0);
                    replay_playing = false;
                    return;
                }
                VirtualKeyCode::End => {
                    let last = state.replay_len().saturating_sub(1);
                    state.replay_seek(last);
                    replay_playing = false;
                    return;
                }
                VirtualKeyCode::Up => {
                    replay_fps = replay_fps.saturating_mul(2).min(240).max(1);
                    replay_next_step = now;
                    return;
                }
                VirtualKeyCode::Down => {
                    replay_fps = (replay_fps / 2).max(1);
                    replay_next_step = now;
                    return;
                }
                _ => {}
            }
        }

        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::Resized(size) => {
                    ctx.surface_size = SurfaceSize::new(size.width, size.height);
                    if let Err(err) = ctx.renderer.resize(ctx.surface_size) {
                        eprintln!("resize failed: {err}");
                    }
                    ctx.window.request_redraw();
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                if replay_playing && now >= replay_next_step {
                    let max_frame = state.replay_len().saturating_sub(1);
                    if state.replay_frame() < max_frame {
                        state.replay_forward(1);
                    } else {
                        replay_playing = false;
                    }
                    let interval = Duration::from_secs_f64(1.0 / (replay_fps.max(1) as f64));
                    replay_next_step = now + interval;
                }

                let view_for_render = game.build_view(&state, &ctx);
                let draw_res = ctx.renderer.draw_frame(|gfx| {
                    game.render(&view_for_render, gfx);
                });
                if let Err(err) = draw_res {
                    eprintln!("draw failed: {err}");
                }
                if let Err(err) = ctx.renderer.present() {
                    eprintln!("present failed: {err}");
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

pub fn run_game_with_profile<G>(
    config: AppConfig,
    mut game: G,
    profile: ProfileConfig,
) -> Result<(), Box<dyn Error>>
where
    G: GameApp + 'static,
{
    let event_loop = EventLoop::new();
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
        .with_title(config.title)
        .with_inner_size(initial_size)
        .build(&event_loop)?;

    let window_size = window.inner_size();
    let surface_size = SurfaceSize::new(window_size.width, window_size.height);

    let build_pixels =
        |present_mode: Option<pixels::wgpu::PresentMode>| -> Result<Pixels, pixels::Error> {
            let surface_texture = SurfaceTexture::new(surface_size.width, surface_size.height, &window);
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
        match std::panic::catch_unwind(|| build_pixels(Some(mode))) {
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

    let mut ctx = AppContext {
        window,
        renderer,
        surface_size,
    };
    let mut state = game.init_state(&mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut trace = TraceCapture::new(profile.target_frames);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if game.handle_event(&event, &mut state, &mut input, &mut ctx, control_flow) {
            return;
        }

        match &event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                WindowEvent::Resized(size) => {
                    ctx.surface_size = SurfaceSize::new(size.width, size.height);
                    if let Err(err) = ctx.renderer.resize(ctx.surface_size) {
                        eprintln!("resize failed: {err}");
                    }
                    ctx.window.request_redraw();
                }
                WindowEvent::CursorMoved { position, .. } => {
                    let new_x = position.x.max(0.0) as u32;
                    let new_y = position.y.max(0.0) as u32;
                    input.mouse_pos = Some((new_x, new_y));
                }
                WindowEvent::MouseInput { state: mouse_state, button, .. } => {
                    if *button == MouseButton::Left {
                        match mouse_state {
                            ElementState::Pressed => {
                                input.mouse_down = true;
                            }
                            ElementState::Released => {
                                input.mouse_up = true;
                            }
                        }
                    }
                }
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let frame_start = Instant::now();
                let update_start = Instant::now();
                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: input.mouse_pos,
                        mouse_down: input.mouse_down,
                        mouse_up: input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, input, dt, &actions, &mut ctx);
                let update_dt = update_start.elapsed();
                trace.record("engine.update", update_start, update_dt);

                let draw_start = Instant::now();
                let view_for_render = game.build_view(&state, &ctx);
                let draw_res = ctx.renderer.draw_frame(|gfx| {
                    game.render(&view_for_render, gfx);
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

                game.handle_effects(effects, &mut ctx);
                input.mouse_down = false;
                input.mouse_up = false;

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
        writeln!(f, "{\"traceEvents\":[")?;
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
