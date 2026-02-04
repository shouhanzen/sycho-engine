use std::error::Error;
use std::time::{Duration, Instant};

use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

use crate::graphics::Renderer2d;
use crate::pixels_renderer::PixelsRenderer2d;
use crate::surface::SurfaceSize;
use crate::ui_tree::UiInput;
use crate::view_tree::{hit_test_actions, ViewTree};

pub struct AppConfig {
    pub title: String,
    pub desired_size: PhysicalSize<u32>,
    pub clamp_to_monitor: bool,
    pub vsync: Option<bool>,
    pub present_mode: Option<pixels::wgpu::PresentMode>,
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
