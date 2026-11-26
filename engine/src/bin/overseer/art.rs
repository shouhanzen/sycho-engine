use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use engine::{
    graphics::Image,
    surface::SurfaceSize,
    tiles::{Cell, TileGrid},
    tilesets::{AssetKey, AssetWatch, Atlas, AtlasRef, Tilesets, relative_atlas},
};

use super::board::{Board, Bounds, Panel, Point, TITLE_HEIGHT, WindowId};
use super::level;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Pick {
    pub definition: PathBuf,
    pub tile: Cell,
}

#[derive(Clone, PartialEq)]
pub(super) struct AssetView {
    pub source: Option<PathBuf>,
    pub camera: Camera,
    pub atlas: Option<Arc<Atlas>>,
    pub image: Option<Arc<Image>>,
    pub error: Option<String>,
    key: Option<AssetKey>,
}

impl AssetView {
    pub fn tile_at(&self, view: Bounds, point: Point) -> Option<Pick> {
        let atlas = self.atlas.as_ref()?;
        let size = atlas.image.size();
        let image = self.camera.image(view, size);
        if !view.contains(point) || !image.contains(point) {
            return None;
        }
        let x = (point.x - image.x) / image.w * f64::from(size.width);
        let y = (point.y - image.y) / image.h * f64::from(size.height);
        let tile = Cell::new(
            (x / f64::from(atlas.tile_size.width)) as u16,
            (y / f64::from(atlas.tile_size.height)) as u16,
        );
        atlas.region(tile)?;
        Some(Pick {
            definition: self.source.clone()?,
            tile,
        })
    }

    fn update(&mut self, watch: &AssetWatch) {
        let value = self.key.as_ref().and_then(|key| watch.get(key));
        self.atlas = value.and_then(|value| value.atlas.clone());
        self.image = value
            .and_then(|value| value.image.clone())
            .or_else(|| self.atlas.as_ref().map(|atlas| Arc::clone(&atlas.image)));
        self.error = value.and_then(|value| value.error.clone());
    }
}

struct LevelSources {
    directory: PathBuf,
    grid: Arc<TileGrid>,
    sources: BTreeSet<AtlasRef>,
    picked: Option<AtlasRef>,
}

#[derive(Default)]
pub(super) struct Assets {
    pub watch: AssetWatch,
    pub panels: BTreeMap<WindowId, AssetView>,
    pub level_tilesets: BTreeMap<WindowId, Arc<Tilesets>>,
    levels: BTreeMap<WindowId, LevelSources>,
}

impl Assets {
    pub fn sync(
        &mut self,
        board: &Board,
        editors: &BTreeMap<WindowId, level::Editor>,
        pick: Option<&Pick>,
    ) {
        self.panels.retain(|&id, _| board.window(id).is_some());
        for panel in board.windows() {
            let key = match panel.kind() {
                Panel::TileAtlas => panel
                    .asset_source()
                    .map(|path| AssetKey::Atlas(path.to_path_buf())),
                Panel::TextureInspector => panel
                    .asset_source()
                    .map(|path| AssetKey::Texture(path.to_path_buf())),
                _ => continue,
            };
            if self
                .panels
                .get(&panel.id)
                .is_some_and(|view| view.key == key)
            {
                continue;
            }
            let mut view = AssetView {
                source: panel.asset_source().map(Path::to_path_buf),
                camera: Camera::default(),
                atlas: None,
                image: None,
                error: None,
                key,
            };
            view.update(&self.watch);
            self.panels.insert(panel.id, view);
        }

        let before = self.levels.len();
        self.levels.retain(|id, _| {
            editors
                .get(id)
                .is_some_and(|editor| editor.assets().is_some())
        });
        let mut changed = self.levels.len() != before;
        for (&id, editor) in editors {
            let Some((directory, grid)) = editor.assets() else {
                continue;
            };
            let picked = pick.and_then(|pick| relative_atlas(directory, &pick.definition));
            let level = self.levels.entry(id).or_insert_with(|| {
                changed = true;
                LevelSources {
                    directory: directory.into(),
                    grid: Arc::clone(grid),
                    sources: grid.atlas_sources(),
                    picked: picked.clone(),
                }
            });
            if level.directory != directory || !Arc::ptr_eq(&level.grid, grid) {
                level.directory = directory.into();
                level.grid = Arc::clone(grid);
                level.sources = grid.atlas_sources();
                changed = true;
            }
            if level.picked != picked {
                level.picked = picked;
                changed = true;
            }
        }
        let mut keys: BTreeSet<_> = self
            .panels
            .values()
            .filter_map(|view| view.key.clone())
            .collect();
        if let Some(pick) = pick {
            keys.insert(AssetKey::Atlas(pick.definition.clone()));
        }
        for level in self.levels.values() {
            keys.extend(
                level
                    .sources
                    .iter()
                    .map(|source| source.key(&level.directory)),
            );
        }
        self.watch.watch(keys);
        if changed {
            self.update_levels();
        }
    }

