//! Filesystem atlas definitions and the tile references stored in levels.
mod watch;

pub use watch::{AssetKey, AssetValue, AssetWatch};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{self, BufReader},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{graphics::Image, surface::SurfaceSize, tiles::Cell, ui::Rect};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AtlasRef {
    File { atlas_path: PathBuf },
    Legacy { atlas: String },
}

impl AtlasRef {
    pub fn key(&self, level_directory: &Path) -> AssetKey {
        match self {
            Self::File { atlas_path } => {
                AssetKey::Atlas(normalize(&level_directory.join(atlas_path)))
            }
            Self::Legacy { atlas } => AssetKey::Legacy {
                manifest: normalize(&level_directory.join("../assets/tilesets.json")),
                name: atlas.clone(),
            },
        }
    }

    pub(crate) fn valid(&self) -> bool {
        match self {
            Self::File { atlas_path } => {
                is_atlas(atlas_path) && atlas_path.is_relative() && !atlas_path.has_root()
            }
            Self::Legacy { atlas } => {
                !atlas.is_empty()
                    && atlas.len() <= 80
                    && atlas
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRef {
    #[serde(flatten)]
    pub source: AtlasRef,
    pub tile: Cell,
}

#[derive(Deserialize)]
struct Definition {
    texture: PathBuf,
    tile_width: u16,
    tile_height: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Atlas {
    pub texture: PathBuf,
    pub image: Arc<Image>,
    pub tile_size: SurfaceSize,
}

impl Atlas {
    pub fn dimensions(&self) -> SurfaceSize {
        let image = self.image.size();
        SurfaceSize::new(
            image.width / self.tile_size.width,
            image.height / self.tile_size.height,
        )
    }

    pub fn region(&self, tile: Cell) -> Option<Rect> {
        let (x, y) = (u32::from(tile.x), u32::from(tile.y));
        let size = self.dimensions();
        (x < size.width && y < size.height).then(|| {
            Rect::new(
                x * self.tile_size.width,
                y * self.tile_size.height,
                self.tile_size.width,
                self.tile_size.height,
            )
        })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Tilesets {
    atlases: BTreeMap<AtlasRef, Arc<Atlas>>,
    pub errors: Vec<String>,
}

impl Tilesets {
    pub fn from_watch(directory: &Path, sources: &BTreeSet<AtlasRef>, watch: &AssetWatch) -> Self {
        let mut result = Self::default();
        for source in sources {
            if let Some(value) = watch.get(&source.key(directory)) {
                if let Some(atlas) = &value.atlas {
                    result.atlases.insert(source.clone(), Arc::clone(atlas));
                }
                if let Some(err) = &value.error {
                    result.errors.push(err.clone());
                }
            }
        }
        result
    }

    pub fn sprite(&self, tile: &TileRef) -> (&Arc<Image>, Rect) {
        if let Some(atlas) = self.atlases.get(&tile.source)
            && let Some(region) = atlas.region(tile.tile)
        {
            (&atlas.image, region)
        } else {
            (Image::missing_texture(), Rect::from_size(2, 2))
        }
    }
}

pub fn is_atlas(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".atlas.json"))
}

pub fn is_png(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
}

/// Use a common lexical spelling even while an asset is missing.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if result.file_name().is_some_and(|name| name != "..") => {
                result.pop();
            }
            Component::ParentDir if result.has_root() => {}
            part => result.push(part.as_os_str()),
        }
    }
    result
}

pub fn relative_atlas(level_directory: &Path, definition: &Path) -> Option<AtlasRef> {
    let from = normalize(level_directory);
    let to = normalize(definition);
    if !from.is_absolute() || !to.is_absolute() || !is_atlas(&to) {
        return None;
    }
    let mut from = from.components().peekable();
    let mut to = to.components().peekable();
    if from.peek() != to.peek() {
        return None;
    }
    while from.peek().is_some() && from.peek() == to.peek() {
        from.next();
        to.next();
    }
    let mut path = PathBuf::new();
    for _ in from {
        path.push("..");
    }
    for part in to {
        path.push(part.as_os_str());
    }
    Some(AtlasRef::File { atlas_path: path })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<T> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        graphics::{CpuRenderer, ImageRect, Renderer2d},
        tiles::{LevelBase, TileEdit, TileGrid, TilePalette, level_directory},
    };
    use std::{
        fs,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("sycho-shared-atlas-{}-{now}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_image(path: &Path, color: [u8; 4]) {
        let mut png = png::Encoder::new(File::create(path).unwrap(), 4, 2);
        png.set_color(png::ColorType::Rgba);
        png.write_header()
            .unwrap()
            .write_image_data(&color.repeat(8))
            .unwrap();
    }

    fn wait_for(
        watch: &mut AssetWatch,
        key: &AssetKey,
        predicate: impl Fn(&AssetValue) -> bool,
    ) -> AssetValue {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            watch.poll();
            if let Some(value) = watch.get(key).filter(|value| predicate(value)) {
                return value.clone();
            }
            assert!(
                Instant::now() < deadline,
                "asset watcher timed out for {key:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn pixels(tilesets: &Tilesets, tile: &TileRef) -> [u8; 16] {
        let mut pixels = [0; 16];
        let size = SurfaceSize::new(2, 2);
        let (image, source) = tilesets.sprite(tile);
        CpuRenderer::new(&mut pixels, size).draw_image(
            image,
            source,
            ImageRect::new(0, 0, 2, 2),
            Rect::from_size(2, 2),
        );
        pixels
    }

    #[test]
    fn saved_levels_share_an_atlas_and_follow_the_moved_tree() {
        let temp = Scratch::new();
        let tree = temp.0.join("workspace");
        let asset = tree.join("art/ground.atlas.json");
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::write(
            &asset,
            br#"{"texture":"ground.png","tile_width":2,"tile_height":2}"#,
        )
        .unwrap();
        let color = [16, 93, 140, 255];
        write_image(&asset.with_file_name("ground.png"), color);
        let base = LevelBase {
            id: "room".into(),
            palette: TilePalette {
                empty: [0; 4],
                solid: [255; 4],
            },
            grid: TileGrid::new(3, 3, |_| false),
        };
        let cell = Cell::new(1, 1);
        let projects = ["first", "games/second"];
        for project in projects {
            let project = tree.join(project);
            let source = relative_atlas(&level_directory(&project), &asset).unwrap();
            base.save_edits(
                &project,
                &[TileEdit {
                    cell,
                    solid: true,
                    sprite: Some(TileRef {
                        source,
                        tile: Cell::new(0, 0),
                    }),
                }],
            )
            .unwrap();
            let file = fs::read_to_string(project.join("levels/room.tiles.json")).unwrap();
            assert!(!file.contains(tree.to_str().unwrap()));
            assert!(file.contains("ground.atlas.json"));
        }

        let new_root = temp.0.join("moved");
        fs::rename(&tree, &new_root).unwrap();
        let key = AssetKey::Atlas(new_root.join("art/ground.atlas.json"));
        let mut watch = AssetWatch::default();
        watch.watch(BTreeSet::from([key.clone()]));
        wait_for(&mut watch, &key, |value| value.atlas.is_some());
        for project in projects {
            let project = new_root.join(project);
            let directory = level_directory(&project);
            let grid = base.resolve(&project).unwrap();
            assert!(grid.solid(cell));
            let tile = grid.sprite(cell).unwrap();
            assert_eq!(tile.source.key(&directory), key);
            let tilesets = Tilesets::from_watch(&directory, &grid.atlas_sources(), &watch);
            assert_eq!(pixels(&tilesets, tile).as_slice(), color.repeat(4));
        }
    }

    #[test]
    fn watcher_keeps_the_last_good_image_then_marks_missing_and_recovers() {
        let temp = Scratch::new();
        let file = temp.0.join("ground.atlas.json");
        let png = temp.0.join("ground.png");
        let definition =
            |width| format!(r#"{{"texture":"ground.png","tile_width":{width},"tile_height":2}}"#);
        fs::write(&file, definition(2)).unwrap();
        write_image(&png, [200, 10, 10, 255]);

        let legacy_manifest = temp.0.join("assets/tilesets.json");
        fs::create_dir_all(legacy_manifest.parent().unwrap()).unwrap();
        fs::write(
            &legacy_manifest,
            br#"{"old":{"texture":"../ground.png","tile_width":2,"tile_height":2}}"#,
        )
        .unwrap();
        let directory = temp.0.join("levels");
        let legacy: TileRef =
            serde_json::from_str(r#"{"atlas":"old","tile":{"x":0,"y":0}}"#).unwrap();
        let tile = TileRef {
            source: relative_atlas(&directory, &file).unwrap(),
            tile: Cell::new(0, 0),
        };
        let legacy_key = legacy.source.key(&directory);
        let key = tile.source.key(&directory);
        let sources = BTreeSet::from([tile.source.clone(), legacy.source.clone()]);
        let mut watch = AssetWatch::default();
        watch.watch(BTreeSet::from([key.clone(), legacy_key.clone()]));
        let original = wait_for(&mut watch, &key, |value| value.atlas.is_some())
            .atlas
            .unwrap();
        wait_for(&mut watch, &legacy_key, |value| value.atlas.is_some());
        let read = |watch: &AssetWatch| Tilesets::from_watch(&directory, &sources, watch);
        assert_eq!(pixels(&read(&watch), &tile), pixels(&read(&watch), &legacy));

        fs::write(&png, b"partial write").unwrap();
        let partial = wait_for(&mut watch, &key, |value| value.error.is_some());
        assert!(Arc::ptr_eq(partial.atlas.as_ref().unwrap(), &original));
        let blue = [5, 10, 215, 255];
        write_image(&png, blue);
        wait_for(&mut watch, &key, |value| {
            value.error.is_none()
                && value
                    .atlas
                    .as_ref()
                    .is_some_and(|atlas| !Arc::ptr_eq(atlas, &original))
        });
        assert_eq!(pixels(&read(&watch), &tile).as_slice(), blue.repeat(4));

        fs::write(&file, definition(4)).unwrap();
        wait_for(&mut watch, &key, |value| {
            value
                .atlas
                .as_ref()
                .is_some_and(|atlas| atlas.tile_size.width == 4)
        });
        fs::remove_file(&file).unwrap();
        let missing = wait_for(&mut watch, &key, |value| {
            value.atlas.is_none() && value.error.is_some()
        });
        assert!(missing.error.unwrap().contains("ground.atlas.json"));
        assert_eq!(
            pixels(&read(&watch), &tile),
            [
                255, 0, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 0, 255, 255
            ]
        );

        fs::write(&file, definition(2)).unwrap();
        wait_for(&mut watch, &key, |value| {
            value.error.is_none() && value.atlas.is_some()
        });
        assert_eq!(pixels(&read(&watch), &tile).as_slice(), blue.repeat(4));
    }
}
