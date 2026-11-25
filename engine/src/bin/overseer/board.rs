use std::fs;
use std::io;
use std::path::Path;

use engine::embedded::{DEFAULT_PREVIEW_SIZE, MAX_PREVIEW_DIMENSION};
use engine::surface::SurfaceSize;
use serde::{Deserialize, Serialize};

pub const TITLE_HEIGHT: f64 = 30.0;
const MIN_ZOOM: f64 = 0.03;
const MAX_ZOOM: f64 = 4.0;
const MIN_GAME_ZOOM: f64 = 0.5;
const MAX_GAME_ZOOM: f64 = 2.0;
const EDGE: f64 = 7.0;
const GRIP: f64 = 11.0;
const DOCK_DISTANCE: f64 = 16.0;
const MIN_TIMELINE_HEIGHT: f64 = 90.0;
const DEFAULT_TIMELINE_HEIGHT: f64 = 96.0;
const PREVIOUS_DEFAULT_TIMELINE_HEIGHT: f64 = 152.0;
const BOARD_LAYOUT_VERSION: u32 = 1;

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
}

impl Panel {
    fn minimum(self) -> Point {
        match self {
            Self::Game => Point::new(260.0, 200.0),
            Self::Timeline => Point::new(260.0, MIN_TIMELINE_HEIGHT),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Board {
    pub game: Bounds,
    pub timeline: Bounds,
    pub front: Panel,
    pub pan: Point,
    pub zoom: f64,
    #[serde(default = "default_game_zoom")]
    pub game_zoom: f64,
    #[serde(default)]
    pub timeline_docked: bool,
}

#[derive(Serialize, Deserialize)]
struct SavedBoard {
    #[serde(default)]
    layout_version: u32,
    #[serde(flatten)]
    board: Board,
}

fn default_game_zoom() -> f64 {
    1.0
}

impl Default for Board {
    fn default() -> Self {
        let width = f64::from(DEFAULT_PREVIEW_SIZE.width) + 2.0;
        let height = f64::from(DEFAULT_PREVIEW_SIZE.height) + TITLE_HEIGHT + 1.0;
        Self {
            game: Bounds::new(40.0, 36.0, width, height),
            timeline: Bounds::new(40.0, 36.0 + height + 22.0, width, DEFAULT_TIMELINE_HEIGHT),
            front: Panel::Game,
            pan: Point::default(),
            zoom: 1.0,
            game_zoom: default_game_zoom(),
            timeline_docked: false,
        }
    }
}

impl Board {
    pub fn bounds(self, panel: Panel) -> Bounds {
        match panel {
            Panel::Game => self.game,
            Panel::Timeline => self.timeline,
        }
    }

    pub fn set_bounds(&mut self, panel: Panel, bounds: Bounds) {
        match panel {
            Panel::Game => self.game = bounds,
            Panel::Timeline => self.timeline = bounds,
        }
        self.align_docked_timeline();
    }

    fn align_docked_timeline(&mut self) {
        if self.timeline_docked {
            self.timeline = Bounds::new(
                self.game.x,
                self.game.bottom(),
                self.game.w,
                self.timeline.h,
            );
        }
    }

    pub fn can_dock_timeline(self) -> bool {
        let gap = (self.timeline.y - self.game.bottom()).abs() * self.zoom;
        let overlap =
            self.game.right().min(self.timeline.right()) - self.game.x.max(self.timeline.x);
        !self.timeline_docked
            && gap <= DOCK_DISTANCE
            && overlap >= self.game.w.min(self.timeline.w) / 3.0
    }

    pub fn dock_timeline(&mut self) {
        self.timeline_docked = true;
        self.align_docked_timeline();
    }

    pub fn dock_bounds(self) -> Option<Bounds> {
        self.timeline_docked.then(|| {
            Bounds::new(
                self.game.x,
                self.game.y,
                self.game.w,
                self.game.h + self.timeline.h,
            )
        })
    }

    pub fn order(self) -> [Panel; 2] {
        match self.front {
            Panel::Game => [Panel::Timeline, Panel::Game],
            Panel::Timeline => [Panel::Game, Panel::Timeline],
        }
    }

    pub fn panel_at(self, world: Point) -> Option<Panel> {
        self.order()
            .into_iter()
            .rev()
            .find(|&panel| self.bounds(panel).contains(world))
    }

    pub fn game_body(self) -> Bounds {
        Bounds::new(
            self.game.x + 1.0,
            self.game.y + TITLE_HEIGHT,
            self.game.w - 2.0,
            self.game.h - TITLE_HEIGHT - 1.0,
        )
    }

    pub fn game_header(self) -> GameHeader {
        GameHeader::new(self.game)
    }

    pub fn game_size(self, dpi: f64) -> SurfaceSize {
        let body = self.game_body();
        let width = (body.w * dpi).round().max(1.0);
        let height = (body.h * dpi).round().max(1.0);
        let limit = f64::from(MAX_PREVIEW_DIMENSION);
        let scale = self.game_zoom.recip().min(limit / width.max(height));
        SurfaceSize::new(
            (width * scale).round().clamp(1.0, limit) as u32,
            (height * scale).round().clamp(1.0, limit) as u32,
        )
    }

    pub fn world_at(self, screen: Point) -> Point {
        Point::new(
            (screen.x - self.pan.x) / self.zoom,
            (screen.y - self.pan.y) / self.zoom,
        )
    }

    pub fn screen_at(self, world: Point) -> Point {
        Point::new(
            world.x * self.zoom + self.pan.x,
            world.y * self.zoom + self.pan.y,
        )
    }

    pub fn project(self, world: Bounds) -> Bounds {
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

    pub fn fit(&mut self, view: Bounds) {
        let x = self.game.x.min(self.timeline.x);
        let y = self.game.y.min(self.timeline.y);
        let w = self.game.right().max(self.timeline.right()) - x;
        let h = self.game.bottom().max(self.timeline.bottom()) - y;
        self.zoom = ((view.w - 48.0) / w)
            .min((view.h - 48.0) / h)
            .clamp(MIN_ZOOM, 1.0);
        self.pan = Point::new(
            view.x + (view.w - w * self.zoom) / 2.0 - x * self.zoom,
            view.y + (view.h - h * self.zoom) / 2.0 - y * self.zoom,
        );
    }

    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        let SavedBoard {
            layout_version,
            mut board,
        } = serde_json::from_slice(&bytes)?;
        if layout_version == 0 && board.timeline.h == PREVIOUS_DEFAULT_TIMELINE_HEIGHT {
            board.timeline.h = DEFAULT_TIMELINE_HEIGHT;
        }
        board.align_docked_timeline();
        if !(MIN_ZOOM..=MAX_ZOOM).contains(&board.zoom)
            || !(MIN_GAME_ZOOM..=MAX_GAME_ZOOM).contains(&board.game_zoom)
            || board.order().into_iter().any(|panel| {
                let bounds = board.bounds(panel);
                let min = panel.minimum();
                bounds.w < min.x
                    || bounds.h < min.y
                    || !bounds.right().is_finite()
                    || !bounds.bottom().is_finite()
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid editor board bounds",
            ));
        }
        Ok(Some(board))
    }

    pub fn save(self, path: &Path) -> io::Result<()> {
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        let saved = SavedBoard {
            layout_version: BOARD_LAYOUT_VERSION,
            board: self,
        };
        fs::write(&temp, serde_json::to_vec_pretty(&saved)?)?;
        fs::rename(temp, path)
    }
}

#[derive(Clone, Copy)]
pub struct Edges {
    pub horizontal: i8,
    pub vertical: i8,
}

impl Edges {
    pub fn within(board: Board, panel: Panel, point: Point) -> Option<Self> {
        let bounds = board.dock_bounds().unwrap_or_else(|| board.bounds(panel));
        if bounds.grip().contains(point) {
            return Some(Self {
                horizontal: 1,
                vertical: 1,
            });
        }
        let margin = (EDGE / board.zoom).min(bounds.w.min(bounds.h) / 5.0);
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
    pub resolution: Bounds,
}

impl GameHeader {
    fn new(panel: Bounds) -> Self {
        let resolution = Bounds::new(
            panel.right() - 110.0,
            panel.y + 2.0,
            104.0,
            TITLE_HEIGHT - 4.0,
        );
        let width = (panel.w * 0.26).clamp(60.0, 116.0);
        Self {
            zoom_track: Bounds::new(resolution.x - width - 18.0, panel.y + 14.0, width, 3.0),
            resolution,
        }
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
        let takeover_width = (self.panel.w - 154.0).min(104.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_default_timeline_compacts_but_saved_custom_heights_are_restored() {
        let path =
            std::env::temp_dir().join(format!("sycho-timeline-layout-{}.json", std::process::id()));
        let mut legacy = Board::default();
        legacy.timeline.h = PREVIOUS_DEFAULT_TIMELINE_HEIGHT;
        legacy.dock_timeline();
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let upgraded = Board::load(&path).unwrap().unwrap();

        let mut custom = legacy;
        custom.timeline.h += 20.0;
        fs::write(&path, serde_json::to_vec(&custom).unwrap()).unwrap();
        let restored_custom = Board::load(&path).unwrap().unwrap();

        legacy.save(&path).unwrap();
        let restored_new = Board::load(&path).unwrap().unwrap();
        fs::remove_file(path).unwrap();

        assert_eq!(upgraded.game, legacy.game);
        assert!(upgraded.timeline_docked);
        assert_eq!(upgraded.timeline.y, legacy.game.bottom());
        assert_eq!(upgraded.timeline.h, DEFAULT_TIMELINE_HEIGHT);
        assert_eq!(restored_custom, custom);
        assert_eq!(restored_new, legacy);
    }

    #[test]
    fn compact_timeline_keeps_scrubbing_and_controls_apart() {
        let panel = Bounds::new(40.0, 60.0, 260.0, MIN_TIMELINE_HEIGHT);
        let layout = TimelineLayout::new(panel);
        assert!(layout.track.y > panel.y + TITLE_HEIGHT);
        for control in TRANSPORT_CONTROLS {
            let button = layout.button(control);
            assert!(button.bottom() < panel.bottom());
            assert!(!layout.hit_track(Point::new(button.x + button.w / 2.0, button.y)));
        }
    }
}
