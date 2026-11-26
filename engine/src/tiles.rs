//! Immutable tile levels and the cell overrides authored in the editor.
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::graphics::Color;
use crate::persistence::{JsonWriteOptions, write_json_atomic};
use crate::tilesets::{AssetWatch, AtlasRef, TileRef, Tilesets};

pub const MAX_GRID_CELLS: usize = 512 * 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cell {
    pub x: u16,
    pub y: u16,
}

impl Cell {
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileGrid {
    width: u16,
    height: u16,
    cells: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sprites: Vec<GroundSprite>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct GroundSprite {
    cell: Cell,
    sprite: TileRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridSize {
    pub width: u16,
    pub height: u16,
}

impl GridSize {
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    fn is_valid(self) -> bool {
        self.width != 0
            && self.height != 0
            && usize::from(self.width) * usize::from(self.height) <= MAX_GRID_CELLS
    }

    fn contains(self, cell: Cell) -> bool {
        cell.x < self.width && cell.y < self.height
    }
}

impl TileGrid {
    pub fn new(width: u16, height: u16, mut solid: impl FnMut(Cell) -> bool) -> Self {
        assert!(GridSize::new(width, height).is_valid());
        let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
        for y in 0..height {
            for x in 0..width {
                cells.push(u8::from(solid(Cell::new(x, y))));
            }
        }
        Self {
            width,
            height,
            cells,
            sprites: Vec::new(),
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn size(&self) -> GridSize {
        GridSize::new(self.width, self.height)
    }

    pub fn solid(&self, cell: Cell) -> bool {
        self.index(cell).is_some_and(|index| self.cells[index] != 0)
    }

    pub fn sprite(&self, cell: Cell) -> Option<&TileRef> {
        let index = self
            .sprites
            .binary_search_by_key(&cell, |sprite| sprite.cell)
            .ok()?;
        Some(&self.sprites[index].sprite)
    }

    pub fn atlas_sources(&self) -> BTreeSet<AtlasRef> {
        let mut sources = BTreeSet::new();
        for entry in &self.sprites {
            if !sources.contains(&entry.sprite.source) {
                sources.insert(entry.sprite.source.clone());
            }
        }
        sources
    }

    fn index(&self, cell: Cell) -> Option<usize> {
        self.size()
            .contains(cell)
            .then(|| usize::from(cell.y) * usize::from(self.width) + usize::from(cell.x))
    }

    fn apply(&mut self, edits: &[TileEdit]) {
        let mut sprites: BTreeMap<_, _> = self
            .sprites
            .drain(..)
            .map(|sprite| (sprite.cell, sprite.sprite))
            .collect();
        for edit in edits {
            if let Some(index) = self.index(edit.cell) {
                self.cells[index] = u8::from(edit.solid);
                if let Some(sprite) = edit.sprite.as_ref().filter(|_| edit.solid) {
                    sprites.insert(edit.cell, sprite.clone());
                } else {
                    sprites.remove(&edit.cell);
                }
            }
        }
        self.sprites = sprites
            .into_iter()
            .map(|(cell, sprite)| GroundSprite { cell, sprite })
            .collect();
    }

    fn validate(&self) -> io::Result<()> {
        if !self.size().is_valid()
            || usize::from(self.width) * usize::from(self.height) != self.cells.len()
            || self.cells.iter().any(|&value| value > 1)
            || !self
                .sprites
                .windows(2)
                .all(|pair| pair[0].cell < pair[1].cell)
            || self
                .sprites
                .iter()
                .any(|sprite| !self.solid(sprite.cell) || !sprite.sprite.source.valid())
        {
            return Err(invalid("invalid binary tile grid"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TilePalette {
    pub empty: Color,
    pub solid: Color,
}

/// A game can describe a level with a pure grid builder. The runtime resolves it
/// once and passes the same grid to all hooks until the level or its edits change.
#[derive(Clone, Copy)]
pub struct TileLevel {
    pub id: &'static str,
    pub project: &'static Path,
    pub binary: &'static str,
    pub build: fn() -> TileGrid,
    pub palette: TilePalette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TilePreview {
    pub base: u64,
    pub edits: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelBase {
    pub id: String,
    pub palette: TilePalette,
    pub grid: TileGrid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileEdit {
    pub cell: Cell,
    pub solid: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprite: Option<TileRef>,
}

impl TileEdit {
    pub const fn plain(cell: Cell, solid: bool) -> Self {
        Self {
            cell,
            solid,
            sprite: None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct LevelEdits {
    #[serde(default)]
    dimensions: Option<GridSize>,
    #[serde(default)]
    cells: Vec<TileEdit>,
}

impl LevelEdits {
    pub fn grid(&self, base: &TileGrid) -> TileGrid {
        self.grid_at(base, self.dimensions.unwrap_or_else(|| base.size()))
    }

    /// Build the visible grid without dropping overrides outside its bounds.
    pub fn grid_at(&self, base: &TileGrid, size: GridSize) -> TileGrid {
        let mut grid = if base.size() == size {
            base.clone()
        } else {
            let mut resized = TileGrid::new(size.width, size.height, |cell| base.solid(cell));
            resized.sprites = base
                .sprites
                .iter()
                .filter(|sprite| size.contains(sprite.cell))
                .cloned()
                .collect();
            resized
        };
        grid.apply(&self.cells);
        grid
    }
}

impl LevelBase {
    fn fingerprint(&self, grid: &TileGrid) -> u64 {
        let mut hash = Sha256::new();
        hash.update(self.id.as_bytes());
        hash.update(grid.width.to_le_bytes());
        hash.update(grid.height.to_le_bytes());
        hash.update(self.palette.empty);
        hash.update(self.palette.solid);
        hash.update(&grid.cells);
        for entry in &grid.sprites {
            hash.update(entry.cell.x.to_le_bytes());
            hash.update(entry.cell.y.to_le_bytes());
            let source = match &entry.sprite.source {
                AtlasRef::Legacy { atlas } => atlas.as_bytes(),
                AtlasRef::File { atlas_path } => atlas_path.as_os_str().as_encoded_bytes(),
            };
            hash.update((source.len() as u64).to_le_bytes());
            hash.update(source);
            hash.update(entry.sprite.tile.x.to_le_bytes());
            hash.update(entry.sprite.tile.y.to_le_bytes());
        }
        let bytes = hash.finalize();
        u64::from_le_bytes(bytes[..8].try_into().unwrap())
    }

    fn preview_path(project: &Path, binary: &str, fingerprint: u64) -> io::Result<PathBuf> {
        Ok(project
            .join(".sycho/level-cache")
            .join(format!("{}-{fingerprint:016x}.json", name(binary)?)))
    }

    fn edit_path(&self, project: &Path) -> io::Result<PathBuf> {
        Ok(level_directory(project).join(format!("{}.tiles.json", name(&self.id)?)))
    }

    /// The cache is the current code-defined grid, shared with the local editor
    /// through the project directory. Runtime frames carry only its fingerprint.
    pub fn read_preview(project: &Path, binary: &str, preview: TilePreview) -> io::Result<Self> {
        let file = fs::File::open(Self::preview_path(project, binary, preview.base)?)?;
        let base: Self = serde_json::from_reader(io::BufReader::new(file))?;
        base.grid.validate()?;
        name(&base.id)?;
        if base.fingerprint(&base.grid) != preview.base {
            return Err(invalid("the game is switching levels"));
        }
        Ok(base)
    }

    pub fn resolve(&self, project: &Path) -> io::Result<TileGrid> {
        Ok(self.read_edits(project)?.grid(&self.grid))
    }

    pub fn read_edits(&self, project: &Path) -> io::Result<LevelEdits> {
        read_edits(&self.edit_path(project)?)
    }

    pub fn save_edits(&self, project: &Path, edits: &[TileEdit]) -> io::Result<TileGrid> {
        Ok(self.save_changes(project, None, edits)?.grid(&self.grid))
    }

    /// Merge a stroke or a resize with the last committed edits and save atomically.
    pub fn save_changes(
        &self,
        project: &Path,
        dimensions: Option<GridSize>,
        edits: &[TileEdit],
    ) -> io::Result<LevelEdits> {
        if dimensions.is_some_and(|size| !size.is_valid()) {
            return Err(invalid("invalid level dimensions"));
        }
        let path = self.edit_path(project)?;
        let lock_path = project
            .join(".sycho/level-locks")
            .join(format!("{}.lock", name(&self.id)?));
        fs::create_dir_all(lock_path.parent().unwrap())?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|err| io::Error::other(format!("could not lock level edits: {err}")))?;
        let mut saved = read_edits(&path)?;
        if let Some(size) = dimensions {
            saved.dimensions = Some(size);
        }
        let size = saved.dimensions.unwrap_or_else(|| self.grid.size());
        let mut merged: BTreeMap<_, _> = saved
            .cells
            .into_iter()
            .map(|edit| (edit.cell, edit))
            .collect();
        merged.extend(
            edits
                .iter()
                .filter(|edit| size.contains(edit.cell))
                .map(|edit| (edit.cell, edit.clone())),
        );
        saved.cells = merged.into_values().collect();
        if saved.dimensions.is_some() {
            write_json_atomic(&path, &saved, JsonWriteOptions::PRETTY)?;
        } else {
            write_json_atomic(&path, &saved.cells, JsonWriteOptions::PRETTY)?;
        }
        Ok(saved)
    }
}

fn name(value: &str) -> io::Result<&str> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
    {
        return Err(invalid("use a simple level or binary ID"));
    }
    Ok(value)
}

fn read_edits(path: &Path) -> io::Result<LevelEdits> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Saved {
        Cells(Vec<TileEdit>),
        Level(LevelEdits),
    }
    match fs::File::open(path) {
        Ok(file) => {
            let edits = match serde_json::from_reader(io::BufReader::new(file))? {
                Saved::Level(edits) => edits,
                Saved::Cells(cells) => LevelEdits {
                    dimensions: None,
                    cells,
                },
            };
            if edits.dimensions.is_some_and(|size| !size.is_valid()) {
                return Err(invalid("invalid level dimensions"));
            }
            Ok(edits)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(LevelEdits::default()),
        Err(err) => Err(err),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn level_directory(project: &Path) -> PathBuf {
    project.join("levels")
}

pub(crate) struct LoadedLevel {
    spec: TileLevel,
    base: LevelBase,
    grid: Arc<TileGrid>,
    pub preview: Option<TilePreview>,
    pub tilesets: Arc<Tilesets>,
    asset_watch: AssetWatch,
    asset_sources: BTreeSet<AtlasRef>,
}

impl LoadedLevel {
    pub fn load(spec: TileLevel, editor: bool) -> io::Result<Self> {
        let base = LevelBase {
            id: name(spec.id)?.to_owned(),
            palette: spec.palette,
            grid: (spec.build)(),
        };
        let grid = Arc::new(base.resolve(spec.project).unwrap_or_else(|err| {
            eprintln!("could not read tile overrides for {}: {err}", spec.id);
            base.grid.clone()
        }));
        let preview = if editor {
            match LevelBase::preview_path(spec.project, spec.binary, base.fingerprint(&base.grid))
                .and_then(|path| {
                    write_json_atomic(
                        &path,
                        &base,
                        JsonWriteOptions {
                            sync_file: false,
                            ..JsonWriteOptions::COMPACT
                        },
                    )
                }) {
                Ok(()) => Some(TilePreview {
                    base: base.fingerprint(&base.grid),
                    edits: base.fingerprint(&grid),
                }),
                Err(err) => {
                    eprintln!("could not share tile level with the editor: {err}");
                    None
                }
            }
        } else {
            None
        };
        let mut level = Self {
            spec,
            base,
            grid,
            preview,
            tilesets: Arc::default(),
            asset_watch: AssetWatch::default(),
            asset_sources: BTreeSet::new(),
        };
        level.watch_assets();
        Ok(level)
    }

    pub fn matches(&self, spec: TileLevel) -> bool {
        self.spec.id == spec.id
            && self.spec.project == spec.project
            && self.spec.binary == spec.binary
    }

    pub fn refresh(&mut self) -> io::Result<()> {
        let grid = self.base.resolve(self.spec.project)?;
        if let Some(preview) = &mut self.preview {
            preview.edits = self.base.fingerprint(&grid);
        }
        self.grid = Arc::new(grid);
        self.watch_assets();
        Ok(())
    }

    fn watch_assets(&mut self) {
        let directory = level_directory(self.spec.project);
        self.asset_sources = self.grid.atlas_sources();
        self.asset_watch.watch(
            self.asset_sources
                .iter()
                .map(|source| source.key(&directory))
                .collect(),
        );
    }

    pub fn poll_assets(&mut self) -> bool {
        if !self.asset_watch.poll() {
            return false;
        }
        let tilesets = Tilesets::from_watch(
            &level_directory(self.spec.project),
            &self.asset_sources,
            &self.asset_watch,
        );
        if tilesets.errors != self.tilesets.errors {
            for error in &tilesets.errors {
                eprintln!("tile asset: {error}");
            }
        }
        self.tilesets = Arc::new(tilesets);
        true
    }

    pub fn grid(&self) -> Arc<TileGrid> {
        Arc::clone(&self.grid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_keep_edited_cells_and_follow_code_for_untouched_cells() {
        let project = std::env::temp_dir().join(format!(
            "sycho-tile-edits-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let palette = TilePalette {
            empty: [0, 0, 0, 255],
            solid: [255; 4],
        };
        let mut level = LevelBase {
            id: "courtyard".into(),
            palette,
            grid: TileGrid::new(4, 4, |cell| cell == Cell::new(0, 0)),
        };
        let erased = Cell::new(0, 0);
        let painted = Cell::new(1, 2);
        level
            .save_edits(&project, &[TileEdit::plain(erased, false)])
            .unwrap();
        let result = level
            .save_edits(&project, &[TileEdit::plain(painted, true)])
            .unwrap();
        assert!(!result.solid(erased));
        assert!(result.solid(painted));
        assert_eq!(
            read_edits(&level.edit_path(&project).unwrap())
                .unwrap()
                .cells
                .len(),
            2
        );
        let untouched = Cell::new(3, 3);
        level.grid = TileGrid::new(4, 4, |cell| cell == erased || cell == untouched);
        let result = level.resolve(&project).unwrap();
        assert!(!result.solid(erased));
        assert!(result.solid(painted) && result.solid(untouched));
        fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn resizing_hides_and_restores_code_tiles_and_overrides_after_loading() {
        let project = std::env::temp_dir().join(format!(
            "sycho-tile-resize-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let code_tile = Cell::new(3, 2);
        let erased = Cell::new(3, 1);
        let painted = Cell::new(5, 3);
        let level = LevelBase {
            id: "courtyard".into(),
            palette: TilePalette {
                empty: [0; 4],
                solid: [255; 4],
            },
            grid: TileGrid::new(4, 4, |cell| cell == code_tile || cell == erased),
        };
        level
            .save_edits(&project, &[TileEdit::plain(erased, false)])
            .unwrap();
        let expanded = GridSize::new(6, 4);
        level
            .save_changes(&project, Some(expanded), &[TileEdit::plain(painted, true)])
            .unwrap();
        let small = GridSize::new(3, 2);
        level.save_changes(&project, Some(small), &[]).unwrap();
        let hidden = level.resolve(&project).unwrap();
        assert_eq!(hidden.size(), small);
        assert!(!hidden.solid(code_tile) && !hidden.solid(painted));

        let draft = level
            .read_edits(&project)
            .unwrap()
            .grid_at(&level.grid, expanded);
        assert!(draft.solid(code_tile) && draft.solid(painted));
        assert!(!draft.solid(erased));
        assert!(!draft.solid(Cell::new(5, 2)));
        assert_eq!(level.resolve(&project).unwrap().size(), small);

        level.save_changes(&project, Some(expanded), &[]).unwrap();
        assert_eq!(level.resolve(&project).unwrap(), draft);
        fs::remove_dir_all(project).unwrap();
    }
}
