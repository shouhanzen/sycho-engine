use std::collections::BTreeMap;
#[cfg(target_os = "macos")]
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::{self, Receiver, Sender},
};

use engine::embedded::{PreviewFrame, TimelineStatus};
use engine::gpu_timer::{GpuTimer, TimestampReadback};
use engine::graphics::{Color, GpuRenderer2d, Image, ImageRect, Renderer2d};
#[cfg(target_os = "macos")]
use engine::shared_surface::Surface;
use engine::surface::SurfaceSize;
use engine::tilesets::Tilesets;
use engine::ui::Rect;
use pixels::{Pixels, PixelsContext, wgpu};

use super::board::{
    Board, Bounds, GameHeader, Panel, PanelMenu, PanelWindow, Point, TITLE_HEIGHT,
    TRANSPORT_CONTROLS, TimelineLayout, TimelineResetLayout, Transport, WindowId,
    fullscreen_exit_button, viewport,
};
use super::explorer::{
    self, EntryKind, Explorer, Field, Layout as ExplorerLayout, Mode as ExplorerMode,
};
use super::profiler::{self, FrameKey, Histories, History};
use super::render::Preview;
use super::text::{Style, TextRenderer};
use super::{Phase, dock_command};
use super::{art, bookmarks, level, target};
use engine::tiles::Cell;

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

    fn panel(self, board: &Board, panel: &PanelWindow) -> PanelPixels {
        let bounds = panel.chrome_bounds();
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
        chrome: &Chrome,
        panel: WindowId,
        area: Bounds,
        extra_cover: Option<Bounds>,
    ) -> Vec<Bounds> {
        let Some(region) = area.intersect(viewport(self.surface)) else {
            return Vec::new();
        };
        let covers = chrome
            .board
            .windows()
            .skip_while(|window| window.id != panel)
            .skip(1)
            .map(|window| chrome.board.project(window.chrome_bounds()))
            .chain(PanelMenu::new(self.logical).covers(chrome.menu_open))
            .chain(chrome.target_menu.as_ref().map(|menu| menu.bounds))
            .chain(chrome.header_menu.map(|menu| menu.bounds))
            .chain(
                chrome
                    .bookmark_popup
                    .as_ref()
                    .map(|(layout, _)| layout.bounds),
            )
            .chain(chrome.reset_prompt.map(|layout| layout.bounds))
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

#[derive(Clone, PartialEq)]
pub(super) struct GameChrome {
    pub phase: Phase,
    pub connected: bool,
    pub frame_size: Option<SurfaceSize>,
    pub master_volume: Option<u8>,
    pub timeline: TimelineStatus,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct FullscreenChrome {
    pub game: WindowId,
    pub show_exit: bool,
}

#[derive(Clone, PartialEq)]
pub(super) struct Chrome {
    pub board: Board,
    pub size: SurfaceSize,
    pub games: BTreeMap<WindowId, GameChrome>,
    pub focus: Option<WindowId>,
    pub hovered: Option<(WindowId, Transport)>,
    pub drag_ms: Option<(WindowId, u64)>,
    pub dock_target: Option<WindowId>,
    pub menu_open: bool,
    pub target_menu: Option<target::Menu>,
    pub header_menu: Option<target::HeaderMenu>,
    pub notice: Option<String>,
    pub bookmark_popup: Option<(bookmarks::Layout, bookmarks::Popup)>,
    pub reset_prompt: Option<TimelineResetLayout>,
    pub levels: BTreeMap<WindowId, level::View>,
    pub picked_tile: Option<art::Pick>,
    pub level_tilesets: BTreeMap<WindowId, Arc<Tilesets>>,
    pub assets: BTreeMap<WindowId, art::AssetView>,
    pub explorers: BTreeMap<WindowId, Explorer>,
    pub active_explorer: Option<WindowId>,
    pub fullscreen: Option<FullscreenChrome>,
}

pub(super) struct Submission {
    pub presented: BTreeMap<WindowId, u64>,
    #[cfg(target_os = "macos")]
    shared_previews: Vec<Arc<PreviewFrame>>,
    timestamps: Option<(Vec<FrameKey>, TimestampReadback)>,
}

pub struct Canvas {
    ui: GpuRenderer2d,
    feedback: GpuRenderer2d,
    text: TextRenderer,
    previews: BTreeMap<WindowId, GameBlit>,
    format: wgpu::TextureFormat,
    drawn: Option<(Chrome, SurfaceSize, Vec<(WindowId, u64)>)>,
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
            feedback: GpuRenderer2d::new(device, format),
            text: TextRenderer::new(device, format),
            previews: BTreeMap::new(),
            format,
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
        chrome: &Chrome,
        surface: SurfaceSize,
        drag: Option<target::Drag>,
        frames: &BTreeMap<WindowId, Preview>,
        histories: &Histories,
        profile_keys: Vec<FrameKey>,
    ) -> Submission {
        let grid = PixelGrid::new(chrome.size, surface);
        let revisions: Vec<_> = histories
            .iter()
            .map(|(&id, (_, history))| (id, history.revision))
            .collect();
        if self
            .drawn
            .as_ref()
            .is_none_or(|(previous, size, revision)| {
                previous != chrome || *size != surface || *revision != revisions
            })
        {
            self.ui.begin_frame(surface);
            self.text.begin_frame(chrome.size, surface);
            draw_board(&mut self.ui, &mut self.text, chrome, grid, histories);
            self.drawn = Some((chrome.clone(), surface, revisions));
        }
        self.previews.retain(|id, _| frames.contains_key(id));
        let mut visible_previews = Vec::new();
        for (&id, frame) in frames {
            let (area, visible) = if let Some(fullscreen) = chrome.fullscreen {
                if fullscreen.game != id {
                    continue;
                }
                let area = viewport(surface);
                let visible = if fullscreen.show_exit {
                    area.excluding(grid.snap(fullscreen_exit_button(chrome.size)))
                } else {
                    vec![area]
                };
                (area, visible)
            } else {
                let Some(window) = chrome.board.window(id) else {
                    continue;
                };
                let area = grid.panel(&chrome.board, window).body;
                let grip = chrome
                    .board
                    .dock_group(id)
                    .is_none()
                    .then(|| chrome.board.project(window.bounds.grip()));
                (area, grid.visible_regions(chrome, id, area, grip))
            };
            if visible.is_empty() {
                if let Some(preview) = self.previews.get_mut(&id) {
                    preview.clear();
                }
                continue;
            }
            let preview = self
                .previews
                .entry(id)
                .or_insert_with(|| GameBlit::new(&context.device, self.format));
            preview.upload(context, &frame.frame);
            preview.place(&context.queue, area, surface, frame.blur);
            visible_previews.push((id, visible));
        }
        let timestamps = (!profile_keys.is_empty())
            .then(|| {
                self.gpu_timer
                    .as_ref()?
                    .begin(encoder)
                    .map(|readback| (profile_keys, readback))
            })
            .flatten();
        self.ui
            .render(encoder, target, &context.device, &context.queue);
        self.text
            .render(encoder, target, &context.device, &context.queue);
        let mut presented = BTreeMap::new();
        #[cfg(target_os = "macos")]
        let mut shared_previews = Vec::new();
        for (id, visible) in visible_previews {
            let preview = &self.previews[&id];
            preview.draw(encoder, target, &visible);
            if let Some(stamp) = preview.image.as_ref().and_then(|image| image.frame.stamp) {
                presented.insert(id, stamp);
            }
            #[cfg(target_os = "macos")]
            if let Some(image) = &preview.image
                && image.frame.surface.is_some()
            {
                shared_previews.push(image.frame.clone());
            }
        }
        if let Some(drag) = drag {
            self.feedback.begin_frame(surface);
            draw_target_drag(&mut self.feedback, chrome, grid, drag);
            self.feedback
                .render_overlay(encoder, target, &context.device, &context.queue);
        }
        if let Some((timer, (_, readback))) = self.gpu_timer.as_ref().zip(timestamps.as_ref()) {
            timer.end(encoder, readback);
        }
        Submission {
            presented,
            #[cfg(target_os = "macos")]
            shared_previews,
            timestamps,
        }
    }

    pub fn after_submit(&self, _context: &PixelsContext, submission: Submission) {
        if let Some((keys, timestamps)) = submission.timestamps {
            let pending = self.pending_gpu.clone();
            let sender = self.gpu_sender.clone();
            pending.fetch_add(1, Ordering::Relaxed);
            timestamps.map(move |elapsed| {
                for key in keys {
                    let _ = sender.send((key, elapsed));
                }
                pending.fetch_sub(1, Ordering::Relaxed);
            });
        }
        #[cfg(target_os = "macos")]
        if !submission.shared_previews.is_empty() {
            let frames = submission.shared_previews;
            let pending = self.pending_gpu.clone();
            pending.fetch_add(1, Ordering::Relaxed);
            // Retain the IOSurface until the editor has finished sampling it.
            _context.queue.on_submitted_work_done(move || {
                drop(frames);
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
    board: &'a Board,
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

    fn image(&mut self, image: &Arc<Image>, source: Rect, dest: Bounds, clip: Bounds) {
        let dest = self.grid.snap(self.board.project(dest));
        let clip = self.grid.snap(self.board.project(clip));
        if let Some(clip) = clip.intersect(viewport(self.grid.surface)) {
            self.gfx.draw_image(
                image,
                source,
                ImageRect::new(dest.x as i32, dest.y as i32, dest.w as u32, dest.h as u32),
                pixel_rect(clip),
            );
        }
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
        self.board_text(point, text, color, style, style.size());
    }

    pub(super) fn text_width(&self, text: &str, style: Style) -> f64 {
        self.text.measure(text, style, style.size()).x
    }

    fn board_text(&mut self, point: Point, label: &str, color: Color, style: Style, size: f64) {
        self.text.draw(
            self.board.screen_at(point),
            label,
            style,
            size * self.board.zoom,
            color,
            &self.text_clips,
        );
    }

    pub(super) fn centered_text(
        &mut self,
        bounds: Bounds,
        label: &str,
        color: Color,
        style: Style,
    ) {
        if bounds.w <= 6.0 || bounds.h <= 2.0 || label.is_empty() {
            return;
        }
        let metrics = self.text.measure(label, style, style.size());
        let fit = ((bounds.w - 6.0) / metrics.x)
            .min((bounds.h - 2.0) / metrics.y)
            .min(1.0);
        let point = Point::new(
            bounds.x + (bounds.w - metrics.x * fit) / 2.0,
            bounds.y + (bounds.h - metrics.y * fit) / 2.0,
        );
        self.board_text(point, label, color, style, style.size() * fit);
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
    chrome: &Chrome,
    grid: PixelGrid,
    histories: &Histories,
) {
    let board = &chrome.board;
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
    if let Some(fullscreen) = chrome.fullscreen {
        paint.screen_fill(screen, BLACK);
        if fullscreen.show_exit {
            let button = fullscreen_exit_button(chrome.size);
            paint.screen_fill(button, HEADER);
            paint.text_clips.push(screen);
            let label = "Exit fullscreen";
            let x = button.x + (button.w - paint.text_width(label, Style::Label)) / 2.0;
            paint.screen_text(Point::new(x, button.y + 6.0), label, INK, Style::Label);
        }
        return;
    }
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

    for panel in board.windows() {
        if panel.kind() == Panel::LevelEditor {
            draw_level_editor(&mut paint, panel, chrome, screen);
            continue;
        }
        let bounds = panel.bounds;
        let dock = board.dock_group(panel.id);
        let shadow = match dock {
            Some((game, _)) if game == panel.id => board.dock_bounds(game),
            Some(_) => None,
            None => Some(bounds),
        };
        if let Some(shadow) = shadow {
            paint.fill(shadow.translated(super::board::PANEL_SHADOW), GRID);
        }
        let pixels = grid.panel(board, panel);
        paint.pixel_fill(
            pixels.outer,
            if dock.is_some()
                || board
                    .windows()
                    .next_back()
                    .is_some_and(|front| front.id == panel.id)
            {
                GREEN
            } else {
                BORDER
            },
        );
        paint.pixel_fill(pixels.interior, PAPER);
        paint.pixel_fill(pixels.header, HEADER);
        paint.text_clips = grid
            .visible_regions(chrome, panel.id, pixels.outer, None)
            .into_iter()
            .map(|bounds| grid.logical_bounds(bounds))
            .collect();
        match panel.kind() {
            Panel::Game => draw_game(
                &mut paint,
                panel,
                chrome.games.get(&panel.id),
                chrome.focus == Some(panel.id),
                pixels.body,
                board.picker(panel.id).is_some(),
            ),
            Panel::Timeline => draw_timeline(&mut paint, chrome, panel),
            Panel::Profiler => {
                let empty = History::default();
                let history = histories
                    .get(&panel.id)
                    .map_or(&empty, |(_, history)| history);
                profiler::draw(&mut paint, bounds, history, panel.attached_game().is_some())
            }
            Panel::LevelEditor => unreachable!(),
            Panel::TileAtlas | Panel::TextureInspector => {
                if let Some(view) = chrome.assets.get(&panel.id) {
                    draw_asset_panel(&mut paint, panel, view, chrome);
                }
            }
            Panel::Explorer => {
                if let Some(explorer) = chrome.explorers.get(&panel.id) {
                    draw_explorer(
                        &mut paint,
                        bounds,
                        explorer,
                        board.recent_projects(),
                        chrome.active_explorer == Some(panel.id),
                    );
                }
            }
        }
        if let Some(button) = panel.target_button() {
            draw_target_button(&mut paint, board, panel, button);
        }
        paint.centered_text(
            panel.close_button(),
            "×",
            if board.picker(panel.id).is_some()
                || chrome
                    .explorers
                    .get(&panel.id)
                    .is_some_and(|explorer| explorer.submitting)
            {
                BORDER
            } else {
                MUTED
            },
            Style::Label,
        );
        if panel.owner.is_none()
            && board.picker(panel.id).is_none()
            && (dock.is_none() || panel.kind() == Panel::Timeline)
        {
            draw_grip(&mut paint, bounds);
        }
    }
    if let Some(panel) = chrome.dock_target.and_then(|id| board.window(id)) {
        let bounds = panel.bounds;
        paint.horizontal_rule(
            Bounds::new(bounds.x, bounds.bottom() - 1.0, bounds.w, 4.0),
            GREEN,
        );
    }
    paint.text_clips = vec![screen];
    draw_panel_menu(&mut paint, chrome);
    if let Some(menu) = &chrome.target_menu {
        draw_target_menu(&mut paint, menu);
    }
    if let Some((layout, popup)) = &chrome.bookmark_popup {
        draw_bookmark_popup(&mut paint, *layout, popup);
    }
    if let Some(layout) = chrome.reset_prompt {
        draw_reset_prompt(&mut paint, layout);
    }
    if let Some(menu) = chrome.header_menu {
        paint.screen_fill(menu.bounds, BORDER);
        let bounds = Bounds::new(
            menu.bounds.x + 1.0,
            menu.bounds.y + 1.0,
            menu.bounds.w - 2.0,
            menu.bounds.h - 2.0,
        );
        paint.screen_fill(bounds, PAPER);
        paint.screen_text(
            Point::new(bounds.x + 10.0, bounds.y + 6.0),
            if menu.pending {
                "Duplicating…"
            } else {
                "Duplicate"
            },
            if menu.pending { MUTED } else { INK },
            Style::Label,
        );
    }
    if let Some(notice) = &chrome.notice {
        let width = screen.w.min(520.0).max(1.0);
        let bounds = Bounds::new((screen.w - width) / 2.0, 14.0, width, 30.0);
        paint.screen_fill(bounds, PAPER);
        let label = fit_label(&paint, notice, Style::Label, width - 22.0);
        paint.screen_text(
            Point::new(bounds.x + 10.0, bounds.y + 5.0),
            &label,
            RED,
            Style::Label,
        );
    }
}

fn draw_panel_menu(paint: &mut Painter<'_>, chrome: &Chrome) {
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
    paint.screen_fill(menu.popup, PAPER);
    for (index, panel) in Panel::ALL.into_iter().enumerate() {
        let label = match panel {
            Panel::Game => "Add Game",
            Panel::Timeline => "Add Timeline",
            _ => panel.title(),
        };
        let row = menu.row(index);
        paint.screen_fill(row, HEADER);
        paint.screen_text(
            Point::new(row.x + 9.0, row.y + 6.0),
            label,
            INK,
            Style::Label,
        );
    }
}

fn draw_level_editor(
    paint: &mut Painter<'_>,
    panel: &PanelWindow,
    chrome: &Chrome,
    screen: Bounds,
) {
    let Some(view) = chrome.levels.get(&panel.id) else {
        return;
    };
    let layout = level::Layout::new(panel, view.grid.as_ref().map(|grid| grid.size()));
    let visible = paint.board.world_at(Point::new(screen.x, screen.y));
    let visible = Bounds::new(
        visible.x,
        visible.y,
        screen.w / paint.board.zoom,
        screen.h / paint.board.zoom,
    );
    if let (Some(grid), Some(palette), Some((columns, rows))) =
        (&view.grid, view.palette, layout.visible_tiles(visible))
    {
        let brush_solid = view.brush == level::Brush::Paint;
        for y in rows.clone() {
            let mut start = None;
            for x in columns.clone() {
                let cell = Cell::new(x, y);
                let (solid, sprite) = if view.draft.contains(&cell) {
                    (brush_solid, view.sprite.as_ref().filter(|_| brush_solid))
                } else {
                    (grid.solid(cell), grid.sprite(cell))
                };
                let sprite = sprite.and_then(|tile| {
                    chrome
                        .level_tilesets
                        .get(&panel.id)
                        .map(|sets| sets.sprite(tile))
                });
                if solid && sprite.is_none() {
                    start.get_or_insert(x);
                    continue;
                }
                if let Some(first) = start.take() {
                    paint.fill(layout.row(first, y, x), palette.solid);
                }
                if let Some((image, source)) = sprite {
                    paint.image(image, source, layout.row(x, y, x + 1), visible);
                }
            }
            if let Some(first) = start {
                paint.fill(layout.row(first, y, columns.end), palette.solid);
            }
        }
        let bounds = layout.grid;
        if view.selected && level::Layout::show_grid_lines(paint.board.zoom) {
            for x in columns {
                let x = layout.row(x, 0, x).x;
                paint.vertical_rule(Bounds::new(x, bounds.y, 1.0, bounds.h), GRID);
            }
            for y in rows {
                let y = layout.row(0, y, 0).y;
                paint.horizontal_rule(Bounds::new(bounds.x, y, bounds.w, 1.0), GRID);
            }
        }
        let border = if view.selected { GREEN } else { BORDER };
        for x in [bounds.x, bounds.right()] {
            paint.vertical_rule(Bounds::new(x, bounds.y, 1.0, bounds.h), border);
        }
        for y in [bounds.y, bounds.bottom()] {
            paint.horizontal_rule(Bounds::new(bounds.x, y, bounds.w, 1.0), border);
        }
        if view.selected {
            let handle = (7.0 / paint.board.zoom).min(14.0);
            for (x, y) in [
                (bounds.right(), bounds.y + bounds.h / 2.0),
                (bounds.x + bounds.w / 2.0, bounds.bottom()),
                (bounds.right(), bounds.bottom()),
            ] {
                paint.fill(
                    Bounds::new(x - handle / 2.0, y - handle / 2.0, handle, handle),
                    GREEN,
                );
            }
        }
    }

    let toolbar = paint.grid.panel(paint.board, panel);
    paint.pixel_fill(toolbar.outer, if view.selected { GREEN } else { BORDER });
    paint.pixel_fill(toolbar.interior, HEADER);
    paint.text_clips = paint
        .grid
        .visible_regions(chrome, panel.id, toolbar.outer, None)
        .into_iter()
        .map(|bounds| paint.grid.logical_bounds(bounds))
        .collect();
    let Some(button) = panel.target_button() else {
        return;
    };
    draw_target_button(paint, paint.board, panel, button);
    paint.centered_text(panel.close_button(), "×", MUTED, Style::Label);
    let Some(grid) = &view.grid else {
        let message = view
            .error
            .as_deref()
            .unwrap_or(if panel.attached_game().is_some() {
                "Waiting for a tile level"
            } else {
                "Choose a Game window"
            });
        let status = Bounds::new(
            layout.paint.x,
            layout.status.y,
            layout.status.right() - layout.paint.x,
            layout.status.h,
        );
        let message = fit_label(paint, message, Style::Caption, status.w - 8.0);
        paint.centered_text(status, &message, MUTED, Style::Caption);
        return;
    };
    for (button, mode, label) in [
        (
            layout.paint,
            level::Brush::Paint,
            if chrome.picked_tile.is_some() {
                "Plain"
            } else {
                "Paint"
            },
        ),
        (layout.erase, level::Brush::Erase, "Erase"),
    ] {
        let active =
            view.brush == mode && (mode == level::Brush::Erase || chrome.picked_tile.is_none());
        paint.fill(button, if active { GREEN_LIGHT } else { HEADER });
        paint.centered_text(
            button,
            label,
            if active { GREEN } else { INK },
            Style::Label,
        );
    }
    let size = format!("{} × {}", grid.width(), grid.height());
    let (status, color) = if view.error.is_some() {
        ("Save failed", RED)
    } else {
        (size.as_str(), MUTED)
    };
    paint.centered_text(layout.status, status, color, Style::Caption);
    if let Some(error) = chrome
        .level_tilesets
        .get(&panel.id)
        .and_then(|assets| assets.errors.first())
    {
        let warning = Bounds::new(
            layout.toolbar.x,
            layout.toolbar.bottom() + 5.0,
            layout.toolbar.w,
            23.0,
        );
        let pixels = paint.grid.snap(paint.board.project(warning));
        paint.text_clips = paint
            .grid
            .visible_regions(chrome, panel.id, pixels, None)
            .into_iter()
            .map(|bounds| paint.grid.logical_bounds(bounds))
            .collect();
        paint.fill(warning, PAPER);
        let label = fit_suffix(paint, error, Style::Caption, warning.w - 12.0);
        paint.text(
            Point::new(warning.x + 6.0, warning.y + 3.0),
            &label,
            RED,
            Style::Caption,
        );
    }
}

fn draw_asset_panel(
    paint: &mut Painter<'_>,
    panel: &PanelWindow,
    view: &art::AssetView,
    chrome: &Chrome,
) {
    let kind = panel.kind();
    let atlas_panel = kind == Panel::TileAtlas;
    let layout = art::Layout::new(panel.bounds);
    paint.fill(layout.image, HEADER);
    let title = panel.title();
    let origin = Point::new(panel.bounds.x + 12.0, panel.bounds.y + 6.0);
    paint.text(origin, title, INK, Style::Title);
    if let Some(path) = &view.source {
        let x = origin.x + paint.text_width(title, Style::Title) + 12.0;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let label = fit_label(paint, &name, Style::Label, panel.close_button().x - x - 8.0);
        paint.text(Point::new(x, origin.y + 1.0), &label, MUTED, Style::Label);
    }
    let image = view.image.as_ref();
    let status = if let Some(image) = image {
        let size = image.size();
        let image_bounds = view.camera.image(layout.image, size);
        if let Some(visible) = image_bounds.intersect(layout.image) {
            draw_checkerboard(paint, visible);
            paint.image(
                image,
                Rect::from_size(size.width, size.height),
                image_bounds,
                layout.image,
            );
        }
        if let Some(atlas) = &view.atlas {
            let tile_width = image_bounds.w / f64::from(atlas.dimensions().width);
            let tile_height = image_bounds.h / f64::from(atlas.dimensions().height);
            if tile_width.min(tile_height) * paint.board.zoom >= 9.0
                && let Some(visible) = image_bounds.intersect(layout.image)
            {
                let lines = |start: f64, end: f64, origin: f64, step: f64| {
                    let first = ((start - origin) / step).ceil() as u32;
                    let last = ((end - origin) / step).floor() as u32;
                    (first..=last).map(move |i| origin + f64::from(i) * step)
                };
                for x in lines(visible.x, visible.right(), image_bounds.x, tile_width) {
                    paint.vertical_rule(Bounds::new(x, visible.y, 1.0, visible.h), BORDER);
                }
                for y in lines(visible.y, visible.bottom(), image_bounds.y, tile_height) {
                    paint.horizontal_rule(Bounds::new(visible.x, y, visible.w, 1.0), BORDER);
                }
            }
            if let Some(tile) = chrome
                .picked_tile
                .as_ref()
                .filter(|pick| Some(&pick.definition) == view.source.as_ref())
                && atlas.region(tile.tile).is_some()
            {
                let x = image_bounds.x + f64::from(tile.tile.x) * tile_width;
                let y = image_bounds.y + f64::from(tile.tile.y) * tile_height;
                let thickness = 2.0_f64.min(tile_width).min(tile_height);
                for edge in [
                    Bounds::new(x, y, tile_width, thickness),
                    Bounds::new(x, y + tile_height - thickness, tile_width, thickness),
                    Bounds::new(x, y, thickness, tile_height),
                    Bounds::new(x + tile_width - thickness, y, thickness, tile_height),
                ] {
                    if let Some(edge) = edge.intersect(layout.image) {
                        paint.fill(edge, GREEN);
                    }
                }
            }
            format!(
                "{} × {} px per tile",
                atlas.tile_size.width, atlas.tile_size.height
            )
        } else {
            format!(
                "{} × {} px · {:.0}%",
                size.width,
                size.height,
                view.camera.scale(layout.image, size) * 100.0
            )
        }
    } else if view.source.is_some() {
        "Loading…".into()
    } else if atlas_panel {
        "Open an .atlas.json file".into()
    } else {
        "Open a PNG".into()
    };
    let (status, color) = view
        .error
        .as_deref()
        .map_or((status.as_str(), MUTED), |error| (error, RED));
    let label = fit_suffix(paint, status, Style::Caption, layout.status.w - 8.0);
    paint.text(
        Point::new(layout.status.x + 2.0, layout.status.y + 5.0),
        &label,
        color,
        Style::Caption,
    );
    paint.fill(layout.open, GREEN_LIGHT);
    paint.centered_text(layout.open, "Open…", GREEN, Style::Caption);
    if image.is_some() {
        paint.fill(layout.action, GREEN_LIGHT);
        paint.centered_text(
            layout.action,
            if atlas_panel { "Inspect" } else { "Fit" },
            GREEN,
            Style::Caption,
        );
    }
}

fn draw_checkerboard(paint: &mut Painter<'_>, bounds: Bounds) {
    let area = paint.grid.snap(paint.board.project(bounds));
    let Some(visible) = area.intersect(viewport(paint.grid.surface)) else {
        return;
    };
    paint.pixel_fill(visible, PAPER);
    let step = (10.0 * paint.grid.scale.x).round().max(1.0);
    let first_x = ((visible.x - area.x) / step).floor() as u32;
    let first_y = ((visible.y - area.y) / step).floor() as u32;
    let mut y = area.y + f64::from(first_y) * step;
    let mut row = first_y;
    while y < visible.bottom() {
        let mut x = area.x + f64::from(first_x) * step;
        let mut column = first_x;
        while x < visible.right() {
            if (row + column) % 2 == 0
                && let Some(square) = Bounds::new(x, y, step, step).intersect(visible)
            {
                paint.pixel_fill(square, GRID);
            }
            x += step;
            column += 1;
        }
        y += step;
        row += 1;
    }
}

fn draw_target_button(
    paint: &mut Painter<'_>,
    board: &Board,
    source: &PanelWindow,
    button: Bounds,
) {
    let panel = source.bounds;
    paint.text(
        Point::new(panel.x + 12.0, panel.y + 6.0),
        source.title(),
        INK,
        Style::Title,
    );
    let target = source.attached_game().and_then(|id| board.window(id));
    let label = target.map_or_else(
        || "Attach to Game".into(),
        |target| board.window_label(target),
    );
    let label = fit_game_label(paint, &label, Style::Label, button.w - 32.0);
    paint.fill(button, if target.is_some() { GREEN_LIGHT } else { PAPER });
    paint.text(
        Point::new(button.x + 8.0, button.y + 3.0),
        &label,
        GREEN,
        Style::Label,
    );
    let arrow_x = button.right() - 17.0;
    let arrow_y = button.y + button.h / 2.0 - 1.0;
    for (inset, width) in [(0.0, 7.0), (1.0, 5.0), (2.0, 3.0), (3.0, 1.0)] {
        paint.horizontal_rule(
            Bounds::new(arrow_x + inset, arrow_y + inset, width, 1.0),
            GREEN,
        );
    }
}

fn draw_target_menu(paint: &mut Painter<'_>, menu: &target::Menu) {
    let bounds = menu.bounds;
    paint.screen_fill(bounds, BORDER);
    paint.screen_fill(
        Bounds::new(
            bounds.x + 1.0,
            bounds.y + 1.0,
            bounds.w - 2.0,
            bounds.h - 2.0,
        ),
        PAPER,
    );
    for (index, choice) in menu
        .rows
        .iter()
        .skip(menu.offset)
        .take(menu.visible)
        .enumerate()
    {
        let row = menu.row(index);
        let selected = choice.target == menu.selected;
        if selected {
            paint.screen_fill(row, GREEN_LIGHT);
        }
        let label = fit_game_label(paint, &choice.label, Style::Label, row.w - 20.0);
        paint.screen_text(
            Point::new(row.x + 9.0, row.y + 5.0),
            &label,
            if selected { GREEN } else { INK },
            Style::Label,
        );
    }
    if menu.max_offset() > 0 {
        let scale = (bounds.h - 8.0) / menu.rows.len() as f64;
        paint.screen_fill(
            Bounds::new(
                bounds.right() - 4.0,
                bounds.y + 4.0 + menu.offset as f64 * scale,
                2.0,
                menu.visible as f64 * scale,
            ),
            BORDER,
        );
    }
}

fn draw_target_drag(gfx: &mut GpuRenderer2d, chrome: &Chrome, grid: PixelGrid, drag: target::Drag) {
    let board = &chrome.board;
    let Some(source) = board.window(drag.source) else {
        return;
    };
    let Some(button) = source.target_button() else {
        return;
    };
    for window in board
        .windows()
        .filter(|window| window.kind() != Panel::Game)
    {
        let outer = grid.panel(board, window).outer;
        for rect in grid.visible_regions(chrome, window.id, outer, None) {
            gfx.blend_rect(pixel_rect(rect), [130, 131, 127, 255], 112);
        }
    }
    if let Some(window) = drag.hovered.and_then(|id| board.window(id)) {
        let outer = grid.panel(board, window).outer;
        let scale = grid.scale.x.min(grid.scale.y);
        for region in grid.visible_regions(chrome, window.id, outer, None) {
            draw_target_outline(gfx, outer, region, scale, drag.phase);
        }
    }
    let button = board.project(button);
    let from = Point::new(button.x + button.w / 2.0, button.bottom());
    let physical = |point: Point| Point::new(point.x * grid.scale.x, point.y * grid.scale.y);
    draw_target_wire(gfx, physical(from), physical(drag.pointer), grid);
}

fn pixel_rect(bounds: Bounds) -> Rect {
    Rect::new(
        bounds.x as u32,
        bounds.y as u32,
        bounds.w as u32,
        bounds.h as u32,
    )
}

fn draw_target_outline(
    gfx: &mut dyn Renderer2d,
    outer: Bounds,
    clip: Bounds,
    scale: f64,
    phase: u8,
) {
    let thickness = (2.0 * scale).round().max(1.0).min(outer.w).min(outer.h);
    let pitch = (9.0 * scale).round().max(1.0);
    let dot = (2.5 * scale).round().max(1.0);
    let travel = f64::from(phase) * pitch / 9.0;
    let left = outer.x;
    let right = outer.right();
    let top = outer.y;
    let bottom = outer.bottom();
    let edges = [
        (Bounds::new(left, top, outer.w, thickness), true, false, 0.0),
        (
            Bounds::new(right - thickness, top, thickness, outer.h),
            false,
            false,
            outer.w,
        ),
        (
            Bounds::new(left, bottom - thickness, outer.w, thickness),
            true,
            true,
            outer.w + outer.h,
        ),
        (
            Bounds::new(left, top, thickness, outer.h),
            false,
            true,
            2.0 * outer.w + outer.h,
        ),
    ];
    for (edge, horizontal, reverse, distance) in edges {
        let Some(visible) = edge.intersect(clip) else {
            continue;
        };
        let (origin, length, lower, upper) = if horizontal {
            (edge.x, edge.w, visible.x, visible.right())
        } else {
            (edge.y, edge.h, visible.y, visible.bottom())
        };
        let shift = if reverse {
            distance + length - travel - dot
        } else {
            travel - distance
        };
        let first = origin + shift.rem_euclid(pitch).round();
        let mut at = first + ((lower - first) / pitch).floor() * pitch;
        while at < upper {
            let mark = if horizontal {
                Bounds::new(at, edge.y, dot, edge.h)
            } else {
                Bounds::new(edge.x, at, edge.w, dot)
            };
            if let Some(mark) = mark.intersect(visible) {
                gfx.fill_rect(pixel_rect(mark), GREEN);
            }
            at += pitch;
        }
    }
}

fn draw_target_wire(gfx: &mut dyn Renderer2d, from: Point, to: Point, grid: PixelGrid) {
    let screen = viewport(grid.surface);
    let thickness = (1.5 * grid.scale.x.min(grid.scale.y)).round().max(2.0);
    // Trace the segment at device-pixel spacing and clip each sample to the surface.
    let from = Point::new(from.x.clamp(0.0, screen.w), from.y.clamp(0.0, screen.h));
    let to = Point::new(to.x.clamp(0.0, screen.w), to.y.clamp(0.0, screen.h));
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as u32;
    for step in 0..=steps {
        let fraction = f64::from(step) / f64::from(steps);
        let x = (from.x + dx * fraction - thickness / 2.0).round();
        let y = (from.y + dy * fraction - thickness / 2.0).round();
        if let Some(rect) = Bounds::new(x, y, thickness, thickness).intersect(screen) {
            gfx.fill_rect(pixel_rect(rect), GREEN);
        }
    }
}

fn draw_explorer(
    paint: &mut Painter<'_>,
    bounds: Bounds,
    explorer: &Explorer,
    recent: &[std::path::PathBuf],
    active: bool,
) {
    let layout = ExplorerLayout::new(bounds);
    let browsing = explorer.targets.is_empty();
    let title = if browsing {
        explorer.mode.title()
    } else {
        "Choose a Cargo binary"
    };
    paint.text(
        Point::new(bounds.x + 12.0, bounds.y + 6.0),
        title,
        if active { GREEN } else { INK },
        Style::Title,
    );
    paint.fill(layout.up, HEADER);
    paint.centered_text(
        layout.up,
        if browsing { "↑" } else { "←" },
        GREEN,
        Style::Title,
    );
    let path_focused = active && browsing && explorer.field == Some(Field::Path);
    paint.fill(layout.path, if path_focused { GREEN_LIGHT } else { HEADER });
    let path = fit_suffix(paint, &explorer.path, Style::Number, layout.path.w - 16.0);
    paint.text(
        Point::new(layout.path.x + 8.0, layout.path.y + 6.0),
        &path,
        INK,
        Style::Number,
    );
    if path_focused && !explorer.all_selected() {
        let x = layout.path.x + 9.0 + paint.text_width(&path, Style::Number);
        paint.vertical_rule(Bounds::new(x, layout.path.y + 5.0, 1.0, 17.0), GREEN);
    }

    paint.fill(layout.sidebar, HEADER);
    paint.fill(layout.list, HEADER);
    if browsing {
        for (index, shortcut) in explorer::shortcuts(recent).iter().enumerate() {
            if index == 2 {
                let y = layout.shortcut(index).y;
                paint.text(
                    Point::new(layout.sidebar.x + 7.0, y - 18.0),
                    "Recent projects",
                    MUTED,
                    Style::Caption,
                );
            }
            let row = layout.shortcut(index);
            if explorer
                .directory
                .as_ref()
                .is_some_and(|directory| directory.path == shortcut.path)
            {
                paint.fill(row, GREEN_LIGHT);
            }
            let label = fit_label(paint, &shortcut.name, Style::Label, row.w - 15.0);
            paint.text(
                Point::new(row.x + 7.0, row.y + 4.0),
                &label,
                INK,
                Style::Label,
            );
        }
    }
    let visible = layout.visible();
    let offset = explorer.current_offset(visible);
    for index in offset..(offset + visible).min(explorer.rows()) {
        let row = layout.row(index - offset);
        let (label, detail) = if let Some(target) = explorer.targets.get(index) {
            (format!("{} / {}", target.package, target.binary), "")
        } else if let Some(entry) = explorer
            .directory
            .as_ref()
            .and_then(|directory| directory.entries.get(index))
        {
            let kind = match entry.kind {
                EntryKind::Project => "Cargo project",
                EntryKind::Folder => "Folder",
                EntryKind::Manifest if explorer.mode == ExplorerMode::Open => "Open",
                _ => "",
            };
            (entry.name.clone(), kind)
        } else {
            continue;
        };
        paint.fill(
            row,
            if explorer.selected == Some(index) {
                GREEN_LIGHT
            } else if index % 2 == 0 {
                PAPER
            } else {
                HEADER
            },
        );
        let detail_width = paint.text_width(detail, Style::Caption);
        let label = fit_label(paint, &label, Style::Label, row.w - detail_width - 25.0);
        paint.text(
            Point::new(row.x + 8.0, row.y + 4.0),
            &label,
            INK,
            Style::Label,
        );
        paint.text(
            Point::new(row.right() - detail_width - 11.0, row.y + 5.0),
            detail,
            MUTED,
            Style::Caption,
        );
    }
    if explorer.rows() > visible {
        let count = explorer.rows() as f64;
        paint.fill(
            Bounds::new(
                layout.list.right() - 3.0,
                layout.list.y + layout.list.h * offset as f64 / count,
                3.0,
                layout.list.h * visible as f64 / count,
            ),
            BORDER,
        );
    }
    let message = if let Some(error) = &explorer.error {
        (error.lines().next().unwrap_or_default(), RED)
    } else if explorer.submitting {
        (
            if explorer.mode == ExplorerMode::Create {
                "Creating project…"
            } else {
                "Reading Cargo targets…"
            },
            MUTED,
        )
    } else if explorer.busy() {
        ("Reading folder…", MUTED)
    } else if !browsing {
        ("Choose the binary to run in this Game window", MUTED)
    } else if explorer.mode == ExplorerMode::Open
        && !explorer
            .directory
            .as_ref()
            .is_some_and(|directory| directory.is_project)
    {
        ("Choose a folder with a Cargo.toml", MUTED)
    } else if explorer.rows() == 0 {
        ("No items in this folder", MUTED)
    } else {
        ("", MUTED)
    };
    let text = fit_label(paint, message.0, Style::Caption, layout.message.w);
    paint.text(
        Point::new(layout.message.x, layout.message.y + 2.0),
        &text,
        message.1,
        Style::Caption,
    );
    if !browsing {
        return;
    }

    if explorer.mode == ExplorerMode::Create {
        let name_focused = active && explorer.field == Some(Field::Name);
        paint.fill(layout.name, if name_focused { GREEN_LIGHT } else { HEADER });
        let placeholder = explorer.name.is_empty() && !name_focused;
        let name = fit_suffix(
            paint,
            if placeholder {
                "Game folder name"
            } else {
                &explorer.name
            },
            Style::Label,
            layout.name.w - 16.0,
        );
        paint.text(
            Point::new(layout.name.x + 8.0, layout.name.y + 5.0),
            &name,
            if placeholder { MUTED } else { INK },
            Style::Label,
        );
        if name_focused && !explorer.all_selected() {
            let x = layout.name.x + 9.0 + paint.text_width(&name, Style::Label);
            paint.vertical_rule(Bounds::new(x, layout.name.y + 5.0, 1.0, 17.0), GREEN);
        }
    }
    if explorer.mode != ExplorerMode::Browse {
        paint.fill(
            layout.confirm,
            if explorer.busy() { HEADER } else { GREEN_LIGHT },
        );
        paint.centered_text(
            layout.confirm,
            explorer.mode.title(),
            if explorer.busy() { MUTED } else { GREEN },
            Style::Label,
        );
    }
}

fn fit_suffix(paint: &Painter<'_>, label: &str, style: Style, width: f64) -> String {
    if paint.text_width(label, style) <= width {
        return label.to_owned();
    }
    let allowance = width - paint.text_width("…", style);
    let mut start = label.len();
    for (index, _) in label.char_indices().rev() {
        if paint.text_width(&label[index..], style) > allowance {
            break;
        }
        start = index;
    }
    format!("…{}", &label[start..])
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

fn draw_game(
    paint: &mut Painter<'_>,
    panel: &PanelWindow,
    game: Option<&GameChrome>,
    focused: bool,
    body_pixels: Bounds,
    blocked: bool,
) {
    let bounds = panel.bounds;
    let body = panel.game_body();
    let Some(game) = game else {
        paint.text(
            Point::new(bounds.x + 12.0, bounds.y + 6.0),
            "Game",
            INK,
            Style::Title,
        );
        if blocked {
            return;
        }
        paint.text(
            Point::new(body.x + 17.0, body.y + 18.0),
            "Start with a Rust project",
            MUTED,
            Style::Label,
        );
        let (new, open) = panel.project_buttons();
        paint.fill(new, GREEN_LIGHT);
        paint.fill(open, HEADER);
        paint.centered_text(new, "New Game", GREEN, Style::Label);
        paint.centered_text(open, "Open Game", INK, Style::Label);
        return;
    };
    let header = GameHeader::new(bounds);
    let phase = match game.phase {
        Phase::Building => Some(("Building", MUTED)),
        Phase::Reconnecting => Some(("Connecting", MUTED)),
        Phase::Running => None,
        Phase::Failed => Some(("Build failed", RED)),
        Phase::GpuFailed => Some(("GPU unavailable", RED)),
        Phase::ReloadFailed => Some(("Reload failed", RED)),
        Phase::Exited => Some(("Game exited", RED)),
        Phase::Disabling => Some(("Stopping", MUTED)),
        Phase::Disabled => Some(("Disabled", MUTED)),
        Phase::Enabling => Some(("Starting", MUTED)),
        Phase::DisableFailed => Some(("Disable failed", RED)),
    };
    let title_color = phase
        .filter(|(_, color)| *color == RED)
        .map_or(if focused { GREEN } else { INK }, |(_, color)| color);
    let volume = game.master_volume.filter(|_| header.volume_track.w > 0.0);
    let left_control = if volume.is_some() {
        header.volume_track.x - 20.0
    } else {
        header.zoom_track.x - 12.0
    };
    let title_at = Point::new(bounds.x + 12.0, bounds.y + 6.0);
    let title = fit_game_label(
        paint,
        &paint.board.window_label(panel),
        Style::Title,
        left_control - title_at.x - 6.0,
    );
    paint.text(title_at, &title, title_color, Style::Title);
    let status_x = title_at.x + paint.text_width(&title, Style::Title) + 12.0;
    if let Some((label, color)) = phase
        && left_control - status_x >= paint.text_width(label, Style::Caption)
    {
        paint.text(
            Point::new(status_x, bounds.y + 7.0),
            label,
            color,
            Style::Caption,
        );
    }

    if let Some(percent) = volume {
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
    let thumb = header.thumb_x(panel.game_zoom());
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
    if let Some(size) = game.frame_size.filter(|_| header.resolution.w > 0.0) {
        paint.centered_text(
            header.resolution,
            &format!("{} × {}", size.width, size.height),
            MUTED,
            Style::Number,
        );
    }
    let (label, color) = match game.phase {
        Phase::Disabled => ("Enable", GREEN),
        Phase::Disabling => ("Stopping", MUTED),
        Phase::Enabling => ("Starting", MUTED),
        Phase::DisableFailed => ("Retry", RED),
        _ => ("Disable", INK),
    };
    paint.fill(
        header.lifecycle_button,
        if game.phase == Phase::Disabled {
            GREEN_LIGHT
        } else {
            PAPER
        },
    );
    paint.centered_text(header.lifecycle_button, label, color, Style::Caption);
    paint.fill(
        header.fullscreen_button,
        if game.connected { PAPER } else { HEADER },
    );
    paint.icon(
        header.fullscreen_button,
        [
            0b1110111, 0b1000001, 0b1000001, 0, 0b1000001, 0b1000001, 0b1110111,
        ],
        if game.connected { INK } else { BORDER },
    );
    paint.pixel_fill(body_pixels, BLACK);
    if game.frame_size.is_none() {
        let message = game
            .error
            .as_deref()
            .and_then(|message| message.lines().next())
            .unwrap_or_else(|| phase.map_or("Waiting for game", |(label, _)| label));
        let message = fit_label(paint, message, Style::Label, body.w - 40.0);
        paint.text(
            Point::new(body.x + 20.0, body.y + 20.0),
            &message,
            WHITE,
            Style::Label,
        );
    }
}

fn fit_game_label(paint: &Painter<'_>, label: &str, style: Style, width: f64) -> String {
    if label.contains(" · ") && width > paint.text_width("…", style) {
        fit_suffix(paint, label, style, width)
    } else {
        fit_label(paint, label, style, width)
    }
}

fn fit_label(paint: &Painter<'_>, label: &str, style: Style, width: f64) -> String {
    if paint.text_width(label, style) <= width {
        return label.to_owned();
    }
    let mut label = label.to_owned();
    let width = width - paint.text_width("…", style);
    if width <= 0.0 {
        return String::new();
    }
    while paint.text_width(&label, style) > width {
        label.pop();
    }
    label.push('…');
    label
}

fn draw_timeline(paint: &mut Painter<'_>, chrome: &Chrome, window: &PanelWindow) {
    let panel = window.bounds;
    let target = window.attached_game();
    let Some(game) = target.and_then(|id| chrome.games.get(&id)) else {
        paint.text(
            Point::new(panel.x + 16.0, panel.y + TITLE_HEIGHT + 16.0),
            if target.is_some() {
                "Waiting for a project"
            } else {
                "Choose a Game window"
            },
            MUTED,
            Style::Label,
        );
        return;
    };
    let layout = TimelineLayout::new(panel);
    let timeline = game.timeline;
    let cursor = chrome
        .drag_ms
        .filter(|(id, _)| *id == window.id)
        .map(|(_, at)| at)
        .or(timeline.cursor.map(|frame| frame.at_ms))
        .unwrap_or(0);
    let tip = timeline.tip.map_or(0, |frame| frame.at_ms);
    let reset = layout.reset_button();
    paint.fill(reset, GREEN_LIGHT);
    paint.centered_text(
        reset,
        "Reset",
        if game.connected && timeline.tip.is_some() {
            INK
        } else {
            MUTED
        },
        Style::Label,
    );
    let time_label = Bounds::new(reset.x - 151.0, panel.y + 2.0, 145.0, TITLE_HEIGHT - 4.0);
    if window
        .target_button()
        .is_some_and(|button| button.right() + 6.0 < time_label.x)
    {
        let time = if timeline.detached_bookmark.is_some() && chrome.drag_ms.is_none() {
            format!("Bookmark · {}", timestamp(cursor))
        } else {
            format!("{} / {}", timestamp(cursor), timestamp(tip))
        };
        paint.centered_text(time_label, &time, MUTED, Style::Number);
    }
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
    if timeline.detached_bookmark.is_none() || chrome.drag_ms.is_some() {
        paint.fill(
            Bounds::new(track.x + progress - 3.0, track.y - 4.0, 6.0, track.h + 8.0),
            GREEN,
        );
    }
    let bookmark = layout.bookmark_button();
    let bookmark_open = chrome
        .bookmark_popup
        .as_ref()
        .is_some_and(|(_, popup)| popup.window == window.id);
    paint.fill(bookmark, if bookmark_open { GREEN_LIGHT } else { HEADER });
    paint.icon(
        bookmark,
        [
            0b0111110, 0b0100010, 0b0100010, 0b0100010, 0b0101010, 0b0110110, 0b0100010,
        ],
        if bookmark_open { GREEN } else { INK },
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
        let active = game.connected && dock_command(control, timeline).is_some();
        let button = layout.button(control);
        let primary = active && control == Transport::Play;
        let color = if primary {
            GREEN
        } else if active
            && (control == Transport::TakeOver || chrome.hovered == Some((window.id, control)))
        {
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

fn draw_bookmark_popup(
    paint: &mut Painter<'_>,
    layout: bookmarks::Layout,
    popup: &bookmarks::Popup,
) {
    let bounds = layout.bounds;
    paint.screen_fill(bounds, BORDER);
    paint.screen_fill(
        Bounds::new(
            bounds.x + 1.0,
            bounds.y + 1.0,
            bounds.w - 2.0,
            bounds.h - 2.0,
        ),
        PAPER,
    );
    paint.screen_fill(layout.name, HEADER);
    paint.screen_fill(layout.save, if popup.can_save { GREEN } else { HEADER });
    let label = if popup.name.is_empty() {
        "Bookmark name"
    } else {
        &popup.name
    };
    let label = fit_label(paint, label, Style::Label, layout.name.w - 17.0);
    paint.screen_text(
        Point::new(layout.name.x + 8.0, layout.name.y + 5.0),
        &label,
        if popup.can_save && !popup.name.is_empty() {
            INK
        } else {
            MUTED
        },
        Style::Label,
    );
    paint.screen_text(
        Point::new(layout.save.x + 11.0, layout.save.y + 5.0),
        "Save",
        if popup.can_save { WHITE } else { MUTED },
        Style::Label,
    );

    if let Some(message) = popup
        .error
        .as_deref()
        .or_else(|| popup.entries.is_empty().then_some("No bookmarks yet"))
    {
        let row = layout.row(0);
        let message = fit_label(paint, message, Style::Label, row.w - 12.0);
        paint.screen_text(
            Point::new(row.x + 3.0, row.y + 5.0),
            &message,
            MUTED,
            Style::Label,
        );
        return;
    }
    for (index, bookmark) in popup
        .entries
        .iter()
        .skip(layout.offset(popup))
        .take(layout.visible)
        .enumerate()
    {
        let row = layout.row(index);
        let selected = popup.selected == Some(bookmark.id);
        let time = timestamp(bookmark.frame.at_ms);
        let remove = layout.remove(index);
        let time_x = remove.x - 10.0 - paint.text_width(&time, Style::Number);
        let label = fit_label(paint, &bookmark.name, Style::Label, time_x - row.x - 14.0);
        if selected {
            paint.screen_fill(row, GREEN_LIGHT);
        }
        paint.screen_text(
            Point::new(row.x + 4.0, row.y + 5.0),
            &label,
            if selected { GREEN } else { INK },
            Style::Label,
        );
        paint.screen_text(Point::new(time_x, row.y + 5.0), &time, MUTED, Style::Number);
        if popup.connected && !selected {
            paint.screen_text(
                Point::new(remove.x + 7.0, row.y + 5.0),
                "×",
                MUTED,
                Style::Label,
            );
        }
    }
}

fn draw_reset_prompt(paint: &mut Painter<'_>, layout: TimelineResetLayout) {
    let bounds = layout.bounds;
    paint.screen_fill(bounds, BORDER);
    paint.screen_fill(
        Bounds::new(
            bounds.x + 1.0,
            bounds.y + 1.0,
            bounds.w - 2.0,
            bounds.h - 2.0,
        ),
        PAPER,
    );
    let lines = [
        (11.0, "Reset timeline?", Style::Title, INK),
        (
            35.0,
            "Clear recorded history and restart the game.",
            Style::Label,
            INK,
        ),
        (54.0, "Saved bookmarks will stay.", Style::Label, MUTED),
    ];
    for (y, text, style, color) in lines {
        let text = fit_label(paint, text, style, bounds.w - 28.0);
        paint.screen_text(
            Point::new(bounds.x + 14.0, bounds.y + y),
            &text,
            color,
            style,
        );
    }
    for (button, text, background, ink) in [
        (layout.cancel, "Cancel", HEADER, INK),
        (layout.confirm, "Reset", RED, WHITE),
    ] {
        paint.screen_fill(button, background);
        let x = button.x + (button.w - paint.text_width(text, Style::Label)) / 2.0;
        paint.screen_text(Point::new(x, button.y + 5.0), text, ink, Style::Label);
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
struct PreviewUniforms {
    position: vec4<f32>,
    effect: vec4<f32>,
}
@group(0) @binding(0) var<uniform> preview: PreviewUniforms;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let uv = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    return Vertex(vec4<f32>(preview.position.xy + uv * preview.position.zw, 0.0, 1.0), uv);
}

@fragment fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    let center = textureSample(image, image_sampler, input.uv);
    let blur = preview.effect.x;
    if blur <= 0.0 {
        return center;
    }
    let delta = vec2<f32>(5.0 * blur) / vec2<f32>(textureDimensions(image, 0));
    let x = vec2<f32>(delta.x, 0.0);
    let y = vec2<f32>(0.0, delta.y);
    let uv = input.uv;
    let axial = textureSample(image, image_sampler, uv - x)
        + textureSample(image, image_sampler, uv + x)
        + textureSample(image, image_sampler, uv - y)
        + textureSample(image, image_sampler, uv + y);
    let corners = textureSample(image, image_sampler, uv - x - y)
        + textureSample(image, image_sampler, uv + x - y)
        + textureSample(image, image_sampler, uv - x + y)
        + textureSample(image, image_sampler, uv + x + y);
    let blurred = (4.0 * center + 2.0 * axial + corners) / 16.0;
    return vec4<f32>(blurred.rgb * (1.0 - 0.32 * blur), blurred.a);
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
            size: 32,
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

    fn clear(&mut self) {
        self.image = None;
        #[cfg(target_os = "macos")]
        {
            self.cached_surfaces.clear();
            self.rejected_surface = None;
        }
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

    fn place(&self, queue: &wgpu::Queue, rect: Bounds, surface: SurfaceSize, blur: f32) {
        let sx = 2.0 / f64::from(surface.width);
        let sy = 2.0 / f64::from(surface.height);
        let position = [
            (rect.x * sx - 1.0) as f32,
            (1.0 - rect.y * sy) as f32,
            (rect.w * sx) as f32,
            (-rect.h * sy) as f32,
            blur,
            0.0,
            0.0,
            0.0,
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
                let mut board = Board::default();
                board.pan = Point::new(pan, pan);
                board.zoom = 0.4;
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
    fn a_higher_game_window_occludes_only_the_overlapping_preview() {
        let logical = SurfaceSize::new(800, 600);
        let grid = PixelGrid::new(logical, SurfaceSize::new(1600, 1200));
        let mut board = Board::default();
        board.pan = Point::new(0.6, 0.3);
        board.zoom = 0.6;
        let lower = board.add(Panel::Game, viewport(logical));
        let upper = board.add(Panel::Game, viewport(logical));
        board.set_bounds(lower, Bounds::new(50.0, 50.0, 600.0, 480.0));
        board.set_bounds(upper, Bounds::new(375.0, 100.0, 470.0, 370.0));
        let chrome = Chrome {
            board,
            size: logical,
            games: BTreeMap::new(),
            focus: None,
            hovered: None,
            drag_ms: None,
            dock_target: None,
            menu_open: false,
            target_menu: None,
            header_menu: None,
            notice: None,
            bookmark_popup: None,
            reset_prompt: None,
            levels: BTreeMap::new(),
            picked_tile: None,
            level_tilesets: BTreeMap::new(),
            assets: BTreeMap::new(),
            explorers: BTreeMap::new(),
            active_explorer: None,
            fullscreen: None,
        };
        let panel = grid.panel(&chrome.board, chrome.board.window(lower).unwrap());
        let cover = grid.panel(&chrome.board, chrome.board.window(upper).unwrap());
        let visible = grid.visible_regions(&chrome, lower, panel.body, None);
        let draws_at = |x, y| visible.iter().any(|rect| rect.contains(Point::new(x, y)));
        assert_eq!(panel.body.y, panel.header.bottom());
        assert!(draws_at(panel.body.x, panel.body.y));
        assert!(!draws_at(panel.body.x - 1.0, panel.body.y));
        assert!(draws_at(cover.outer.x - 1.0, cover.outer.y + 1.0));
        assert!(!draws_at(cover.outer.x, cover.outer.y + 1.0));
    }
}
