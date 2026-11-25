use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use engine::embedded::{PreviewFrame, TimelineStatus};
use engine::graphics::{Color, GpuRenderer2d, Renderer2d};
#[cfg(target_os = "macos")]
use engine::shared_surface::Surface;
use engine::surface::SurfaceSize;
use engine::ui::Rect;
use pixels::{Pixels, wgpu};

use super::board::{
    Board, Bounds, Panel, Point, TITLE_HEIGHT, TRANSPORT_CONTROLS, TimelineLayout, Transport,
};
use super::{Phase, dock_command};

const CANVAS: Color = [235, 229, 216, 255];
const GRID: Color = [210, 201, 183, 255];
const PAPER: Color = [253, 249, 238, 255];
const HEADER: Color = [242, 235, 220, 255];
const INK: Color = [18, 24, 20, 255];
const MUTED: Color = [49, 57, 45, 255];
const BORDER: Color = [179, 172, 154, 255];
const GREEN: Color = [18, 49, 33, 255];
const GREEN_LIGHT: Color = [224, 234, 213, 255];
const RED: Color = [110, 35, 25, 255];
const BLACK: Color = [14, 19, 19, 255];
const WHITE: Color = [251, 246, 230, 255];

pub fn viewport(size: SurfaceSize) -> Bounds {
    Bounds::new(0.0, 0.0, f64::from(size.width), f64::from(size.height))
}

#[derive(Clone, Copy, PartialEq)]
pub struct Chrome {
    pub board: Board,
    pub size: SurfaceSize,
    pub phase: Phase,
    pub connected: bool,
    pub frame_size: Option<SurfaceSize>,
    pub timeline: TimelineStatus,
    pub preview_focus: bool,
    pub hovered: Option<Transport>,
    pub drag_ms: Option<u64>,
    pub dock_target: bool,
}

pub struct Canvas {
    ui: GpuRenderer2d,
    preview: GameBlit,
    drawn: Option<Chrome>,
    #[cfg(target_os = "macos")]
    pending_gpu: Arc<AtomicUsize>,
}

impl Canvas {
    pub fn new(pixels: &Pixels) -> Self {
        let format = pixels.surface_texture_format();
        let device = &pixels.context().device;
        Self {
            ui: GpuRenderer2d::new(device, format),
            preview: GameBlit::new(device, format),
            drawn: None,
            #[cfg(target_os = "macos")]
            pending_gpu: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn draw(
        &mut self,
        pixels: &Pixels,
        chrome: Chrome,
        surface: SurfaceSize,
        frame: Option<&Arc<PreviewFrame>>,
    ) -> Result<(), pixels::Error> {
        if self.drawn != Some(chrome) {
            self.ui.begin_frame(chrome.size);
            draw_board(&mut self.ui, chrome);
            self.drawn = Some(chrome);
        }
        let area = chrome.board.project(chrome.board.game_body());
        let visible = area
            .intersect(viewport(chrome.size))
            .map_or_else(Vec::new, |clipped| {
                let pieces = if chrome.board.timeline_docked {
                    vec![clipped]
                } else {
                    clipped.excluding(chrome.board.project(chrome.board.game.grip()))
                };
                if chrome.board.front == Panel::Timeline {
                    let timeline = chrome.board.timeline;
                    let cover =
                        Bounds::new(timeline.x, timeline.y, timeline.w + 4.0, timeline.h + 5.0);
                    pieces
                        .into_iter()
                        .flat_map(|piece| piece.excluding(chrome.board.project(cover)))
                        .collect()
                } else {
                    pieces
                }
            });
        let has_image = !visible.is_empty() && frame.is_some();
        if let Some(frame) = frame.filter(|_| has_image) {
            self.preview.upload(pixels.context(), frame);
            self.preview
                .place(&pixels.context().queue, area, chrome.size);
        }
        let result = pixels.render_with(|encoder, target, context| {
            self.ui
                .render(encoder, target, &context.device, &context.queue);
            if has_image {
                self.preview
                    .draw(encoder, target, &visible, chrome.size, surface);
            }
            Ok(())
        });
        #[cfg(target_os = "macos")]
        if result.is_ok()
            && has_image
            && let Some(image) = &self.preview.image
            && image.frame.surface.is_some()
        {
            let frame = image.frame.clone();
            let pending = self.pending_gpu.clone();
            pending.fetch_add(1, Ordering::Relaxed);
            // The game may render into this IOSurface again only after the
            // last editor submission that samples it has finished.
            pixels.context().queue.on_submitted_work_done(move || {
                drop(frame);
                pending.fetch_sub(1, Ordering::Relaxed);
            });
        }
        result
    }

    #[cfg(target_os = "macos")]
    pub fn poll_gpu(&self, pixels: &Pixels, flow: &mut winit::event_loop::ControlFlow) {
        use winit::event_loop::ControlFlow;
        if self.pending_gpu.load(Ordering::Relaxed) == 0 {
            return;
        }
        pixels.context().device.poll(wgpu::Maintain::Poll);
        if self.pending_gpu.load(Ordering::Relaxed) == 0 {
            return;
        }
        // Keep servicing GPU completions if the producer has temporarily run
        // out of surfaces and there are no more IPC-driven redraws.
        let poll_at = Instant::now() + Duration::from_millis(2);
        *flow = match *flow {
            ControlFlow::Wait => ControlFlow::WaitUntil(poll_at),
            ControlFlow::WaitUntil(at) => ControlFlow::WaitUntil(at.min(poll_at)),
            current => current,
        };
    }
}

struct Painter<'a> {
    gfx: &'a mut dyn Renderer2d,
    screen: Bounds,
    board: Board,
}

impl Painter<'_> {
    fn screen_fill(&mut self, bounds: Bounds, color: Color) {
        if let Some(rect) = bounds.intersect(self.screen) {
            let x = rect.x.round() as u32;
            let y = rect.y.round() as u32;
            let w = (rect.right().round() as u32).saturating_sub(x);
            let h = (rect.bottom().round() as u32).saturating_sub(y);
            if w > 0 && h > 0 {
                self.gfx.fill_rect(Rect::new(x, y, w, h), color);
            }
        }
    }