    pub fn poll(&mut self) -> bool {
        if !self.watch.poll() {
            return false;
        }
        for view in self.panels.values_mut() {
            view.update(&self.watch);
        }
        self.update_levels();
        true
    }

    fn update_levels(&mut self) {
        self.level_tilesets
            .retain(|id, _| self.levels.contains_key(id));
        for (&id, level) in &self.levels {
            let mut sources = level.sources.clone();
            sources.extend(level.picked.clone());
            let tilesets = Tilesets::from_watch(&level.directory, &sources, &self.watch);
            if self
                .level_tilesets
                .get(&id)
                .is_none_or(|old| old.errors != tilesets.errors)
            {
                for error in &tilesets.errors {
                    eprintln!("tile asset: {error}");
                }
            }
            self.level_tilesets.insert(id, Arc::new(tilesets));
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Camera {
    pub pan: Point,
    zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pan: Point::new(0.0, 0.0),
            zoom: 1.0,
        }
    }
}

impl Camera {
    pub fn image(self, view: Bounds, size: SurfaceSize) -> Bounds {
        let fit = (view.w / f64::from(size.width)).min(view.h / f64::from(size.height));
        let (w, h) = (
            f64::from(size.width) * fit * self.zoom,
            f64::from(size.height) * fit * self.zoom,
        );
        Bounds::new(
            view.x + (view.w - w) / 2.0 + self.pan.x,
            view.y + (view.h - h) / 2.0 + self.pan.y,
            w,
            h,
        )
    }

    pub fn zoom_at(&mut self, view: Bounds, at: Point, delta: f32) {
        let before = self.zoom;
        self.zoom = (self.zoom * 1.18_f64.powf(f64::from(delta))).clamp(0.25, 64.0);
        let center = Point::new(view.x + view.w / 2.0, view.y + view.h / 2.0);
        let ratio = self.zoom / before;
        self.pan.x = at.x - center.x + (center.x + self.pan.x - at.x) * ratio;
        self.pan.y = at.y - center.y + (center.y + self.pan.y - at.y) * ratio;
    }

    pub fn scale(self, view: Bounds, size: SurfaceSize) -> f64 {
        self.image(view, size).w / f64::from(size.width)
    }
}

pub(super) struct Layout {
    pub open: Bounds,
    pub action: Bounds,
    pub status: Bounds,
    pub image: Bounds,
}

impl Layout {
    pub fn new(bounds: Bounds) -> Self {
        let y = bounds.y + TITLE_HEIGHT + 8.0;
        let h = (bounds.bottom() - y - 42.0).max(1.0);
        Self {
            image: Bounds::new(bounds.x + 8.0, y, bounds.w - 16.0, h),
            open: Bounds::new(bounds.x + 8.0, bounds.bottom() - 34.0, 66.0, 24.0),
            action: Bounds::new(bounds.right() - 82.0, bounds.bottom() - 34.0, 66.0, 24.0),
            status: Bounds::new(
                bounds.x + 82.0,
                bounds.bottom() - 34.0,
                bounds.w - 170.0,
                24.0,
            ),
        }
    }
}
