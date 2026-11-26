use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use engine::embedded::{DEFAULT_PREVIEW_SIZE, MAX_PREVIEW_DIMENSION};
use engine::persistence::{JsonWriteOptions, write_json_atomic};
use engine::surface::{DisplayMetrics, SurfaceSize};

use crate::project::Project;
use serde::{Deserialize, Serialize};

pub const TITLE_HEIGHT: f64 = 30.0;
const LEVEL_TOOLBAR_WIDTH: f64 = 470.0;
const MIN_ZOOM: f64 = 0.03;
const MAX_ZOOM: f64 = 4.0;
const MIN_GAME_ZOOM: f64 = 0.5;
const MAX_GAME_ZOOM: f64 = 2.0;
const EDGE: f64 = 7.0;
const GRIP: f64 = 11.0;
const DOCK_DISTANCE: f64 = 16.0;
const MIN_TIMELINE_HEIGHT: f64 = 90.0;
const DEFAULT_TIMELINE_HEIGHT: f64 = 96.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Bounds {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(self) -> f64 {
        self.y + self.h
    }

    pub fn translated(self, offset: Point) -> Self {
        Self::new(self.x + offset.x, self.y + offset.y, self.w, self.h)
    }

    pub fn grip(self) -> Self {
        Self::new(self.right() - GRIP, self.bottom() - GRIP, GRIP, GRIP)
    }

    pub fn contains(self, point: Point) -> bool {
        self.x <= point.x && point.x < self.right() && self.y <= point.y && point.y < self.bottom()
    }

    pub fn intersect(self, rhs: Self) -> Option<Self> {
        let x = self.x.max(rhs.x);
        let y = self.y.max(rhs.y);
        let w = self.right().min(rhs.right()) - x;
        let h = self.bottom().min(rhs.bottom()) - y;
        (w > 0.0 && h > 0.0).then(|| Self::new(x, y, w, h))
    }

    pub fn excluding(self, above: Self) -> Vec<Self> {
        let Some(cover) = self.intersect(above) else {
            return vec![self];
        };
        [
            Self::new(self.x, self.y, self.w, cover.y - self.y),
            Self::new(
                self.x,
                cover.bottom(),
                self.w,
                self.bottom() - cover.bottom(),
            ),
            Self::new(self.x, cover.y, cover.x - self.x, cover.h),
            Self::new(
                cover.right(),
                cover.y,
                self.right() - cover.right(),
                cover.h,
            ),
        ]
        .into_iter()
        .filter(|part| part.w > 0.0 && part.h > 0.0)
        .collect()
    }

    pub fn pixel(self, point: Point, size: SurfaceSize) -> Option<(u32, u32)> {
        self.contains(point).then(|| {
            let x = ((point.x - self.x) / self.w * f64::from(size.width)).floor() as u32;
            let y = ((point.y - self.y) / self.h * f64::from(size.height)).floor() as u32;
            (x.min(size.width - 1), y.min(size.height - 1))
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Panel {
    Game,
    Timeline,
    Profiler,
    Explorer,
    LevelEditor,
    TileAtlas,
    TextureInspector,
}

impl Panel {
    pub const ALL: [Self; 7] = [
        Self::Game,
        Self::Timeline,
        Self::Profiler,
        Self::Explorer,
        Self::LevelEditor,
        Self::TileAtlas,
        Self::TextureInspector,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Timeline => "Timeline",
            Self::Profiler => "Profiler",
            Self::Explorer => "Explorer",
            Self::LevelEditor => "Level Editor",
            Self::TileAtlas => "Tile Atlas",
            Self::TextureInspector => "Texture Inspector",
        }
    }

    fn minimum(self) -> Point {
        match self {
            Self::Game => Point::new(260.0, 200.0),
            Self::Timeline => Point::new(260.0, MIN_TIMELINE_HEIGHT),
            Self::Profiler => Point::new(420.0, 330.0),
            Self::Explorer => Point::new(520.0, 338.0),
            Self::LevelEditor => Point::new(400.0, 250.0),
            Self::TileAtlas => Point::new(390.0, 244.0),
            Self::TextureInspector => Point::new(442.0, 244.0),
        }
    }
}

pub type WindowId = u64;
pub const PANEL_SHADOW: Point = Point::new(4.0, 5.0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub pan: Point,
    pub zoom: f64,
    windows: Vec<PanelWindow>,
    next_id: WindowId,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    recent_projects: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelWindow {
    pub id: WindowId,
    pub bounds: Bounds,
    pub content: Content,
    #[serde(skip)]
    pub owner: Option<WindowId>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Content {
    Game {
        project: Option<Project>,
        enabled: bool,
        zoom: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance: Option<String>,
    },
    Timeline {
        game: Option<WindowId>,
        docked_to: Option<WindowId>,
    },
    Profiler {
        #[serde(default)]
        game: Option<WindowId>,
    },
    LevelEditor {
        game: Option<WindowId>,
    },
    TileAtlas {
        atlas: Option<PathBuf>,
    },
    TextureInspector {
        texture: Option<PathBuf>,
        #[serde(rename = "game", skip_serializing)]
        legacy_game: Option<WindowId>,
    },
    Explorer {
        directory: Option<PathBuf>,
    },
}

impl PanelWindow {
    pub fn kind(&self) -> Panel {
        match self.content {
            Content::Game { .. } => Panel::Game,
            Content::Timeline { .. } => Panel::Timeline,
            Content::Profiler { .. } => Panel::Profiler,
            Content::LevelEditor { .. } => Panel::LevelEditor,
            Content::TileAtlas { .. } => Panel::TileAtlas,
            Content::TextureInspector { .. } => Panel::TextureInspector,
            Content::Explorer { .. } => Panel::Explorer,
        }
    }

    pub fn project(&self) -> Option<&Project> {
        match &self.content {
            Content::Game { project, .. } => project.as_ref(),
            _ => None,
        }
    }

    pub fn title(&self) -> &str {
        self.project()
            .map_or_else(|| self.kind().title(), Project::label)
    }

    pub fn attached_game(&self) -> Option<WindowId> {
        match self.content {
            Content::Timeline { game, .. }
            | Content::Profiler { game }
            | Content::LevelEditor { game } => game,
            _ => None,
        }
    }

    pub fn asset_source(&self) -> Option<&Path> {
        match &self.content {
            Content::TileAtlas { atlas } => atlas.as_deref(),
            Content::TextureInspector { texture, .. } => texture.as_deref(),
            _ => None,
        }
    }

    pub fn instance(&self) -> Option<&str> {
        if let Content::Game { instance, .. } = &self.content {
            instance.as_deref()
        } else {
            None
        }
    }

    pub fn game_zoom(&self) -> f64 {
        match self.content {
            Content::Game { zoom, .. } => zoom,
            _ => 1.0,
        }
    }

    pub fn game_body(&self) -> Bounds {
        let bounds = self.bounds;
        Bounds::new(
            bounds.x + 1.0,
            bounds.y + TITLE_HEIGHT,
            bounds.w - 2.0,
            bounds.h - TITLE_HEIGHT - 1.0,
        )
    }

    pub fn game_display(&self, dpi: f64) -> DisplayMetrics {
        let body = self.game_body();
        let surface = SurfaceSize::new(
            (body.w * dpi).round().max(1.0) as u32,
            (body.h * dpi).round().max(1.0) as u32,
        );
        self.fullscreen_display(surface, dpi)
    }

    pub fn fullscreen_display(&self, surface: SurfaceSize, dpi: f64) -> DisplayMetrics {
        let width = f64::from(surface.width.max(1));
        let height = f64::from(surface.height.max(1));
        let limit = f64::from(MAX_PREVIEW_DIMENSION);
        let zoom = self.game_zoom();
        let scale = zoom.recip().min(limit / width.max(height));
        DisplayMetrics {
            render_size: SurfaceSize::new(
                (width * scale).round().clamp(1.0, limit) as u32,
                (height * scale).round().clamp(1.0, limit) as u32,
            ),
            logical_size: surface.to_logical(dpi * zoom),
            scale_factor: dpi,
        }
    }

    pub fn close_button(&self) -> Bounds {
        let bounds = self.chrome_bounds();
        Bounds::new(bounds.right() - 25.0, bounds.y + 4.0, 20.0, 22.0)
    }

    pub fn chrome_bounds(&self) -> Bounds {
        if self.kind() == Panel::LevelEditor {
            Bounds::new(
                self.bounds.x,
                self.bounds.y,
                LEVEL_TOOLBAR_WIDTH,
                TITLE_HEIGHT,
            )
        } else {
            self.bounds
        }
    }

    pub fn stack_drag_at(&self, point: Point) -> bool {
        self.bounds.contains(point)
            && point.y < self.bounds.y + TITLE_HEIGHT
            && !self.close_button().contains(point)
    }

    pub fn target_button(&self) -> Option<Bounds> {
        let offset = match self.kind() {
            Panel::Profiler | Panel::Timeline => 80.0,
            Panel::LevelEditor => 114.0,
            _ => return None,
        };
        Some(Bounds::new(
            self.bounds.x + offset,
            self.bounds.y + 4.0,
            if self.kind() == Panel::Timeline {
                (self.bounds.w - offset - 90.0).clamp(1.0, 132.0)
            } else {
                132.0
            },
            22.0,
        ))
    }

    pub fn project_buttons(&self) -> (Bounds, Bounds) {
        let x = self.bounds.x + 18.0;
        let y = self.bounds.y + TITLE_HEIGHT + 50.0;
        (
            Bounds::new(x, y, 100.0, 30.0),
            Bounds::new(x + 112.0, y, 100.0, 30.0),
        )
    }
}

pub fn viewport(size: SurfaceSize) -> Bounds {
    Bounds::new(0.0, 0.0, f64::from(size.width), f64::from(size.height))
}

pub(super) fn fullscreen_exit_button(size: SurfaceSize) -> Bounds {
    Bounds::new((f64::from(size.width) - 154.0).max(0.0), 12.0, 142.0, 28.0)
}

impl Default for Board {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            recent_projects: Vec::new(),
            pan: Point::default(),
            zoom: 1.0,
        }
    }
}

impl Board {
    fn add_at(&mut self, kind: Panel, bounds: Bounds) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        let content = match kind {
            Panel::Game => Content::Game {
                project: None,
                enabled: true,
                zoom: 1.0,
                instance: None,
            },
            Panel::Timeline => Content::Timeline {
                game: None,
                docked_to: None,
            },
            Panel::Profiler => Content::Profiler { game: None },
            Panel::LevelEditor => Content::LevelEditor { game: None },
            Panel::TileAtlas => Content::TileAtlas { atlas: None },
            Panel::TextureInspector => Content::TextureInspector {
                texture: None,
                legacy_game: None,
            },
            Panel::Explorer => Content::Explorer { directory: None },
        };
        self.windows.push(PanelWindow {
            id,
            bounds,
            content,
            owner: None,
        });
        id
    }

    pub fn with_example(project: Project, enabled: bool) -> Self {
        let mut board = Self::default();
        let game = Bounds::new(
            32.0,
            28.0,
            f64::from(DEFAULT_PREVIEW_SIZE.width) + 2.0,
            f64::from(DEFAULT_PREVIEW_SIZE.height) + TITLE_HEIGHT + 1.0,
        );
        let game_id = board.add_at(Panel::Game, game);
        let timeline = board.add_at(
            Panel::Timeline,
            Bounds::new(
                game.x,
                game.bottom() + 20.0,
                game.w,
                DEFAULT_TIMELINE_HEIGHT,
            ),
        );
        board
            .attach_project(game_id, project)
            .expect("new game window");
        board.set_enabled(game_id, enabled);
        board.attach_target(timeline, Some(game_id));
        board
    }

    pub fn add(&mut self, kind: Panel, view: Bounds) -> WindowId {
        if matches!(kind, Panel::Profiler | Panel::LevelEditor)
            && let Some(window) = self.windows.iter().find(|window| window.kind() == kind)
        {
            let id = window.id;
            self.raise(id);
            return id;
        }
        let (w, h) = match kind {
            Panel::Game => (656.0, 410.0),
            Panel::Timeline => (540.0, DEFAULT_TIMELINE_HEIGHT),
            Panel::Profiler => (620.0, 484.0),
            Panel::Explorer => (620.0, 424.0),
            Panel::LevelEditor => (660.0, 448.0),
            Panel::TileAtlas => (510.0, 382.0),
            Panel::TextureInspector => (570.0, 420.0),
        };
        let left = (view.w - w * self.zoom).max(0.0);
        let top = (view.h - h * self.zoom).max(0.0);
        let offset = (self.windows.len() % 7) as f64 * 26.0;
        let origin = self.world_at(Point::new(
            (left - 26.0 - offset).max(26.0),
            (top / 2.0 + offset).min(view.h - 65.0).max(26.0),
        ));
        self.add_at(kind, Bounds::new(origin.x, origin.y, w, h))
    }

    pub fn duplicate(&mut self, mut copy: PanelWindow, instance: Option<String>) -> WindowId {
        let id = self.next_id;
        self.next_id += 1;
        copy.id = id;
        copy.owner = None;
        copy.bounds = copy.bounds.translated(Point::new(28.0, 28.0));
        match &mut copy.content {
            Content::Timeline { docked_to, .. } => *docked_to = None,
            Content::Game { instance: slot, .. } => *slot = instance,
            _ => {}
        }
        self.windows.push(copy);
        id
    }

    pub fn window_label(&self, panel: &PanelWindow) -> String {
        let title = panel.title();
        if panel.kind() == Panel::Game
            && self.windows().any(|other| {
                other.id != panel.id && other.kind() == Panel::Game && other.title() == title
            })
        {
            format!("{title} · {}", panel.id)
        } else {
            title.into()
        }
    }

    pub fn window(&self, id: WindowId) -> Option<&PanelWindow> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut PanelWindow> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    pub fn windows(&self) -> impl DoubleEndedIterator<Item = &PanelWindow> {
        self.windows
            .iter()
            .filter(|window| window.kind() == Panel::LevelEditor)
            .chain(
                self.windows
                    .iter()
                    .filter(|window| window.kind() != Panel::LevelEditor),
            )
    }

    pub fn panel_at(&self, world: Point) -> Option<WindowId> {
        self.windows()
            .rev()
            .find(|window| window.chrome_bounds().contains(world))
            .map(|window| window.id)
    }

    pub fn picker(&self, owner: WindowId) -> Option<WindowId> {
        self.windows()
            .find(|window| window.owner == Some(owner))
            .map(|window| window.id)
    }

    pub fn open_picker(&mut self, owner: WindowId) -> Option<WindowId> {
        let window = self.window(owner)?;
        match window.kind() {
            Panel::Game if window.project().is_none() => {}
            Panel::TileAtlas | Panel::TextureInspector => {}
            _ => return None,
        }
        if let Some(picker) = self.picker(owner) {
            self.raise(owner);
            return Some(picker);
        }
        let bounds = Self::picker_bounds(window.bounds);
        let id = self.add_at(Panel::Explorer, bounds);
        self.window_mut(id).unwrap().owner = Some(owner);
        self.raise(owner);
        Some(id)
    }

    fn picker_bounds(parent: Bounds) -> Bounds {
        let min = Panel::Explorer.minimum();
        let width = parent.w.clamp(min.x, 660.0);
        let height = parent.h.clamp(410.0, 490.0);
        Bounds::new(
            parent.x + 24.0,
            parent.y + TITLE_HEIGHT + 6.0,
            width,
            height,
        )
    }

    pub fn recent_projects(&self) -> &[PathBuf] {
        &self.recent_projects
    }

    pub fn note_project(&mut self, directory: PathBuf) {
        self.recent_projects.retain(|item| item != &directory);
        self.recent_projects.insert(0, directory);
        self.recent_projects.truncate(5);
    }

    pub fn set_explorer_directory(&mut self, id: WindowId, path: PathBuf) {
        if let Some(PanelWindow {
            content: Content::Explorer { directory },
            owner: None,
            ..
        }) = self.window_mut(id)
        {
            *directory = Some(path);
        }
    }

    pub fn raise(&mut self, id: WindowId) {
        let root = self
            .window(id)
            .and_then(|window| window.owner)
            .unwrap_or(id);
        let group = self.dock_group(root);
        let game = group.map_or(root, |(game, _)| game);
        let order = [
            Some(game),
            group.map(|(_, timeline)| timeline),
            self.picker(game),
        ];
        for id in order.into_iter().flatten() {
            if let Some(index) = self.windows.iter().position(|window| window.id == id) {
                let window = self.windows.remove(index);
                self.windows.push(window);
            }
        }
    }

    pub fn close(&mut self, id: WindowId) {
        self.windows
            .retain(|window| window.id != id && window.owner != Some(id));
        for window in &mut self.windows {
            match &mut window.content {
                Content::Timeline { game, docked_to } => {
                    if *game == Some(id) {
                        *game = None;
                    }
                    if *docked_to == Some(id) {
                        *docked_to = None;
                    }
                }
                Content::Profiler { game } | Content::LevelEditor { game } if *game == Some(id) => {
                    *game = None;
                }
                _ => {}
            }
        }
    }

    pub fn attach_project(&mut self, id: WindowId, project: Project) -> io::Result<()> {
        if self.windows().any(|window| {
            window.id != id
                && window.instance().is_none()
                && window.project().is_some_and(|other| {
                    other.manifest == project.manifest && other.binary == project.binary
                })
        }) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "That game is already open on this board",
            ));
        }
        match self.window_mut(id).map(|window| &mut window.content) {
            Some(Content::Game {
                project: slot @ None,
                ..
            }) => {
                *slot = Some(project);
                Ok(())
            }
            _ => Err(io::Error::other("Choose an empty Game window")),
        }
    }

    pub fn game_at(&self, world: Point) -> Option<WindowId> {
        self.panel_at(world).filter(|&id| {
            self.window(id)
                .is_some_and(|window| window.kind() == Panel::Game)
        })
    }

    pub fn attach_target(&mut self, id: WindowId, target: Option<WindowId>) {
        if target.is_some_and(|target| {
            self.window(target)
                .is_none_or(|window| window.kind() != Panel::Game)
        }) {
            return;
        }
        let Some(window) = self.window_mut(id) else {
            return;
        };
        match &mut window.content {
            Content::Timeline { game, .. }
            | Content::Profiler { game }
            | Content::LevelEditor { game } => *game = target,
            _ => {}
        }
    }

    pub fn set_asset_source(&mut self, id: WindowId, path: PathBuf) {
        match self.window_mut(id).map(|window| &mut window.content) {
            Some(Content::TileAtlas { atlas }) => *atlas = Some(path),
            Some(Content::TextureInspector { texture, .. }) => *texture = Some(path),
            _ => {}
        }
    }

    pub fn set_enabled(&mut self, id: WindowId, value: bool) {
        if let Some(PanelWindow {
            content: Content::Game { enabled, .. },
            ..
        }) = self.window_mut(id)
        {
            *enabled = value;
        }
    }

    pub fn set_game_zoom(&mut self, id: WindowId, value: f64) {
        if let Some(PanelWindow {
            content: Content::Game { zoom, .. },
            ..
        }) = self.window_mut(id)
        {
            *zoom = value.clamp(MIN_GAME_ZOOM, MAX_GAME_ZOOM);
        }
    }

    pub fn dock_group(&self, id: WindowId) -> Option<(WindowId, WindowId)> {
        self.windows().find_map(|window| {
            if let Content::Timeline {
                docked_to: Some(game),
                ..
            } = window.content
                && (window.id == id || game == id)
            {
                Some((game, window.id))
            } else {
                None
            }
        })
    }

    pub fn dock_bounds(&self, id: WindowId) -> Option<Bounds> {
        let (game, timeline) = self.dock_group(id)?;
        let game = self.window(game)?.bounds;
        let timeline = self.window(timeline)?.bounds;
        Some(Bounds::new(game.x, game.y, game.w, game.h + timeline.h))
    }

    pub fn dock_candidate(&self, timeline: WindowId) -> Option<WindowId> {
        let window = self.window(timeline)?;
        self.windows()
            .rev()
            .filter(|other| other.kind() == Panel::Game && self.dock_group(other.id).is_none())
            .find(|game| {
                (window.bounds.y - game.bounds.bottom()).abs() * self.zoom <= DOCK_DISTANCE
                    && window.bounds.right() > game.bounds.x + 30.0
                    && window.bounds.x < game.bounds.right() - 30.0
            })
            .map(|game| game.id)
    }

    pub fn dock(&mut self, timeline: WindowId, game: WindowId) {
        let Some(target) = self.window(game).map(|window| window.bounds) else {
            return;
        };
        if let Some(window) = self.window_mut(timeline)
            && let Content::Timeline { docked_to, .. } = &mut window.content
        {
            *docked_to = Some(game);
            window.bounds = Bounds::new(target.x, target.bottom(), target.w, window.bounds.h);
        }
        self.raise(game);
    }

    pub fn undock(&mut self, timeline: WindowId) {
        if let Some(PanelWindow {
            content: Content::Timeline { docked_to, .. },
            ..
        }) = self.window_mut(timeline)
        {
            *docked_to = None;
        }
    }

    pub fn set_bounds(&mut self, id: WindowId, bounds: Bounds) {
        let dock = self.dock_group(id);
        if let Some(window) = self.window_mut(id) {
            window.bounds = bounds;
        }
        if let Some((game, timeline)) = dock
            && game == id
            && let Some(timeline) = self.window_mut(timeline)
        {
            timeline.bounds = Bounds::new(bounds.x, bounds.bottom(), bounds.w, timeline.bounds.h);
        }
        if let Some(picker) = self.picker(id).and_then(|id| self.window_mut(id)) {
            picker.bounds = Self::picker_bounds(bounds);
        }
    }

    pub fn world_at(&self, screen: Point) -> Point {
        Point::new(
            (screen.x - self.pan.x) / self.zoom,
            (screen.y - self.pan.y) / self.zoom,
        )
    }

    pub fn screen_at(&self, world: Point) -> Point {
        Point::new(
            world.x * self.zoom + self.pan.x,
            world.y * self.zoom + self.pan.y,
        )
    }

    pub fn project(&self, world: Bounds) -> Bounds {
        let point = self.screen_at(Point::new(world.x, world.y));
        Bounds::new(point.x, point.y, world.w * self.zoom, world.h * self.zoom)
    }

    pub fn zoom_at(&mut self, screen: Point, factor: f64) {
        let world = self.world_at(screen);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.pan = Point::new(
            screen.x - world.x * self.zoom,
            screen.y - world.y * self.zoom,
        );
    }

    pub fn fit(&mut self, view: Bounds, level: impl IntoIterator<Item = Bounds>) {
        let mut windows = self.windows().map(PanelWindow::chrome_bounds).chain(level);
        let Some(mut bounds) = windows.next() else {
            return;
        };
        for next in windows {
            let x = bounds.x.min(next.x);
            let y = bounds.y.min(next.y);
            bounds = Bounds::new(
                x,
                y,
                bounds.right().max(next.right()) - x,
                bounds.bottom().max(next.bottom()) - y,
            );
        }
        self.zoom = ((view.w - 48.0) / bounds.w)
            .min((view.h - 48.0) / bounds.h)
            .clamp(MIN_ZOOM, 1.0);
        self.pan = Point::new(
            view.x + (view.w - bounds.w * self.zoom) / 2.0 - bounds.x * self.zoom,
            view.y + (view.h - bounds.h * self.zoom) / 2.0 - bounds.y * self.zoom,
        );
    }

    pub fn load(
        path: &Path,
        example: &Project,
        example_enabled: bool,
    ) -> io::Result<Option<(Self, bool)>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let (mut board, legacy) = match serde_json::from_slice::<Self>(&bytes) {
            Ok(board) => (board, false),
            Err(_) => (
                Self::from_legacy(
                    serde_json::from_slice(&bytes)?,
                    example.clone(),
                    example_enabled,
                ),
                true,
            ),
        };
        let migrated: Vec<_> = board
            .windows()
            .filter_map(|panel| {
                let Content::TextureInspector {
                    texture: Some(path),
                    legacy_game,
                } = &panel.content
                else {
                    return None;
                };
                if path.is_absolute() {
                    return None;
                }
                let path = legacy_game
                    .and_then(|id| board.window(id))
                    .and_then(PanelWindow::project)
                    .map(|project| project.root().join("assets").join(path));
                Some((panel.id, path))
            })
            .collect();
        for (id, path) in &migrated {
            if let Some(PanelWindow {
                content:
                    Content::TextureInspector {
                        texture,
                        legacy_game,
                    },
                ..
            }) = board.window_mut(*id)
            {
                *texture = path.clone();
                *legacy_game = None;
            }
        }
        board.validate()?;
        Ok(Some((board, legacy || !migrated.is_empty())))
    }

    fn from_legacy(old: LegacyBoard, example: Project, enabled: bool) -> Self {
        let mut board = Self::with_example(example, enabled);
        board.pan = old.pan;
        board.zoom = old.zoom;
        let game = board.windows[0].id;
        let timeline = board.windows[1].id;
        board.set_bounds(game, old.game);
        board.set_bounds(timeline, old.timeline);
        board.set_game_zoom(game, old.game_zoom);
        if old.timeline_docked {
            board.dock(timeline, game);
        }
        if old.hide_timeline {
            board.close(timeline);
        }
        if old.hide_game {
            board.close(game);
        }
        let profiler = old
            .show_profiler
            .then(|| board.add_at(Panel::Profiler, old.profiler));
        for panel in old.stack {
            let id = match panel {
                Panel::Game => Some(game),
                Panel::Timeline => Some(timeline),
                Panel::Profiler => profiler,
                Panel::Explorer
                | Panel::LevelEditor
                | Panel::TileAtlas
                | Panel::TextureInspector => None,
            };
            if let Some(id) = id {
                board.raise(id);
            }
        }
        board
    }

    fn validate(&self) -> io::Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        let mut projects = std::collections::BTreeSet::new();
        let mut docks = std::collections::BTreeSet::new();
        let valid_game = |id| {
            self.window(id)
                .is_some_and(|window| window.kind() == Panel::Game)
        };
        let valid = (1..WindowId::MAX).contains(&self.next_id)
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
            && self.pan.x.is_finite()
            && self.pan.y.is_finite()
            && self.windows().all(|window| {
                let bounds = window.bounds;
                let min = window.kind().minimum();
                window.id < self.next_id
                    && ids.insert(window.id)
                    && [bounds.x, bounds.y, bounds.w, bounds.h]
                        .into_iter()
                        .all(f64::is_finite)
                    && bounds.w >= min.x
                    && bounds.h >= min.y
                    && match &window.content {
                        Content::Game {
                            project,
                            zoom,
                            instance,
                            ..
                        } => {
                            (MIN_GAME_ZOOM..=MAX_GAME_ZOOM).contains(zoom)
                                && (project.is_some() || instance.is_none())
                                && project.as_ref().is_none_or(|project| {
                                    project.manifest.is_absolute()
                                        && !project.binary.is_empty()
                                        && instance.as_ref().is_none_or(|id| {
                                            !id.is_empty()
                                                && id.bytes().all(|byte| {
                                                    byte.is_ascii_hexdigit() || byte == b'-'
                                                })
                                        })
                                        && projects.insert((
                                            &project.manifest,
                                            &project.binary,
                                            instance,
                                        ))
                                })
                        }
                        Content::Timeline { game, docked_to } => {
                            game.is_none_or(valid_game)
                                && docked_to.is_none_or(|id| valid_game(id) && docks.insert(id))
                        }
                        Content::Profiler { game } | Content::LevelEditor { game } => {
                            game.is_none_or(valid_game)
                        }
                        Content::TileAtlas { atlas } => atlas.as_ref().is_none_or(|path| {
                            path.is_absolute() && engine::tilesets::is_atlas(path)
                        }),
                        Content::TextureInspector { texture, .. } => {
                            texture.as_ref().is_none_or(|path| {
                                path.is_absolute() && engine::tilesets::is_png(path)
                            })
                        }
                        Content::Explorer { directory } => {
                            directory.as_ref().is_none_or(|path| path.is_absolute())
                        }
                    }
            });
        if valid {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid editor board",
            ))
        }
    }

    pub fn persisted(&self) -> Self {
        let mut board = self.clone();
        board.windows.retain(|window| window.owner.is_none());
        board
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_json_atomic(
            path,
            &self.persisted(),
            JsonWriteOptions {
                sync_file: false,
                ..JsonWriteOptions::PRETTY
            },
        )
    }
}