    fn fill(&mut self, bounds: Bounds, color: Color) {
        self.screen_fill(self.board.project(bounds), color);
    }

    fn screen_text(&mut self, point: Point, text: &str, color: Color, scale: u32) {
        if self.screen.contains(point) {
            self.gfx.draw_text_scaled(
                point.x.round() as u32,
                point.y.round() as u32,
                text,
                color,
                scale,
            );
        }
    }

    fn text(&mut self, point: Point, text: &str, color: Color) {
        if self.board.zoom >= 0.36 {
            let scale = (2.0 * self.board.zoom).round().max(1.0) as u32;
            self.screen_text(self.board.screen_at(point), text, color, scale);
        }
    }

    fn centered_text(&mut self, bounds: Bounds, label: &str, color: Color) {
        if self.board.zoom < 0.36 {
            return;
        }
        let bounds = self.board.project(bounds);
        let glyphs = label.chars().count() as f64;
        let scale = (2.0 * self.board.zoom)
            .round()
            .min(((bounds.w - 6.0) / (glyphs * 4.0)).floor())
            .min(((bounds.h - 4.0) / 5.0).floor()) as u32;
        if scale > 0 {
            let scale_px = f64::from(scale);
            self.screen_text(
                Point::new(
                    bounds.x + (bounds.w - glyphs * 4.0 * scale_px) / 2.0,
                    bounds.y + (bounds.h - 5.0 * scale_px) / 2.0,
                ),
                label,
                color,
                scale,
            );
        }
    }

    fn icon(&mut self, bounds: Bounds, rows: [u8; 7], color: Color) {
        let x = bounds.x + (bounds.w - 14.0) / 2.0;
        let y = bounds.y + (bounds.h - 14.0) / 2.0;
        for (row, bits) in rows.into_iter().enumerate() {
            for column in 0..7 {
                if bits & (1 << (6 - column)) != 0 {
                    self.fill(
                        Bounds::new(x + f64::from(column) * 2.0, y + row as f64 * 2.0, 2.0, 2.0),
                        color,
                    );
                }
            }
        }
    }
}

