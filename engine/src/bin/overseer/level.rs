use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use engine::tiles::{
    Cell, GridSize, LevelBase, LevelEdits, MAX_GRID_CELLS, TileEdit, TileGrid, TilePalette,
    TilePreview, level_directory,
};

use engine::tilesets::{TileRef, relative_atlas};

use super::art::Pick;

use crate::project::Project;

use super::board::{Bounds, Edges, PanelWindow, Point, WindowId};

const CELL_SIZE: f64 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Brush {
    Paint,
    Erase,
}

#[derive(Clone, PartialEq)]
pub(super) struct View {
    pub grid: Option<Arc<TileGrid>>,
    pub palette: Option<TilePalette>,
    pub brush: Brush,
    pub sprite: Option<TileRef>,
    pub draft: Arc<BTreeSet<Cell>>,
    pub error: Option<String>,
    pub selected: bool,
}

struct Document {
    directory: PathBuf,
    base: LevelBase,
    edits: LevelEdits,
    grid: Arc<TileGrid>,
}

pub(super) struct Editor {
    target: Option<(WindowId, TilePreview)>,
    document: Option<Document>,
    brush: Brush,
    draft: Arc<BTreeSet<Cell>>,
    resize: Option<Arc<TileGrid>>,
    last: Option<Cell>,
    error: Option<String>,
    selected: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            target: None,
            document: None,
            brush: Brush::Paint,
            draft: Arc::default(),
            resize: None,
            last: None,
            error: None,
            selected: false,
        }
    }
}

impl Editor {
    pub fn sync(&mut self, target: Option<(WindowId, &Project, TilePreview)>) -> Option<bool> {
        let key = target.map(|(id, _, stamp)| (id, stamp));
        if key == self.target {
            return None;
        }
        let same_base = self
            .target
            .zip(key)
            .is_some_and(|((before, old), (after, new))| before == after && old.base == new.base);
        if !same_base {
            self.cancel();
        }
        if self.target.map(|(id, _)| id) != key.map(|(id, _)| id) {
            self.brush = Brush::Paint;
        }
        self.target = key;
        let cached = self
            .document
            .take()
            .filter(|_| same_base)
            .map(|doc| doc.base);
        let loaded = target
            .map(|(_, project, stamp)| {
                let base = match cached {
                    Some(base) => base,
                    None => LevelBase::read_preview(project.root(), &project.binary, stamp)?,
                };
                let edits = base.read_edits(project.root())?;
                let grid = Arc::new(edits.grid(&base.grid));
                Ok::<_, std::io::Error>(Document {
                    directory: level_directory(project.root()),
                    base,
                    edits,
                    grid,
                })
            })
            .transpose();
        match loaded {
            Ok(document) => {
                self.document = document;
                self.error = None;
            }
            Err(err) => {
                self.document = None;
                self.error = Some(err.to_string());
            }
        }
        Some(!same_base)
    }

    pub fn view(&self, picked: Option<&Pick>) -> View {
        View {
            grid: self
                .resize
                .clone()
                .or_else(|| self.document.as_ref().map(|doc| Arc::clone(&doc.grid))),
            palette: self.document.as_ref().map(|doc| doc.base.palette),
            brush: self.brush,
            sprite: self.sprite(picked),
            draft: self.draft.clone(),
            error: self.error.clone(),
            selected: self.selected,
        }
    }

    pub fn select_brush(&mut self, brush: Brush) {
        self.brush = brush;
    }

    pub fn assets(&self) -> Option<(&Path, &Arc<TileGrid>)> {
        let doc = self.document.as_ref()?;
        Some((&doc.directory, self.resize.as_ref().unwrap_or(&doc.grid)))
    }

    fn sprite(&self, picked: Option<&Pick>) -> Option<TileRef> {
        let pick = picked?;
        let source = relative_atlas(&self.document.as_ref()?.directory, &pick.definition)?;
        Some(TileRef {
            source,
            tile: pick.tile,
        })
    }

    pub fn size(&self) -> Option<GridSize> {
        Some(self.document.as_ref()?.grid.size())
    }