#[derive(Deserialize)]
struct LegacyBoard {
    game: Bounds,
    timeline: Bounds,
    pan: Point,
    zoom: f64,
    #[serde(default = "default_legacy_stack")]
    stack: [Panel; 3],
    #[serde(default = "unit_zoom")]
    game_zoom: f64,
    #[serde(default)]
    timeline_docked: bool,
    #[serde(default = "default_legacy_profiler")]
    profiler: Bounds,
    #[serde(default)]
    show_profiler: bool,
    #[serde(default)]
    hide_game: bool,
    #[serde(default)]
    hide_timeline: bool,
}

fn unit_zoom() -> f64 {
    1.0
}
fn default_legacy_stack() -> [Panel; 3] {
    [Panel::Profiler, Panel::Timeline, Panel::Game]
}
fn default_legacy_profiler() -> Bounds {
    Bounds::new(820.0, 45.0, 620.0, 484.0)
}
#[derive(Clone, Copy)]
pub struct PanelMenu {
    pub button: Bounds,
    pub popup: Bounds,
}

impl PanelMenu {
    pub fn new(size: SurfaceSize) -> Self {
        let button = Bounds::new(16.0, f64::from(size.height) - 52.0, 36.0, 36.0);
        let height = Panel::ALL.len() as f64 * 30.0 + 10.0;
        Self {
            button,
            popup: Bounds::new(button.x, button.y - height - 7.0, 168.0, height),
        }
    }