fn draw_board(gfx: &mut dyn Renderer2d, chrome: Chrome) {
    let board = chrome.board;
    let width = f64::from(chrome.size.width);
    let height = f64::from(chrome.size.height);
    let screen = Bounds::new(0.0, 0.0, width, height);
    let mut paint = Painter { gfx, screen, board };
    paint.screen_fill(screen, CANVAS);
    let spacing = (24.0 * board.zoom).max(10.0);
    let mut y = board.pan.y.rem_euclid(spacing);
    while y < height {
        let mut x = board.pan.x.rem_euclid(spacing);
        while x < width {
            paint.screen_fill(Bounds::new(x, y, 1.6, 1.6), GRID);
            x += spacing;
        }
        y += spacing;
    }

    let dock = board.dock_bounds();
    let shadow = Point::new(4.0, 5.0);
    if let Some(bounds) = dock {
        paint.fill(bounds.translated(shadow), GRID);
    }
    for panel in board.order() {
        let bounds = board.bounds(panel);
        if dock.is_none() {
            paint.fill(bounds.translated(shadow), GRID);
        }
        paint.fill(
            bounds,
            if dock.is_some() || board.front == panel {
                GREEN
            } else {
                BORDER
            },
        );
        paint.fill(
            Bounds::new(
                bounds.x + 1.0,
                bounds.y + 1.0,
                bounds.w - 2.0,
                bounds.h - 2.0,
            ),
            PAPER,
        );
        paint.fill(
            Bounds::new(
                bounds.x + 1.0,
                bounds.y + 1.0,
                bounds.w - 2.0,
                TITLE_HEIGHT - 1.0,
            ),
            HEADER,
        );
        match panel {
            Panel::Game => draw_game(&mut paint, chrome),
            Panel::Timeline => draw_timeline(&mut paint, chrome),
        }
        if dock.is_none() || panel == Panel::Timeline {
            draw_grip(&mut paint, bounds);
        }
    }
    if chrome.dock_target {
        paint.fill(
            Bounds::new(board.game.x, board.game.bottom() - 1.0, board.game.w, 4.0),
            GREEN,
        );
    }
}

fn draw_grip(paint: &mut Painter<'_>, bounds: Bounds) {
    paint.fill(bounds.grip(), HEADER);
    for i in 0..3 {
        let offset = f64::from(i) * 3.0;
        paint.fill(
            Bounds::new(
                bounds.right() - 6.0 - offset,
                bounds.bottom() - 5.0,
                2.0,
                2.0,
            ),
            BORDER,
        );
        paint.fill(
            Bounds::new(
                bounds.right() - 6.0,
                bounds.bottom() - 5.0 - offset,
                2.0,
                2.0,
            ),
            BORDER,
        );
    }
}

fn draw_game(paint: &mut Painter<'_>, chrome: Chrome) {
    let bounds = chrome.board.game;
    let body = chrome.board.game_body();
    let header = chrome.board.game_header();
    let phase = match chrome.phase {
        Phase::Building => Some(("BUILDING", MUTED)),
        Phase::Reconnecting => Some(("CONNECTING", MUTED)),
        Phase::Running => None,
        Phase::Failed => Some(("BUILD FAILED", RED)),
        Phase::GpuFailed => Some(("GPU UNAVAILABLE", RED)),
        Phase::ReloadFailed => Some(("RELOAD FAILED", RED)),
        Phase::Exited => Some(("GAME EXITED", RED)),
    };
    let title_color = phase.filter(|(_, color)| *color == RED).map_or(
        if chrome.preview_focus { GREEN } else { INK },
        |(_, color)| color,
    );
    paint.text(
        Point::new(bounds.x + 12.0, bounds.y + 10.0),
        "GAME",
        title_color,
    );
    if bounds.w >= 470.0
        && let Some((label, color)) = phase
    {
        paint.text(Point::new(bounds.x + 65.0, bounds.y + 10.0), label, color);
    }

    let track = header.zoom_track;
    paint.fill(track, BORDER);
    paint.fill(
        Bounds::new(track.x + track.w / 2.0 - 0.5, track.y - 2.0, 1.0, 7.0),
        MUTED,
    );
    let thumb = header.thumb_x(chrome.board.game_zoom);
    paint.fill(Bounds::new(thumb - 3.0, track.y - 3.0, 6.0, 9.0), GREEN);
    paint.fill(Bounds::new(track.x - 10.0, track.y + 1.0, 6.0, 1.0), MUTED);
    paint.fill(
        Bounds::new(track.right() + 5.0, track.y + 1.0, 6.0, 1.0),
        MUTED,
    );
    paint.fill(
        Bounds::new(track.right() + 7.5, track.y - 1.5, 1.0, 6.0),
        MUTED,
    );
    if let Some(size) = chrome.frame_size {
        paint.centered_text(
            header.resolution,
            &format!("{} X {}", size.width, size.height),
            MUTED,
        );
    }
    paint.fill(body, BLACK);
    if chrome.frame_size.is_none() {
        let message = phase.map_or("WAITING FOR GAME", |(label, _)| label);
        paint.text(Point::new(body.x + 20.0, body.y + 20.0), message, WHITE);
    }
}