    pub fn layout(&self, panel: &PanelWindow) -> Layout {
        Layout::new(
            panel,
            self.resize
                .as_ref()
                .map(|grid| grid.size())
                .or_else(|| self.size()),
        )
    }

    pub fn set_selected(&mut self, selected: bool) -> bool {
        std::mem::replace(&mut self.selected, selected)
    }

    pub fn selected(&self) -> bool {
        self.selected
    }

    pub fn hits_grid(&self, panel: &PanelWindow, point: Point, zoom: f64) -> bool {
        let Some(doc) = &self.document else {
            return false;
        };
        let layout = self.layout(panel);
        (self.selected && layout.resize_edges(point, zoom).is_some())
            || layout
                .cell_at(point)
                .is_some_and(|cell| self.selected || doc.grid.solid(cell))
    }

    pub fn paint_to(
        &mut self,
        cell: Option<Cell>,
        picked: Option<&Pick>,
        exposed: impl Fn(Cell) -> bool,
    ) {
        if let Some(end) = cell
            && let Some(doc) = &self.document
        {
            let start = self.last.unwrap_or(end);
            let solid = self.brush == Brush::Paint;
            let sprite = solid.then(|| self.sprite(picked)).flatten();
            if solid && picked.is_some() && sprite.is_none() {
                self.error = Some("Atlas and level must share a filesystem root".into());
                return;
            }
            let draft = Arc::make_mut(&mut self.draft);
            trace(start, end, |cell| {
                if (doc.grid.solid(cell) != solid || doc.grid.sprite(cell) != sprite.as_ref())
                    && exposed(cell)
                {
                    draft.insert(cell);
                }
            });
        }
        self.last = cell;
    }

    pub fn resize_to(&mut self, size: GridSize) {
        if let Some(doc) = &self.document {
            if size == doc.grid.size() {
                self.resize = None;
            } else if self.resize.as_ref().is_none_or(|grid| grid.size() != size) {
                self.resize = Some(Arc::new(doc.edits.grid_at(&doc.base.grid, size)));
            }
        }
    }

    pub fn cancel(&mut self) {
        self.draft = Arc::default();
        self.resize = None;
        self.last = None;
    }

    pub fn save(&mut self, target: WindowId, project: &Project, picked: Option<&Pick>) -> bool {
        self.last = None;
        let draft = std::mem::take(&mut self.draft);
        let dimensions = self.resize.take().map(|grid| grid.size());
        if (draft.is_empty() && dimensions.is_none())
            || self.target.is_none_or(|(id, _)| id != target)
        {
            return false;
        }
        let sprite = if self.brush == Brush::Paint {
            self.sprite(picked)
        } else {
            None
        };
        let Some(doc) = &mut self.document else {
            return false;
        };
        let edits: Vec<_> = draft
            .iter()
            .map(|&cell| TileEdit {
                cell,
                solid: self.brush == Brush::Paint,
                sprite: sprite.clone(),
            })
            .collect();
        match doc.base.save_changes(project.root(), dimensions, &edits) {
            Ok(saved) => {
                self.error = None;
                doc.grid = Arc::new(saved.grid(&doc.base.grid));
                doc.edits = saved;
                true
            }
            Err(err) => {
                eprintln!("could not save level: {err}");
                self.error = Some(format!("Could not save level. {err}"));
                false
            }
        }
    }
    pub fn level_id(&self) -> Option<&str> {
        Some(&self.document.as_ref()?.base.id)
    }

    pub fn reload_shared(&mut self, project: &Project, id: &str) {
        if let Some(doc) = &mut self.document
            && doc.directory == level_directory(project.root())
            && doc.base.id == id
        {
            match doc.base.read_edits(project.root()) {
                Ok(edits) => {
                    doc.grid = Arc::new(edits.grid(&doc.base.grid));
                    doc.edits = edits;
                    self.error = None;
                }
                Err(err) => self.error = Some(err.to_string()),
            }
        }
    }
}