    pub fn covers(self, open: bool) -> impl Iterator<Item = Bounds> {
        [Some(self.button), open.then_some(self.popup)]
            .into_iter()
            .flatten()
    }

    pub fn row(self, index: usize) -> Bounds {
        Bounds::new(
            self.popup.x + 4.0,
            self.popup.y + 5.0 + index as f64 * 30.0,
            self.popup.w - 8.0,
            30.0,
        )
    }

    pub fn panel_at(self, point: Point) -> Option<Panel> {
        Panel::ALL
            .into_iter()
            .enumerate()
            .find(|(index, _)| self.row(*index).contains(point))
            .map(|(_, panel)| panel)
    }
}

#[derive(Clone, Copy)]
pub struct Edges {
    pub horizontal: i8,
    pub vertical: i8,
}

impl Edges {
    pub fn within(bounds: Bounds, zoom: f64, point: Point) -> Option<Self> {
        if bounds.grip().contains(point) {
            return Some(Self {
                horizontal: 1,
                vertical: 1,
            });
        }
        let margin = (EDGE / zoom).min(bounds.w.min(bounds.h) / 5.0);
        let horizontal = if point.x < bounds.x + margin {
            -1
        } else if point.x >= bounds.right() - margin {
            1
        } else {
            0
        };
        let vertical = if point.y < bounds.y + margin {
            -1
        } else if point.y >= bounds.bottom() - margin {
            1
        } else {
            0
        };
        (horizontal != 0 || vertical != 0).then_some(Self {
            horizontal,
            vertical,
        })
    }