fn draw_timeline(paint: &mut Painter<'_>, chrome: Chrome) {
    let panel = chrome.board.timeline;
    let layout = TimelineLayout::new(panel);
    let timeline = chrome.timeline;
    let cursor = chrome
        .drag_ms
        .or(timeline.cursor.map(|frame| frame.at_ms))
        .unwrap_or(0);
    let tip = timeline.tip.map_or(0, |frame| frame.at_ms);
    paint.text(Point::new(panel.x + 12.0, panel.y + 10.0), "TIMELINE", INK);
    paint.centered_text(
        Bounds::new(
            panel.right() - 161.0,
            panel.y + 2.0,
            153.0,
            TITLE_HEIGHT - 4.0,
        ),
        &format!("{} / {}", timestamp(cursor), timestamp(tip)),
        MUTED,
    );
    let track = layout.track;
    paint.fill(track, HEADER);
    let fraction = if tip == 0 {
        0.0
    } else {
        cursor.min(tip) as f64 / tip as f64
    };
    let progress = track.w * fraction;
    paint.fill(Bounds::new(track.x, track.y + 2.0, progress, 4.0), GREEN);
    for i in 0..=10 {
        let x = track.x + track.w * f64::from(i) / 10.0;
        paint.fill(Bounds::new(x, track.bottom(), 1.0, 5.0), BORDER);
    }
    paint.fill(
        Bounds::new(track.x + progress - 3.0, track.y - 4.0, 6.0, track.h + 8.0),
        GREEN,
    );
    for control in TRANSPORT_CONTROLS {
        let icon = match control {
            Transport::Back => Some([
                0b0001000, 0b0010000, 0b0100000, 0b1111110, 0b0100000, 0b0010000, 0b0001000,
            ]),
            Transport::Play if timeline.playing || !timeline.paused => Some([0b0110110; 7]),
            Transport::Play => Some([
                0b0100000, 0b0110000, 0b0111100, 0b0111110, 0b0111100, 0b0110000, 0b0100000,
            ]),
            Transport::Next => Some([
                0b0001000, 0b0000100, 0b0000010, 0b0111111, 0b0000010, 0b0000100, 0b0001000,
            ]),
            Transport::TakeOver => None,
        };
        let active = chrome.connected && dock_command(control, timeline).is_some();
        let button = layout.button(control);
        let primary = active && control == Transport::Play;
        let color = if primary {
            GREEN
        } else if active && (control == Transport::TakeOver || chrome.hovered == Some(control)) {
            GREEN_LIGHT
        } else {
            HEADER
        };
        let icon_color = if primary {
            WHITE
        } else if active {
            INK
        } else {
            BORDER
        };
        paint.fill(button, color);
        if let Some(icon) = icon {
            paint.icon(button, icon, icon_color);
        } else {
            paint.centered_text(
                button,
                "TAKE OVER",
                if active { GREEN } else { MUTED },
            );
        }
    }
}

fn timestamp(ms: u64) -> String {
    format!(
        "{}:{:02}.{}",
        ms / 60_000,
        (ms / 1_000) % 60,
        (ms / 100) % 10
    )
}

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var<uniform> position: vec4<f32>;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    return Vertex(vec4<f32>(position.xy + uv * position.zw, 0.0, 1.0), uv);
}

@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    return textureSample(image, image_sampler, input.uv);
}
"#;

struct PreviewTexture {
    texture: wgpu::Texture,
    bindings: wgpu::BindGroup,
}

struct GameImage {
    texture: Arc<PreviewTexture>,
    frame: Arc<PreviewFrame>,
}

struct GameBlit {
    pipeline: wgpu::RenderPipeline,
    position: wgpu::Buffer,
    sampler: wgpu::Sampler,
    image: Option<GameImage>,
    #[cfg(target_os = "macos")]
    cached_surfaces: VecDeque<(Arc<Surface>, Arc<PreviewTexture>)>,
    #[cfg(target_os = "macos")]
    rejected_surface: Option<u32>,
}