fn trace(start: Cell, end: Cell, mut visit: impl FnMut(Cell)) {
    let (mut x, mut y) = (i32::from(start.x), i32::from(start.y));
    let (end_x, end_y) = (i32::from(end.x), i32::from(end.y));
    let dx = (end_x - x).abs();
    let dy = -(end_y - y).abs();
    let sx = (end_x - x).signum();
    let sy = (end_y - y).signum();
    let mut error = dx + dy;
    loop {
        visit(Cell::new(x as u16, y as u16));
        if x == end_x && y == end_y {
            break;
        }
        let step = 2 * error;
        if step >= dy {
            error += dy;
            x += sx;
        }
        if step <= dx {
            error += dx;
            y += sy;
        }
    }
}

pub(super) struct Layout {
    pub toolbar: Bounds,
    pub paint: Bounds,
    pub erase: Bounds,
    pub status: Bounds,
    pub grid: Bounds,
    size: Option<GridSize>,
}

impl Layout {
    pub fn new(panel: &PanelWindow, size: Option<GridSize>) -> Self {
        let toolbar = panel.chrome_bounds();
        let x = toolbar.x;
        let y = toolbar.y + 4.0;
        let (width, height) = size.map_or((0.0, 0.0), |size| {
            (
                f64::from(size.width) * CELL_SIZE,
                f64::from(size.height) * CELL_SIZE,
            )
        });
        Self {
            toolbar,
            paint: Bounds::new(x + 252.0, y, 54.0, 22.0),
            erase: Bounds::new(x + 312.0, y, 54.0, 22.0),
            status: Bounds::new(x + 372.0, y, 67.0, 22.0),
            grid: Bounds::new(x, toolbar.bottom() + 8.0, width, height),
            size,
        }
    }

    pub fn cell_at(&self, point: Point) -> Option<Cell> {
        self.grid.contains(point).then(|| {
            Cell::new(
                ((point.x - self.grid.x) / CELL_SIZE) as u16,
                ((point.y - self.grid.y) / CELL_SIZE) as u16,
            )
        })
    }

    pub fn row(&self, x: u16, y: u16, end: u16) -> Bounds {
        Bounds::new(
            self.grid.x + f64::from(x) * CELL_SIZE,
            self.grid.y + f64::from(y) * CELL_SIZE,
            f64::from(end - x) * CELL_SIZE,
            CELL_SIZE,
        )
    }

    pub fn visible_tiles(&self, visible: Bounds) -> Option<(Range<u16>, Range<u16>)> {
        let region = self.grid.intersect(visible)?;
        let size = self.size?;
        let range = |start: f64, end: f64, origin: f64, limit: u16| {
            (((start - origin) / CELL_SIZE) as u16).min(limit)
                ..(((end - origin) / CELL_SIZE).ceil() as u16).min(limit)
        };
        Some((
            range(region.x, region.right(), self.grid.x, size.width),
            range(region.y, region.bottom(), self.grid.y, size.height),
        ))
    }

    pub fn resize_edges(&self, point: Point, zoom: f64) -> Option<Edges> {
        self.size?;
        let slop = (6.0 / zoom).min(CELL_SIZE / 3.0);
        let right = self.grid.right();
        let bottom = self.grid.bottom();
        let horizontal =
            (point.x - right).abs() <= slop && (self.grid.y..=bottom + slop).contains(&point.y);
        let vertical =
            (point.y - bottom).abs() <= slop && (self.grid.x..=right + slop).contains(&point.x);
        (horizontal || vertical).then_some(Edges {
            horizontal: i8::from(horizontal),
            vertical: i8::from(vertical),
        })
    }

    pub fn show_grid_lines(zoom: f64) -> bool {
        CELL_SIZE * zoom >= 6.0
    }
}

