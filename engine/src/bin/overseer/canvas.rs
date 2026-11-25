#[cfg(target_os = "macos")]
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender},
};

use engine::embedded::{PreviewFrame, TimelineStatus};
use engine::gpu_timer::{GpuTimer, TimestampReadback};
use engine::graphics::{Color, GpuRenderer2d, Renderer2d};
#[cfg(target_os = "macos")]
use engine::shared_surface::Surface;
use engine::surface::SurfaceSize;
use engine::ui::Rect;
use pixels::{Pixels, PixelsContext, wgpu};

use super::board::{
    Board, Bounds, Panel, PanelMenu, Point, TITLE_HEIGHT, TRANSPORT_CONTROLS, TimelineLayout,
    Transport, viewport,
};
use super::profiler::{self, FrameKey, History};
use super::text::{Style, TextRenderer};
use super::{Phase, dock_command};

const CANVAS: Color = [235, 229, 216, 255];
const GRID: Color = [210, 201, 183, 255];
const PAPER: Color = [253, 249, 238, 255];
pub(super) const HEADER: Color = [242, 235, 220, 255];
pub(super) const INK: Color = [18, 24, 20, 255];
pub(super) const MUTED: Color = [49, 57, 45, 255];
pub(super) const BORDER: Color = [179, 172, 154, 255];
pub(super) const GREEN: Color = [18, 49, 33, 255];
pub(super) const GREEN_LIGHT: Color = [224, 234, 213, 255];
pub(super) const RED: Color = [110, 35, 25, 255];
const BLACK: Color = [14, 19, 19, 255];
const WHITE: Color = [251, 246, 230, 255];

#[derive(Clone, Copy)]
struct PixelGrid {
    logical: SurfaceSize,
    surface: SurfaceSize,
    scale: Point,
}

impl PixelGrid {
    fn new(logical: SurfaceSize, surface: SurfaceSize) -> Self {
        Self {
            logical,
            surface,
            scale: Point::new(
                f64::from(surface.width) / f64::from(logical.width),
                f64::from(surface.height) / f64::from(logical.height),
            ),
        }
    }

    // Snap edges independently so adjacent fills share the same physical edge.
    fn snap(self, bounds: Bounds) -> Bounds {
        let x = (bounds.x * self.scale.x).round();
        let y = (bounds.y * self.scale.y).round();
        Bounds::new(
            x,
            y,
            (bounds.right() * self.scale.x).round() - x,
            (bounds.bottom() * self.scale.y).round() - y,
        )
    }

    fn horizontal_rule(self, bounds: Bounds) -> Bounds {
        let mut pixels = self.snap(bounds);
        pixels.w = pixels.w.max(1.0);
        pixels.h = stroke_pixels(bounds.h * self.scale.y);
        pixels
    }

    fn vertical_rule(self, bounds: Bounds) -> Bounds {
        let mut pixels = self.snap(bounds);
        pixels.w = stroke_pixels(bounds.w * self.scale.x);
        pixels.h = pixels.h.max(1.0);
        pixels
    }

    fn logical_bounds(self, physical: Bounds) -> Bounds {
        Bounds::new(
            physical.x / self.scale.x,
            physical.y / self.scale.y,
            physical.w / self.scale.x,
            physical.h / self.scale.y,
        )
    }

    fn panel(self, board: Board, panel: Panel) -> PanelPixels {
        let bounds = board.bounds(panel);
        let outer = self.snap(board.project(bounds));
        let border_x = stroke_pixels(board.zoom * self.scale.x);
        let border_y = stroke_pixels(board.zoom * self.scale.y);
        let interior = Bounds::new(
            outer.x + border_x,
            outer.y + border_y,
            (outer.w - border_x * 2.0).max(0.0),
            (outer.h - border_y * 2.0).max(0.0),
        );
        let body_y = (board
            .screen_at(Point::new(bounds.x, bounds.y + TITLE_HEIGHT))
            .y
            * self.scale.y)
            .round()
            .clamp(interior.y, interior.bottom());
        PanelPixels {
            outer,
            interior,
            header: Bounds::new(interior.x, interior.y, interior.w, body_y - interior.y),
            body: Bounds::new(interior.x, body_y, interior.w, interior.bottom() - body_y),
        }
    }