impl GameBlit {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("editor game image shader"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("editor game image"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vertex",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fragment",
                targets: &[Some(format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let position = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("editor game image position"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Nearest,
            mag_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        Self {
            pipeline,
            position,
            sampler,
            image: None,
            #[cfg(target_os = "macos")]
            cached_surfaces: VecDeque::new(),
            #[cfg(target_os = "macos")]
            rejected_surface: None,
        }
    }

    fn upload(&mut self, context: &pixels::PixelsContext, frame: &Arc<PreviewFrame>) {
        if self
            .image
            .as_ref()
            .is_some_and(|image| Arc::ptr_eq(&image.frame, frame))
        {
            return;
        }
        #[cfg(target_os = "macos")]
        if let Some(remote) = &frame.surface {
            let id = remote.surface.id();
            let cached = self
                .cached_surfaces
                .iter()
                .position(|(surface, _)| surface.id() == id);
            let texture = if let Some(index) = cached {
                self.cached_surfaces.remove(index).unwrap().1
            } else {
                if self.rejected_surface == Some(id) {
                    return;
                }
                let texture = match remote.surface.sampled_texture(&context.device, frame.size) {
                    Ok(texture) => texture,
                    Err(err) => {
                        eprintln!("editor could not import the shared preview, using RGBA: {err}");
                        self.rejected_surface = Some(id);
                        remote.use_rgba();
                        return;
                    }
                };
                Arc::new(self.bind_texture(&context.device, texture))
            };
            self.cached_surfaces
                .push_back((remote.surface.clone(), texture.clone()));
            if self.cached_surfaces.len() > 3 {
                self.cached_surfaces.pop_front();
            }
            self.image = Some(GameImage {
                texture,
                frame: frame.clone(),
            });
            return;
        }
        #[cfg(target_os = "macos")]
        self.cached_surfaces.clear();

        let size = wgpu::Extent3d {
            width: frame.size.width,
            height: frame.size.height,
            depth_or_array_layers: 1,
        };
        let existing = self.image.as_ref().filter(|image| {
            if image.frame.size != frame.size {
                return false;
            }
            #[cfg(target_os = "macos")]
            if image.frame.surface.is_some() {
                return false;
            }
            true
        });
        let texture = if let Some(image) = existing {
            image.texture.clone()
        } else {
            let texture = context.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("game preview RGBA texture"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            Arc::new(self.bind_texture(&context.device, texture))
        };
        context.queue.write_texture(
            texture.texture.as_image_copy(),
            &frame.pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(frame.size.width * 4),
                rows_per_image: None,
            },
            size,
        );
        self.image = Some(GameImage {
            texture,
            frame: frame.clone(),
        });
    }

    fn bind_texture(&self, device: &wgpu::Device, texture: wgpu::Texture) -> PreviewTexture {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("game preview image bindings"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.position.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        PreviewTexture { texture, bindings }
    }

    fn place(&self, queue: &wgpu::Queue, rect: Bounds, logical: SurfaceSize) {
        let sx = 2.0 / f64::from(logical.width);
        let sy = 2.0 / f64::from(logical.height);
        let position = [
            (rect.x * sx - 1.0) as f32,
            (1.0 - rect.y * sy) as f32,
            (rect.w * sx) as f32,
            (-rect.h * sy) as f32,
        ];
        queue.write_buffer(&self.position, 0, bytemuck::cast_slice(&position));
    }

    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        visible: &[Bounds],
        logical: SurfaceSize,
        surface: SurfaceSize,
    ) {
        let Some(image) = &self.image else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("editor game preview"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: true,
                },
            })],
            depth_stencil_attachment: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &image.texture.bindings, &[]);
        let sx = f64::from(surface.width) / f64::from(logical.width);
        let sy = f64::from(surface.height) / f64::from(logical.height);
        for rect in visible {
            let x = (rect.x * sx).round() as u32;
            let y = (rect.y * sy).round() as u32;
            let w = ((rect.right() * sx).round() as u32)
                .min(surface.width)
                .saturating_sub(x);
            let h = ((rect.bottom() * sy).round() as u32)
                .min(surface.height)
                .saturating_sub(y);
            if w > 0 && h > 0 {
                pass.set_scissor_rect(x, y, w, h);
                pass.draw(0..4, 0..1);
            }
        }
    }
}