pub(super) fn snapped_size(initial: GridSize, edges: Edges, delta: Point) -> GridSize {
    let axis = |before, delta: f64, edge| {
        let cells = f64::from(before) + (delta / CELL_SIZE).round() * f64::from(edge);
        cells.clamp(1.0, f64::from(u16::MAX)) as u16
    };
    let mut width = axis(initial.width, delta.x, edges.horizontal);
    let mut height = axis(initial.height, delta.y, edges.vertical);
    if edges.vertical == 0 {
        width = usize::from(width).min(MAX_GRID_CELLS / usize::from(height)) as u16;
    } else {
        height = usize::from(height).min(MAX_GRID_CELLS / usize::from(width)) as u16;
    }
    GridSize::new(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::board::{Board, Panel};

    #[test]
    fn a_tile_picked_before_editing_is_available_to_each_level() {
        let mut editor = Editor::default();
        let root = std::env::temp_dir();
        let pick = Pick {
            definition: root.join("shared/ground.atlas.json"),
            tile: Cell::new(3, 0),
        };
        assert!(editor.view(Some(&pick)).sprite.is_none());

        let grid = TileGrid::new(2, 2, |_| false);
        editor.document = Some(Document {
            directory: root.join("one/levels"),
            base: LevelBase {
                id: "test".into(),
                palette: TilePalette {
                    empty: [0; 4],
                    solid: [255; 4],
                },
                grid: grid.clone(),
            },
            grid: Arc::new(grid),
            edits: LevelEdits::default(),
        });
        let first = editor.view(Some(&pick)).sprite.unwrap();
        editor.document.as_mut().unwrap().directory = root.join("projects/two/levels");
        let second = editor.view(Some(&pick)).sprite.unwrap();
        assert_eq!(first.tile, second.tile);
        assert_ne!(first.source, second.source);
        assert_eq!(
            first.source.key(&root.join("one/levels")),
            second.source.key(&root.join("projects/two/levels"))
        );
    }

    #[test]
    fn transparent_cells_select_only_when_the_level_is_selected_and_panels_stay_above() {
        let mut board = Board::default();
        let area = Bounds::new(0.0, 0.0, 1100.0, 800.0);
        let game = board.add(Panel::Game, area);
        let id = board.add(Panel::LevelEditor, area);
        let grid = TileGrid::new(4, 4, |cell| cell == Cell::new(1, 0));
        let mut editor = Editor {
            document: Some(Document {
                directory: std::env::temp_dir(),
                grid: Arc::new(grid.clone()),
                base: LevelBase {
                    id: "test".into(),
                    palette: TilePalette {
                        empty: [0; 4],
                        solid: [255; 4],
                    },
                    grid,
                },
                edits: LevelEdits::default(),
            }),
            ..Editor::default()
        };
        board.set_bounds(id, Bounds::new(10.0, 10.0, 660.0, 448.0));
        let empty = Point::new(20.0, 58.0);
        let solid = Point::new(40.0, 58.0);
        board.set_bounds(game, Bounds::new(30.0, 10.0, 320.0, 240.0));
        board.raise(id);
        let panel = board.window(id).unwrap();
        assert_eq!(board.panel_at(solid), Some(game));
        assert_eq!(board.panel_at(Point::new(37.0, 20.0)), Some(game));
        assert_eq!(board.panel_at(empty), None);
        assert!(!editor.hits_grid(panel, empty, 1.0));
        assert!(editor.hits_grid(panel, solid, 1.0));
        editor.set_selected(true);
        assert!(editor.hits_grid(panel, empty, 1.0));

        let initial = editor.layout(panel);
        let corner = Point::new(initial.grid.right(), initial.grid.bottom());
        let edges = initial.resize_edges(corner, 1.0).unwrap();
        assert_eq!(
            snapped_size(GridSize::new(4, 4), edges, Point::new(29.0, -32.0)),
            GridSize::new(5, 2)
        );
        editor.resize_to(GridSize::new(5, 2));
        let resized = editor.layout(panel);
        assert_eq!(
            (resized.grid.x, resized.grid.y),
            (initial.grid.x, initial.grid.y)
        );
        assert_eq!(resized.grid.w - initial.grid.w, CELL_SIZE);
        assert_eq!(editor.size(), Some(GridSize::new(4, 4)));
        editor.cancel();
        assert_eq!(editor.layout(panel).grid, initial.grid);
    }
}