    fn visible_regions(
        self,
        chrome: Chrome,
        panel: Panel,
        area: Bounds,
        extra_cover: Option<Bounds>,
    ) -> Vec<Bounds> {
        if !chrome.board.visible(panel) {
            return Vec::new();
        }
        let Some(region) = area.intersect(viewport(self.surface)) else {
            return Vec::new();
        };
        let covers = chrome
            .board
            .occluders(panel, self.logical, chrome.menu_open)
            .chain(extra_cover);
        covers
            .map(|cover| self.snap(cover))
            .fold(vec![region], |regions, cover| {
                if regions
                    .iter()
                    .any(|region| region.intersect(cover).is_some())
                {
                    regions
                        .into_iter()
                        .flat_map(|region| region.excluding(cover))
                        .collect()
                } else {
                    regions
                }
            })
    }
}

fn stroke_pixels(thickness: f64) -> f64 {
    thickness.round().max(1.0)
}

struct PanelPixels {
    outer: Bounds,
    interior: Bounds,
    header: Bounds,
    body: Bounds,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Chrome {
    pub board: Board,
    pub size: SurfaceSize,
    pub phase: Phase,
    pub connected: bool,
    pub frame_size: Option<SurfaceSize>,
    pub master_volume: Option<u8>,
    pub timeline: TimelineStatus,
    pub preview_focus: bool,
    pub hovered: Option<Transport>,
    pub drag_ms: Option<u64>,
    pub dock_target: bool,
    pub menu_open: bool,
}

pub(super) struct Submission {
    pub composited: bool,
    timestamps: Option<(FrameKey, TimestampReadback)>,
}

pub struct Canvas {
    ui: GpuRenderer2d,
    text: TextRenderer,
    preview: GameBlit,
    drawn: Option<(Chrome, SurfaceSize, u64)>,
    pending_gpu: Arc<AtomicUsize>,
    gpu_timer: Option<GpuTimer>,
    gpu_results: Receiver<(FrameKey, Option<u64>)>,
    gpu_sender: Sender<(FrameKey, Option<u64>)>,
}

impl Canvas {
    pub fn new(pixels: &Pixels) -> Self {
        let format = pixels.surface_texture_format();
        let context = pixels.context();
        let device = &context.device;
        let (gpu_sender, gpu_results) = mpsc::channel();
        Self {
            ui: GpuRenderer2d::new(device, format),
            text: TextRenderer::new(device, format),
            preview: GameBlit::new(device, format),
            drawn: None,
            pending_gpu: Arc::new(AtomicUsize::new(0)),
            gpu_timer: GpuTimer::new(device, &context.queue),
            gpu_results,
            gpu_sender,
        }
    }

    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        context: &PixelsContext,
        chrome: Chrome,
        surface: SurfaceSize,
        frame: Option<&Arc<PreviewFrame>>,
        history: &History,
        profile_key: Option<FrameKey>,
    ) -> Submission {
        let grid = PixelGrid::new(chrome.size, surface);
        let appearance = (chrome, surface, history.revision);
        if self.drawn != Some(appearance) {
            self.ui.begin_frame(surface);
            self.text.begin_frame(chrome.size, surface);
            draw_board(&mut self.ui, &mut self.text, chrome, grid, history);
            self.drawn = Some(appearance);
        }
        let area = grid.panel(chrome.board, Panel::Game).body;
        let grip = chrome
            .board
            .game_grip()
            .map(|grip| chrome.board.project(grip));
        let visible = grid.visible_regions(chrome, Panel::Game, area, grip);
        let has_image = !visible.is_empty() && frame.is_some();
        if let Some(frame) = frame.filter(|_| has_image) {
            self.preview.upload(context, frame);
            self.preview.place(&context.queue, area, surface);
        }
        let timestamps = profile_key.and_then(|key| {
            self.gpu_timer
                .as_ref()?
                .begin(encoder)
                .map(|readback| (key, readback))
        });
        self.ui
            .render(encoder, target, &context.device, &context.queue);
        self.text
            .render(encoder, target, &context.device, &context.queue);
        if has_image {
            self.preview.draw(encoder, target, &visible);
        }
        if let Some((timer, (_, readback))) = self.gpu_timer.as_ref().zip(timestamps.as_ref()) {
            timer.end(encoder, readback);
        }
        Submission {
            composited: has_image,
            timestamps,
        }
    }