    pub fn resize(self, panel: Panel, initial: Bounds, delta: Point, dpi: f64) -> Bounds {
        let min = panel.minimum();
        let (max_w, max_h) = if panel == Panel::Game {
            let max_body = f64::from(MAX_PREVIEW_DIMENSION) / dpi;
            (
                (max_body + 2.0).max(min.x),
                (max_body + TITLE_HEIGHT + 1.0).max(min.y),
            )
        } else {
            (f64::INFINITY, f64::INFINITY)
        };
        let w = (initial.w + delta.x * f64::from(self.horizontal)).clamp(min.x, max_w);
        let h = (initial.h + delta.y * f64::from(self.vertical)).clamp(min.y, max_h);
        let x = if self.horizontal < 0 {
            initial.right() - w
        } else {
            initial.x
        };
        let y = if self.vertical < 0 {
            initial.bottom() - h
        } else {
            initial.y
        };
        Bounds::new(x, y, w, h)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Back,
    Play,
    Next,
    TakeOver,
}

pub const TRANSPORT_CONTROLS: [Transport; 4] = [
    Transport::Back,
    Transport::Play,
    Transport::Next,
    Transport::TakeOver,
];

#[derive(Clone, Copy)]
pub struct GameHeader {
    pub zoom_track: Bounds,
    pub volume_track: Bounds,
    pub resolution: Bounds,
    pub lifecycle_button: Bounds,
    pub fullscreen_button: Bounds,
}

impl GameHeader {
    pub fn new(panel: Bounds) -> Self {
        let fullscreen_button = Bounds::new(
            panel.right() - 54.0,
            panel.y + 3.0,
            23.0,
            TITLE_HEIGHT - 6.0,
        );
        let lifecycle_width = if panel.w < 365.0 { 58.0 } else { 70.0 };
        let lifecycle_button = Bounds::new(
            fullscreen_button.x - lifecycle_width - 6.0,
            panel.y + 3.0,
            lifecycle_width,
            TITLE_HEIGHT - 6.0,
        );
        let resolution_width = if panel.w >= 365.0 {
            (panel.w * 0.22).clamp(72.0, 104.0)
        } else {
            0.0
        };
        let resolution = Bounds::new(
            lifecycle_button.x - resolution_width - 7.0,
            panel.y + 2.0,
            resolution_width,
            TITLE_HEIGHT - 4.0,
        );
        let zoom_width = (panel.w * 0.16).clamp(40.0, 116.0);
        let zoom_track = Bounds::new(
            resolution.x - zoom_width - 16.0,
            panel.y + 14.0,
            zoom_width,
            3.0,
        );
        let available_volume = (zoom_track.x - panel.x - 94.0).max(0.0);
        let volume_width = (panel.w * 0.11).clamp(28.0, 92.0).min(available_volume);
        let volume_width = if volume_width >= 18.0 {
            volume_width
        } else {
            0.0
        };
        let volume_track = Bounds::new(
            zoom_track.x - volume_width - 20.0,
            zoom_track.y,
            volume_width,
            zoom_track.h,
        );
        Self {
            zoom_track,
            volume_track,
            resolution,
            lifecycle_button,
            fullscreen_button,
        }
    }

