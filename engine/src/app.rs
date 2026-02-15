use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::fs;
use std::hash::Hash;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::dpi::PhysicalSize;
use winit::event::{
    ElementState, Event, KeyboardInput, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use crate::graphics::Renderer2d;
use crate::pixels_renderer::PixelsRenderer2d;
use crate::surface::SurfaceSize;
use crate::ui_tree::UiInput;
use crate::view_tree::{ViewTree, hit_test_actions};
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

#[derive(Debug, Clone, Default)]
pub struct CaptureCli {
    pub help: bool,
    pub record_path: Option<PathBuf>,
    pub replay_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Normal,
    Recording,
    Replay,
    Profile,
}

pub fn default_recording_path(app_tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("recordings")
        .join(format!("{app_tag}_{nanos}.json"))
}

pub fn parse_capture_cli_with_default_path(
    default_recording_path: impl Fn() -> PathBuf,
) -> io::Result<CaptureCli> {
    let mut cli = CaptureCli::default();
    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                cli.help = true;
            }
            "--record" => {
                cli.record_path = Some(
                    args.peek()
                        .filter(|next| !next.starts_with('-'))
                        .map_or_else(&default_recording_path, |p| PathBuf::from(p.clone())),
                );
                if args.peek().is_some_and(|next| !next.starts_with('-')) {
                    let _ = args.next();
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

    if cli.record_path.is_some() && cli.replay_path.is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot combine --record and --replay",
        ));
    }

    Ok(cli)
}

pub struct AppContext {
    pub window: Window,
    pub renderer: PixelsRenderer2d,
    pub surface_size: SurfaceSize,
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

    fn render(&mut self, view: &ViewTree<Self::Action>, renderer: &mut dyn Renderer2d);

    fn handle_effects(&mut self, _effects: Vec<Self::Effect>, _ctx: &mut AppContext) {}

    fn on_run_mode(&mut self, _mode: RunMode, _state: &mut Self::State, _ctx: &mut AppContext) {}

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

fn create_app_context(
    config: &AppConfig,
    event_loop: &EventLoop<()>,
) -> Result<AppContext, Box<dyn Error>> {
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
    Ok(AppContext {
        window,
        renderer,
        surface_size,
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

pub fn run_game<G: GameApp + 'static>(
    config: AppConfig,
    mut game: G,
) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new();
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = game.init_state(&mut ctx);
    game.on_run_mode(RunMode::Normal, &mut state, &mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Event::WindowEvent { event, .. } = &event {
            apply_window_event_to_input(&mut input, event);
        }

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
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let frame_input = input.clone();
                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: frame_input.mouse_pos,
                        mouse_down: frame_input.mouse_down,
                        mouse_up: frame_input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, frame_input, dt, &actions, &mut ctx);

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
                input.clear_frame_transients();
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
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = game.init_state(&mut ctx);
    game.on_run_mode(RunMode::Recording, &mut state, &mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut recording_saved = false;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Event::WindowEvent { event, .. } = &event {
            apply_window_event_to_input(&mut input, event);
        }

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
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let frame_input = input.clone();
                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: frame_input.mouse_pos,
                        mouse_down: frame_input.mouse_down,
                        mouse_up: frame_input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, frame_input, dt, &actions, &mut ctx);

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
                input.clear_frame_transients();
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
    let mut ctx = create_app_context(&config, &event_loop)?;
    let initial_state = game.init_state(&mut ctx);
    let mut state = initial_state
        .replay_load(&replay.path)
        .map_err(|err| -> Box<dyn Error> { err.into() })?;
    game.on_run_mode(RunMode::Replay, &mut state, &mut ctx);
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
    let mut ctx = create_app_context(&config, &event_loop)?;
    let mut state = game.init_state(&mut ctx);
    game.on_run_mode(RunMode::Profile, &mut state, &mut ctx);
    let mut input = InputFrame::default();
    let mut last_frame = Instant::now();
    let mut trace = TraceCapture::new(profile.target_frames);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;

        if let Event::WindowEvent { event, .. } = &event {
            apply_window_event_to_input(&mut input, event);
        }

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
                _ => {}
            },
            Event::RedrawRequested(_) => {
                let now = Instant::now();
                let dt = now.saturating_duration_since(last_frame);
                last_frame = now;

                let frame_start = Instant::now();
                let update_start = Instant::now();
                let frame_input = input.clone();
                let view_for_input = game.build_view(&state, &ctx);
                let actions = hit_test_actions(
                    &view_for_input,
                    UiInput {
                        mouse_pos: frame_input.mouse_pos,
                        mouse_down: frame_input.mouse_down,
                        mouse_up: frame_input.mouse_up,
                    },
                );
                let effects = game.update_state(&mut state, frame_input, dt, &actions, &mut ctx);
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