    pub fn after_submit(&self, _context: &PixelsContext, submission: Submission) {
        if let Some((key, timestamps)) = submission.timestamps {
            let pending = self.pending_gpu.clone();
            let sender = self.gpu_sender.clone();
            pending.fetch_add(1, Ordering::Relaxed);
            timestamps.map(move |elapsed| {
                let _ = sender.send((key, elapsed));
                pending.fetch_sub(1, Ordering::Relaxed);
            });
        }
        #[cfg(target_os = "macos")]
        if submission.composited
            && let Some(image) = &self.preview.image
            && image.frame.surface.is_some()
        {
            let frame = image.frame.clone();
            let pending = self.pending_gpu.clone();
            pending.fetch_add(1, Ordering::Relaxed);
            // Retain the IOSurface until the editor has finished sampling it.
            _context.queue.on_submitted_work_done(move || {
                drop(frame);
                pending.fetch_sub(1, Ordering::Relaxed);
            });
        }
    }

    pub fn take_gpu_profiles(&self) -> impl Iterator<Item = (FrameKey, Option<u64>)> + '_ {
        self.gpu_results.try_iter()
    }

    pub fn poll_gpu(&self, pixels: &Pixels) -> bool {
        if self.pending_gpu.load(Ordering::Relaxed) != 0 {
            pixels.context().device.poll(wgpu::Maintain::Poll);
        }
        self.pending_gpu.load(Ordering::Relaxed) != 0
    }
}

pub(super) struct Painter<'a> {
    gfx: &'a mut dyn Renderer2d,
    text: &'a mut TextRenderer,
    text_clips: Vec<Bounds>,
    grid: PixelGrid,
    board: Board,
}