    pub fn hit_volume(self, point: Point) -> bool {
        self.volume_track.w > 0.0
            && Bounds::new(
                self.volume_track.x - 20.0,
                self.volume_track.y - 7.0,
                self.volume_track.w + 26.0,
                self.volume_track.h + 14.0,
            )
            .contains(point)
    }

    pub fn volume_at(self, x: f64) -> u8 {
        (((x - self.volume_track.x) / self.volume_track.w).clamp(0.0, 1.0) * 100.0).round() as u8
    }

    pub fn volume_thumb_x(self, percent: u8) -> f64 {
        self.volume_track.x + f64::from(percent.min(100)) / 100.0 * self.volume_track.w
    }

    pub fn hit_zoom(self, point: Point) -> bool {
        Bounds::new(
            self.zoom_track.x - 12.0,
            self.zoom_track.y - 7.0,
            self.zoom_track.w + 24.0,
            self.zoom_track.h + 14.0,
        )
        .contains(point)
    }

    pub fn zoom_at(self, x: f64) -> f64 {
        let fraction = ((x - self.zoom_track.x) / self.zoom_track.w).clamp(0.0, 1.0);
        if (fraction - 0.5).abs() < 0.015 {
            1.0
        } else {
            (fraction * 2.0 - 1.0).exp2()
        }
    }

    pub fn thumb_x(self, zoom: f64) -> f64 {
        self.zoom_track.x + (zoom.log2() + 1.0) / 2.0 * self.zoom_track.w
    }
}

#[derive(Clone, Copy)]
pub struct TimelineLayout {
    panel: Bounds,
    pub track: Bounds,
}

impl TimelineLayout {
    pub fn new(panel: Bounds) -> Self {
        Self {
            panel,
            track: Bounds::new(
                panel.x + 16.0,
                panel.y + TITLE_HEIGHT + 10.0 + (panel.h - MIN_TIMELINE_HEIGHT) / 2.0,
                panel.w - 32.0,
                8.0,
            ),
        }
    }

    pub fn button(self, control: Transport) -> Bounds {
        let height = 24.0;
        let y = self.panel.bottom() - height - 9.0;
        let takeover_width = (self.panel.w - 194.0).clamp(70.0, 104.0);
        let takeover_x = self.panel.right() - takeover_width - 16.0;
        let center = (self.panel.x + self.panel.w / 2.0).min(takeover_x - 67.0);
        let (offset, width) = match control {
            Transport::Back => (-57.0, 30.0),
            Transport::Play => (-19.0, 38.0),
            Transport::Next => (27.0, 30.0),
            Transport::TakeOver => return Bounds::new(takeover_x, y, takeover_width, height),
        };
        Bounds::new(center + offset, y, width, height)
    }

    pub fn bookmark_button(self) -> Bounds {
        Bounds::new(self.panel.x + 16.0, self.panel.bottom() - 33.0, 28.0, 24.0)
    }

    pub fn reset_button(self) -> Bounds {
        Bounds::new(self.panel.right() - 84.0, self.panel.y + 4.0, 54.0, 22.0)
    }

    pub fn hit_button(self, point: Point) -> Option<Transport> {
        TRANSPORT_CONTROLS
            .into_iter()
            .find(|&control| self.button(control).contains(point))
    }