impl Painter<'_> {
    fn pixel_fill(&mut self, bounds: Bounds, color: Color) {
        if let Some(rect) = bounds.intersect(viewport(self.grid.surface)) {
            self.gfx.fill_rect(
                Rect::new(rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32),
                color,
            );
        }
    }

    fn screen_fill(&mut self, bounds: Bounds, color: Color) {
        self.pixel_fill(self.grid.snap(bounds), color);
    }

    pub(super) fn fill(&mut self, bounds: Bounds, color: Color) {
        self.screen_fill(self.board.project(bounds), color);
    }

    pub(super) fn horizontal_rule(&mut self, bounds: Bounds, color: Color) {
        self.pixel_fill(self.grid.horizontal_rule(self.board.project(bounds)), color);
    }

    pub(super) fn vertical_rule(&mut self, bounds: Bounds, color: Color) {
        self.pixel_fill(self.grid.vertical_rule(self.board.project(bounds)), color);
    }

    fn screen_text(&mut self, point: Point, text: &str, color: Color, style: Style) {
        self.text
            .draw(point, text, style, style.size(), color, &self.text_clips);
    }

    pub(super) fn text(&mut self, point: Point, text: &str, color: Color, style: Style) {
        if self.board.zoom >= 0.36 {
            let size = (style.size() * self.board.zoom).max(10.0);
            self.text.draw(
                self.board.screen_at(point),
                text,
                style,
                size,
                color,
                &self.text_clips,
            );
        }
    }

    pub(super) fn centered_text(
        &mut self,
        bounds: Bounds,
        label: &str,
        color: Color,
        style: Style,
    ) {
        if self.board.zoom < 0.36 || label.is_empty() {
            return;
        }
        let bounds = self.board.project(bounds);
        let mut size = (style.size() * self.board.zoom).max(10.0);
        let metrics = self.text.measure(label, style, size);
        size *= ((bounds.w - 6.0) / metrics.x)
            .min((bounds.h - 2.0) / metrics.y)
            .min(1.0);
        if size >= 9.0 {
            let metrics = self.text.measure(label, style, size);
            let point = Point::new(
                bounds.x + (bounds.w - metrics.x) / 2.0,
                bounds.y + (bounds.h - metrics.y) / 2.0,
            );
            self.text
                .draw(point, label, style, size, color, &self.text_clips);
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

fn draw_board(
    gfx: &mut dyn Renderer2d,
    text: &mut TextRenderer,
    chrome: Chrome,
    grid: PixelGrid,
    history: &History,
) {
    let board = chrome.board;
    let width = f64::from(chrome.size.width);
    let height = f64::from(chrome.size.height);
    let screen = viewport(chrome.size);
    let mut paint = Painter {
        gfx,
        text,
        text_clips: Vec::new(),
        grid,
        board,
    };
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
    let shadow = super::board::PANEL_SHADOW;
    if let Some(bounds) = dock {
        paint.fill(bounds.translated(shadow), GRID);
    }
    for panel in board.order() {
        let bounds = board.bounds(panel);
        let docked = dock.is_some() && panel != Panel::Profiler;
        if let Some(shadow) = board.panel_shadow(panel) {
            paint.fill(shadow, GRID);
        }
        let pixels = grid.panel(board, panel);
        paint.pixel_fill(
            pixels.outer,
            if docked || board.front() == Some(panel) {
                GREEN
            } else {
                BORDER
            },
        );
        paint.pixel_fill(pixels.interior, PAPER);
        paint.pixel_fill(pixels.header, HEADER);
        paint.text_clips = grid
            .visible_regions(chrome, panel, pixels.outer, None)
            .into_iter()
            .map(|bounds| grid.logical_bounds(bounds))
            .collect();
        match panel {
            Panel::Game => draw_game(&mut paint, chrome, pixels.body),
            Panel::Timeline => draw_timeline(&mut paint, chrome),
            Panel::Profiler => profiler::draw(&mut paint, bounds, history),
        }
        if !docked || panel == Panel::Timeline {
            draw_grip(&mut paint, bounds);
        }
    }
    if chrome.dock_target {
        paint.horizontal_rule(
            Bounds::new(board.game.x, board.game.bottom() - 1.0, board.game.w, 4.0),
            GREEN,
        );
    }
    paint.text_clips = vec![screen];
    draw_panel_menu(&mut paint, chrome);
}

fn draw_panel_menu(paint: &mut Painter<'_>, chrome: Chrome) {
    let menu = PanelMenu::new(chrome.size);
    let button = menu.button;
    paint.screen_fill(button, GREEN);
    paint.screen_fill(
        Bounds::new(button.x + 10.0, button.y + 17.0, 16.0, 2.0),
        WHITE,
    );
    paint.screen_fill(
        Bounds::new(button.x + 17.0, button.y + 10.0, 2.0, 16.0),
        WHITE,
    );
    if !chrome.menu_open {
        return;
    }
    paint.screen_fill(menu.popup, BORDER);
    paint.screen_fill(
        Bounds::new(
            menu.popup.x + 1.0,
            menu.popup.y + 1.0,
            menu.popup.w - 2.0,
            menu.popup.h - 2.0,
        ),
        PAPER,
    );
    for (index, panel) in Panel::ALL.into_iter().enumerate() {
        let row = menu.row(index);
        if chrome.board.visible(panel) {
            paint.screen_fill(row, GREEN_LIGHT);
        }
        paint.screen_text(
            Point::new(row.x + 9.0, row.y + 6.0),
            panel.title(),
            INK,
            Style::Label,
        );
        let center = Point::new(row.right() - 16.0, row.y + 15.0);
        paint.screen_fill(Bounds::new(center.x - 4.0, center.y, 9.0, 1.0), GREEN);
        if !chrome.board.visible(panel) {
            paint.screen_fill(Bounds::new(center.x, center.y - 4.0, 1.0, 9.0), GREEN);
        }
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

fn draw_game(paint: &mut Painter<'_>, chrome: Chrome, body_pixels: Bounds) {
    let bounds = chrome.board.game;
    let body = chrome.board.game_body();
    let header = chrome.board.game_header();
    let phase = match chrome.phase {
        Phase::Building => Some(("Building", MUTED)),
        Phase::Reconnecting => Some(("Connecting", MUTED)),
        Phase::Running => None,
        Phase::Failed => Some(("Build failed", RED)),
        Phase::GpuFailed => Some(("GPU unavailable", RED)),
        Phase::ReloadFailed => Some(("Reload failed", RED)),
        Phase::Exited => Some(("Game exited", RED)),
    };
    let title_color = phase.filter(|(_, color)| *color == RED).map_or(
        if chrome.preview_focus { GREEN } else { INK },
        |(_, color)| color,
    );
    paint.text(
        Point::new(bounds.x + 12.0, bounds.y + 6.0),
        "Game",
        title_color,
        Style::Title,
    );
    if bounds.w >= 470.0
        && let Some((label, color)) = phase
    {
        paint.text(
            Point::new(bounds.x + 65.0, bounds.y + 7.0),
            label,
            color,
            Style::Caption,
        );
    }

    if let Some(percent) = chrome.master_volume {
        let track = header.volume_track;
        let thumb = header.volume_thumb_x(percent);
        let speaker = [
            0b0001000, 0b0011000, 0b1111010, 0b1111001, 0b1111010, 0b0011000, 0b0001000,
        ];
        paint.icon(
            Bounds::new(track.x - 19.0, track.y - 5.5, 14.0, 14.0),
            speaker,
            MUTED,
        );
        paint.horizontal_rule(track, BORDER);
        if percent > 0 {
            paint.horizontal_rule(
                Bounds::new(track.x, track.y, thumb - track.x, track.h),
                GREEN,
            );
        }
        paint.fill(Bounds::new(thumb - 3.0, track.y - 3.0, 6.0, 9.0), GREEN);
    }
    let track = header.zoom_track;
    paint.horizontal_rule(track, BORDER);
    paint.vertical_rule(
        Bounds::new(track.x + track.w / 2.0 - 0.5, track.y - 2.0, 1.0, 7.0),
        MUTED,
    );
    let thumb = header.thumb_x(chrome.board.game_zoom);
    paint.fill(Bounds::new(thumb - 3.0, track.y - 3.0, 6.0, 9.0), GREEN);
    paint.horizontal_rule(Bounds::new(track.x - 10.0, track.y + 1.0, 6.0, 1.0), MUTED);
    paint.horizontal_rule(
        Bounds::new(track.right() + 5.0, track.y + 1.0, 6.0, 1.0),
        MUTED,
    );
    paint.vertical_rule(
        Bounds::new(track.right() + 7.5, track.y - 1.5, 1.0, 6.0),
        MUTED,
    );
    if let Some(size) = chrome.frame_size {
        paint.centered_text(
            header.resolution,
            &format!("{} × {}", size.width, size.height),
            MUTED,
            Style::Number,
        );
    }
    paint.pixel_fill(body_pixels, BLACK);
    if chrome.frame_size.is_none() {
        let message = phase.map_or("Waiting for game", |(label, _)| label);
        paint.text(
            Point::new(body.x + 20.0, body.y + 20.0),
            message,
            WHITE,
            Style::Label,
        );
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
    paint.text(
        Point::new(panel.x + 12.0, panel.y + 6.0),
        "Timeline",
        INK,
        Style::Title,
    );
    paint.centered_text(
        Bounds::new(
            panel.right() - 161.0,
            panel.y + 2.0,
            153.0,
            TITLE_HEIGHT - 4.0,
        ),
        &format!("{} / {}", timestamp(cursor), timestamp(tip)),
        MUTED,
        Style::Number,
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
        paint.vertical_rule(Bounds::new(x, track.bottom(), 1.0, 5.0), BORDER);
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
                "Take over",
                if active { GREEN } else { MUTED },
                Style::Label,
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

    fn place(&self, queue: &wgpu::Queue, rect: Bounds, surface: SurfaceSize) {
        let sx = 2.0 / f64::from(surface.width);
        let sy = 2.0 / f64::from(surface.height);
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
        for rect in visible {
            pass.set_scissor_rect(rect.x as u32, rect.y as u32, rect.w as u32, rect.h as u32);
            pass.draw(0..4, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_keep_their_physical_thickness_during_fractional_panning() {
        let logical = SurfaceSize::new(800, 600);
        for surface in [
            logical,
            SurfaceSize::new(1201, 901),
            SurfaceSize::new(1600, 1200),
        ] {
            let grid = PixelGrid::new(logical, surface);
            for pan in [0.1, 0.35, 0.6, 0.85] {
                let mut board = Board {
                    pan: Point::new(pan, pan),
                    zoom: 0.4,
                    ..Board::default()
                };
                let horizontal = Bounds::new(40.0, 40.0, 20.0, 1.0);
                let vertical = Bounds::new(40.0, 40.0, 1.0, 20.0);
                assert_eq!(grid.horizontal_rule(board.project(horizontal)).h, 1.0);
                assert_eq!(grid.vertical_rule(board.project(vertical)).w, 1.0);
                board.zoom = 2.0;
                assert_eq!(
                    grid.horizontal_rule(board.project(horizontal)).h,
                    (2.0 * grid.scale.y).round()
                );
                assert_eq!(
                    grid.vertical_rule(board.project(vertical)).w,
                    (2.0 * grid.scale.x).round()
                );
            }
        }
    }

    #[test]
    fn preview_clip_meets_the_snapped_panel_edges_and_covers() {
        let logical = SurfaceSize::new(800, 600);
        let grid = PixelGrid::new(logical, SurfaceSize::new(1201, 901));
        let mut board = Board {
            game: Bounds::new(40.5, 36.0, 960.0, 640.0),
            profiler: Bounds::new(520.5, 76.0, 530.0, 400.0),
            pan: Point::new(0.6, 0.3),
            zoom: 0.44,
            show_profiler: true,
            ..Board::default()
        };
        board.raise(Panel::Profiler);
        let chrome = Chrome {
            board,
            size: logical,
            phase: Phase::Running,
            connected: true,
            frame_size: None,
            master_volume: None,
            timeline: TimelineStatus::default(),
            preview_focus: false,
            hovered: None,
            drag_ms: None,
            dock_target: false,
            menu_open: false,
        };
        let panel = grid.panel(board, Panel::Game);
        let cover = grid.panel(board, Panel::Profiler).outer;
        let grip = board.project(board.game_grip().unwrap());
        let visible = grid.visible_regions(chrome, Panel::Game, panel.body, Some(grip));
        let draws_at = |x, y| visible.iter().any(|rect| rect.contains(Point::new(x, y)));
        assert_eq!(panel.body.y, panel.header.bottom());
        assert_eq!(panel.body.x, panel.outer.x + 1.0);
        assert_eq!(panel.body.bottom() + 1.0, panel.outer.bottom());
        assert!(draws_at(panel.body.x, panel.body.y));
        assert!(!draws_at(panel.body.x - 1.0, panel.body.y));
        assert!(!draws_at(panel.body.x, panel.body.y - 1.0));
        assert!(draws_at(cover.x - 1.0, cover.y));
        assert!(!draws_at(cover.x, cover.y));
        let grip = grid.snap(grip);
        assert!(draws_at(grip.x - 1.0, grip.y));
        assert!(!draws_at(grip.x, grip.y));
    }
}