    pub fn hit_track(self, point: Point) -> bool {
        Bounds::new(
            self.track.x - 6.0,
            self.track.y - 6.0,
            self.track.w + 12.0,
            self.track.h + 12.0,
        )
        .contains(point)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TimelineResetLayout {
    pub bounds: Bounds,
    pub cancel: Bounds,
    pub confirm: Bounds,
}

impl TimelineResetLayout {
    pub fn new(anchor: Bounds, surface: SurfaceSize) -> Self {
        let screen_width = f64::from(surface.width);
        let screen_height = f64::from(surface.height);
        let width = 318.0_f64.min((screen_width - 12.0).max(1.0));
        let height = 115.0;
        let x = (anchor.right() - width).clamp(6.0, (screen_width - width - 6.0).max(6.0));
        let below = anchor.bottom() + 4.0;
        let y = if below + height + 6.0 <= screen_height {
            below
        } else {
            anchor.y - height - 4.0
        };
        let y = y.clamp(6.0, (screen_height - height - 6.0).max(6.0));
        Self {
            bounds: Bounds::new(x, y, width, height),
            cancel: Bounds::new(x + width - 164.0, y + 78.0, 72.0, 26.0),
            confirm: Bounds::new(x + width - 83.0, y + 78.0, 72.0, 26.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Staging;

    fn project(staging: &Staging, name: &str) -> Project {
        Project {
            manifest: staging.0.join(name).join("Cargo.toml"),
            package: name.into(),
            binary: name.into(),
        }
    }

    #[test]
    fn windows_projects_and_independent_timeline_links_survive_reopen() {
        let staging = Staging::new().unwrap();
        let path = staging.0.join("board.json");
        let terrarium = project(&staging, "terrarium");
        let garden = project(&staging, "garden");
        let view = viewport(SurfaceSize::new(1120, 850));
        let mut board = Board::with_example(terrarium.clone(), true);
        let first_game = board.windows().next().unwrap().id;
        let second_game = board.add(Panel::Game, view);
        let empty_game = board.add(Panel::Game, view);
        let second_timeline = board.add(Panel::Timeline, view);
        let empty_timeline = board.add(Panel::Timeline, view);
        board.attach_project(second_game, garden.clone()).unwrap();
        assert!(board.attach_project(empty_game, garden).is_err());
        board.attach_target(second_timeline, Some(second_game));
        board.dock(second_timeline, first_game);
        board.set_enabled(first_game, false);
        board.set_game_zoom(second_game, 2.0);
        board.save(&path).unwrap();

        let (mut saved, _) = Board::load(&path, &terrarium, true).unwrap().unwrap();
        assert_eq!(saved, board);
        assert!(saved.window(empty_game).unwrap().project().is_none());
        assert_eq!(saved.window(empty_timeline).unwrap().attached_game(), None);
        assert_eq!(
            saved.window(second_timeline).unwrap().attached_game(),
            Some(second_game)
        );
        assert_eq!(
            saved.dock_group(first_game),
            Some((first_game, second_timeline))
        );

        saved.close(second_timeline);
        assert!(saved.window(second_game).unwrap().project().is_some());
        assert!(saved.window(first_game).unwrap().project().is_some());
        let replacement = saved.add(Panel::Timeline, view);
        assert!(replacement > empty_timeline);
        saved.attach_target(replacement, Some(second_game));
        saved.close(second_game);
        assert_eq!(saved.window(replacement).unwrap().attached_game(), None);
    }

    #[test]
    fn duplicated_games_and_docked_panels_reopen_with_independent_identity() {
        let staging = Staging::new().unwrap();
        let project = project(&staging, "example");
        let path = staging.0.join("board.json");
        let view = viewport(SurfaceSize::new(1100, 800));
        let mut board = Board::default();
        let source = board.add(Panel::Game, view);
        board.attach_project(source, project.clone()).unwrap();
        board.set_game_zoom(source, 1.5);
        let timeline = board.add(Panel::Timeline, view);
        board.attach_target(timeline, Some(source));
        board.dock(timeline, source);
        let timeline_copy = board.duplicate(board.window(timeline).unwrap().clone(), None);
        assert!(board.dock_group(timeline_copy).is_none());
        assert_eq!(
            board.window(timeline_copy).unwrap().attached_game(),
            Some(source)
        );
        let game_copy = board.duplicate(board.window(source).unwrap().clone(), Some("ab-0".into()));
        assert_eq!(board.window(game_copy).unwrap().game_zoom(), 1.5);
        assert_ne!(
            board.window_label(board.window(source).unwrap()),
            board.window_label(board.window(game_copy).unwrap())
        );
        board.attach_target(timeline_copy, Some(game_copy));
        board.save(&path).unwrap();

        let (saved, _) = Board::load(&path, &project, true).unwrap().unwrap();
        assert_eq!(saved.window(source).unwrap().instance(), None);
        assert_eq!(saved.window(game_copy).unwrap().instance(), Some("ab-0"));
        assert_eq!(
            saved.window(timeline).unwrap().attached_game(),
            Some(source)
        );
        assert_eq!(
            saved.window(timeline_copy).unwrap().attached_game(),
            Some(game_copy)
        );
    }

    #[test]
    fn docking_and_resizing_do_not_rebind_timeline_or_change_its_height() {
        let view = viewport(SurfaceSize::new(1000, 800));
        let mut board = Board::default();
        let first = board.add(Panel::Game, view);
        let second = board.add(Panel::Game, view);
        let timeline = board.add(Panel::Timeline, view);
        let initial_height = board.window(timeline).unwrap().bounds.h;
        board.attach_target(timeline, Some(second));
        board.dock(timeline, first);
        let resized = Bounds::new(50.0, 65.0, 738.0, 440.0);
        board.set_bounds(first, resized);
        assert_eq!(
            board.window(timeline).unwrap().bounds,
            Bounds::new(resized.x, resized.bottom(), resized.w, initial_height)
        );
        let game_pixels = board.window(first).unwrap().game_display(2.0).render_size;
        board.zoom_at(Point::new(100.0, 100.0), 0.48);
        assert_eq!(
            board.window(first).unwrap().game_display(2.0).render_size,
            game_pixels
        );
        board.set_game_zoom(first, 2.0);
        let zoomed = board.window(first).unwrap().game_display(2.0).render_size;
        assert_eq!(zoomed.width * 2, game_pixels.width);
        assert_eq!(
            board.window(timeline).unwrap().attached_game(),
            Some(second)
        );
        board.undock(timeline);
        assert_eq!(
            board.window(timeline).unwrap().attached_game(),
            Some(second)
        );
        assert!(board.dock_group(timeline).is_none());
    }

    #[test]
    fn legacy_board_restores_tetree_layout_and_disable_state() {
        let staging = Staging::new().unwrap();
        let path = staging.0.join("board.json");
        let tetree = project(&staging, "tetree");
        let old = serde_json::json!({
            "game": {"x":40.0, "y":50.0, "w":512.0, "h":360.0},
            "timeline": {"x":40.0, "y":410.0, "w":512.0, "h":101.0},
            "pan": {"x":12.0,"y":9.0}, "zoom":0.67, "game_zoom":2.0,
            "timeline_docked":true, "show_profiler":true
        });
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let (board, legacy) = Board::load(&path, &tetree, false).unwrap().unwrap();
        assert!(legacy);
        let game = board
            .windows()
            .find(|window| window.kind() == Panel::Game)
            .unwrap();
        let timeline = board
            .windows()
            .find(|window| window.kind() == Panel::Timeline)
            .unwrap();
        assert!(
            matches!(&game.content, Content::Game { project: Some(project), enabled: false, zoom, .. } if project == &tetree && *zoom == 2.0)
        );
        assert_eq!(board.dock_group(timeline.id), Some((game.id, timeline.id)));
        assert_eq!(timeline.attached_game(), Some(game.id));
        assert_eq!(timeline.bounds.h, 101.0);
        assert_eq!(board.zoom, 0.67);
    }

    #[test]
    fn compact_timeline_keeps_scrub_separate_from_transport_controls() {
        let panel = Bounds::new(0.0, 0.0, 260.0, MIN_TIMELINE_HEIGHT);
        let layout = TimelineLayout::new(panel);
        for control in TRANSPORT_CONTROLS {
            let button = layout.button(control);
            assert!(button.y > layout.track.bottom());
            assert_eq!(
                layout.hit_button(Point::new(button.x + 1.0, button.y + 1.0)),
                Some(control)
            );
        }
    }

    #[test]
    fn fullscreen_uses_display_pixels_and_keeps_the_game_zoom() {
        let mut board = Board::default();
        let window = board.add(Panel::Game, viewport(SurfaceSize::new(1200, 800)));
        board.set_bounds(window, Bounds::new(0.0, 0.0, 260.0, 220.0));
        let game = board.window(window).unwrap();
        let header = GameHeader::new(game.bounds);
        assert!(
            header
                .fullscreen_button
                .intersect(header.lifecycle_button)
                .is_none()
        );
        assert!(
            header
                .fullscreen_button
                .intersect(game.close_button())
                .is_none()
        );
        assert_eq!(
            game.fullscreen_display(SurfaceSize::new(1920, 1080), 1.0)
                .render_size,
            SurfaceSize::new(1920, 1080)
        );
        let standard = game.game_display(1.0);
        let retina = game.game_display(2.0);
        assert_eq!(retina.logical_size, standard.logical_size);
        assert_eq!(
            retina.render_size,
            SurfaceSize::new(
                standard.render_size.width * 2,
                standard.render_size.height * 2
            )
        );
        assert_eq!(
            game.fullscreen_display(SurfaceSize::new(5120, 2880), 2.0)
                .render_size,
            SurfaceSize::new(4096, 2304)
        );
        board.set_game_zoom(window, 2.0);
        let zoom = board
            .window(window)
            .unwrap()
            .fullscreen_display(SurfaceSize::new(3840, 2160), 2.0);
        assert_eq!(zoom.render_size, SurfaceSize::new(1920, 1080));
        assert_eq!(zoom.logical_size, SurfaceSize::new(960, 540));
    }

    #[test]
    fn owned_pickers_follow_their_games_and_are_transient() {
        let temp = Staging::new().unwrap();
        let mut board = Board::default();
        let view = viewport(SurfaceSize::new(1200, 820));
        let first = board.add(Panel::Game, view);
        let second = board.add(Panel::Game, view);
        let browser = board.add(Panel::Explorer, view);
        board.set_bounds(first, Bounds::new(200.0, 100.0, 730.0, 560.0));
        board.set_bounds(second, Bounds::new(700.0, 160.0, 278.0, 250.0));
        board.set_explorer_directory(browser, temp.0.clone());
        board.note_project(temp.0.clone());
        board.pan = Point::new(10.0, 20.0);
        board.zoom = 0.7;

        let first_picker = board.open_picker(first).unwrap();
        let second_picker = board.open_picker(second).unwrap();
        assert_eq!(board.open_picker(first), Some(first_picker));
        assert_eq!(board.pan, Point::new(10.0, 20.0));
        assert_eq!(board.zoom, 0.7);
        let small = board.window(second).unwrap().bounds;
        let picker = board.window(second_picker).unwrap().bounds;
        assert!(picker.x > small.x && picker.y >= small.y + TITLE_HEIGHT);
        assert!(picker.w > small.w && picker.h > small.h);

        let timeline = board.add(Panel::Timeline, view);
        board.dock(timeline, second);
        let docked = board.window(timeline).unwrap().bounds;
        let other_picker = board.window(first_picker).unwrap().bounds;
        let delta = Point::new(53.0, -26.0);
        board.set_bounds(second, small.translated(delta));
        assert_eq!(
            board.window(second_picker).unwrap().bounds,
            picker.translated(delta)
        );
        assert_eq!(
            board.window(timeline).unwrap().bounds,
            docked.translated(delta)
        );
        assert_eq!(board.window(first_picker).unwrap().bounds, other_picker);

        board.raise(browser);
        assert_eq!(board.windows().next_back().unwrap().id, browser);
        board.raise(first_picker);
        let mut order = board.windows().rev();
        assert_eq!(order.next().unwrap().id, first_picker);
        assert_eq!(order.next().unwrap().id, first);
        drop(order);
        board.close(first_picker);
        assert!(board.window(first).is_some());
        assert_eq!(board.picker(second), Some(second_picker));

        let path = temp.0.join("board.json");
        board.save(&path).unwrap();
        let (saved, _) = Board::load(&path, &project(&temp, "example"), true)
            .unwrap()
            .unwrap();
        assert!(saved.window(second_picker).is_none());
        assert!(
            matches!(&saved.window(browser).unwrap().content, Content::Explorer { directory: Some(directory) } if directory == &temp.0)
        );
        assert_eq!(saved.recent_projects(), &[temp.0.clone()]);
    }

    #[test]
    fn atlas_keeps_its_file_with_no_game_windows() {
        let staging = Staging::new().unwrap();
        let path = staging.0.join("board.json");
        let mut board = Board::default();
        let atlas = board.add(Panel::TileAtlas, viewport(SurfaceSize::new(1100, 800)));
        let file = staging.0.join("terrain.atlas.json");
        board.set_asset_source(atlas, file.clone());
        let picker = board.open_picker(atlas).unwrap();
        assert_eq!(board.window(picker).unwrap().owner, Some(atlas));
        board.save(&path).unwrap();
        let (restored, _) = Board::load(&path, &project(&staging, "example"), true)
            .unwrap()
            .unwrap();
        assert!(restored.picker(atlas).is_none());
        assert_eq!(
            restored.window(atlas).unwrap().asset_source(),
            Some(file.as_path())
        );
    }

    #[test]
    fn profiler_and_timeline_targets_are_stable_and_clear_independently() {
        let staging = Staging::new().unwrap();
        let path = staging.0.join("board.json");
        let mut board = Board::default();
        let view = viewport(SurfaceSize::new(1100, 800));
        let first = board.add(Panel::Game, view);
        let second = board.add(Panel::Game, view);
        let profiler = board.add(Panel::Profiler, view);
        let timeline = board.add(Panel::Timeline, view);
        assert_eq!(board.window(profiler).unwrap().attached_game(), None);
        assert_eq!(board.window(timeline).unwrap().attached_game(), None);
        board.attach_target(profiler, Some(first));
        board.attach_target(timeline, Some(second));
        board.attach_target(profiler, Some(timeline)); // An incompatible drop preserves the link.
        board.set_enabled(first, false);
        board.raise(second);
        board.save(&path).unwrap();
        let (mut reopened, _) = Board::load(&path, &project(&staging, "example"), true)
            .unwrap()
            .unwrap();
        assert_eq!(
            reopened.window(profiler).unwrap().attached_game(),
            Some(first)
        );
        assert_eq!(
            reopened.window(timeline).unwrap().attached_game(),
            Some(second)
        );
        reopened.close(first);
        assert_eq!(reopened.window(profiler).unwrap().attached_game(), None);
        assert_eq!(
            reopened.window(timeline).unwrap().attached_game(),
            Some(second)
        );
    }

    #[test]
    fn game_target_hit_respects_z_order_and_counts_empty_or_disabled_games() {
        let mut board = Board::default();
        let view = viewport(SurfaceSize::new(1000, 800));
        let game = board.add(Panel::Game, view);
        let other = board.add(Panel::Explorer, view);
        board.set_bounds(game, Bounds::new(80.0, 90.0, 440.0, 380.0));
        board.set_enabled(game, false);
        board.set_bounds(other, Bounds::new(250.0, 190.0, 530.0, 350.0));
        assert_eq!(board.game_at(Point::new(160.0, 106.0)), Some(game));
        assert_eq!(board.game_at(Point::new(328.0, 270.0)), None);
        board.raise(game);
        assert_eq!(board.game_at(Point::new(328.0, 270.0)), Some(game));
    }
}
