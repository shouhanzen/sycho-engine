use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, SystemTime},
};

use serde::Deserialize;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use super::{Atlas, Definition, is_png, normalize, read_json};
use crate::{graphics::Image, surface::SurfaceSize};

const POLL_INTERVAL: Duration = Duration::from_millis(160);

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AssetKey {
    Atlas(PathBuf),
    Texture(PathBuf),
    Legacy { manifest: PathBuf, name: String },
}

#[derive(Clone, Default)]
pub struct AssetValue {
    pub atlas: Option<Arc<Atlas>>,
    pub image: Option<Arc<Image>>,
    pub error: Option<String>,
}

type Values = BTreeMap<AssetKey, AssetValue>;

#[derive(Default)]
pub struct AssetWatch {
    requests: Option<mpsc::Sender<BTreeSet<AssetKey>>>,
    results: Option<mpsc::Receiver<Values>>,
    keys: BTreeSet<AssetKey>,
    latest: Values,
    notify: Option<Box<dyn Fn() + Send>>,
}

impl AssetWatch {
    pub fn with_notify(notify: impl Fn() + Send + 'static) -> Self {
        Self {
            notify: Some(Box::new(notify)),
            ..Self::default()
        }
    }

    pub fn watch(&mut self, keys: BTreeSet<AssetKey>) {
        if self.keys == keys {
            return;
        }
        let requests = self.requests.get_or_insert_with(|| {
            let (requests, input) = mpsc::channel();
            let (output, results) = mpsc::channel();
            self.results = Some(results);
            let notify = self.notify.take();
            thread::spawn(move || run(input, output, notify));
            requests
        });
        self.latest.retain(|key, _| keys.contains(key));
        let _ = requests.send(keys.clone());
        self.keys = keys;
    }

    pub fn poll(&mut self) -> bool {
        let Some(results) = &self.results else {
            return false;
        };
        let mut changed = false;
        for mut values in results.try_iter() {
            values.retain(|key, _| self.keys.contains(key));
            self.latest = values;
            changed = true;
        }
        changed
    }

    pub fn get(&self, key: &AssetKey) -> Option<&AssetValue> {
        self.latest.get(key)
    }
}

fn run(
    input: mpsc::Receiver<BTreeSet<AssetKey>>,
    output: mpsc::Sender<Values>,
    notify: Option<Box<dyn Fn() + Send>>,
) {
    let mut assets = BTreeMap::<AssetKey, Watched>::new();
    loop {
        let next = if assets.is_empty() {
            input
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        } else {
            input.recv_timeout(POLL_INTERVAL)
        };
        let mut changed = false;
        match next {
            Ok(mut keys) => {
                for newer in input.try_iter() {
                    keys = newer;
                }
                assets.retain(|key, _| keys.contains(key));
                for key in keys {
                    assets.entry(key).or_default();
                }
                changed = true;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for (key, asset) in &mut assets {
            changed |= asset.refresh(key);
        }
        if changed {
            if output
                .send(
                    assets
                        .iter()
                        .map(|(key, asset)| (key.clone(), asset.value.clone()))
                        .collect(),
                )
                .is_err()
            {
                break;
            }
            if let Some(notify) = &notify {
                notify();
            }
        }
    }
}

#[derive(Default)]
struct Watched {
    value: AssetValue,
    files: BTreeMap<PathBuf, Stamp>,
    pending: bool,
}

#[derive(PartialEq, Eq)]
enum Stamp {
    Present {
        size: u64,
        time: Option<SystemTime>,
        #[cfg(unix)]
        inode: u64,
        #[cfg(unix)]
        changed: (i64, i64),
    },
    Unavailable(io::ErrorKind),
}

impl Stamp {
    fn read(path: &Path) -> Self {
        match fs::metadata(path) {
            Ok(meta) => Self::Present {
                size: meta.len(),
                time: meta.modified().ok(),
                #[cfg(unix)]
                inode: meta.ino(),
                #[cfg(unix)]
                changed: (meta.ctime(), meta.ctime_nsec()),
            },
            Err(err) => Self::Unavailable(err.kind()),
        }
    }
}

impl Watched {
    fn refresh(&mut self, key: &AssetKey) -> bool {
        let mut changed = false;
        for (path, stamp) in &mut self.files {
            let now = Stamp::read(path);
            if now != *stamp {
                *stamp = now;
                changed = true;
            }
        }
        if changed {
            // Save-by-replacement and chunked writes settle before decoding.
            self.pending = true;
            return false;
        }
        if !self.files.is_empty() && !std::mem::take(&mut self.pending) {
            return false;
        }
        let mut files = BTreeMap::new();
        let mut record = |path: &Path| {
            files.insert(path.to_path_buf(), Stamp::read(path));
        };
        match load(key, &mut record) {
            Ok(value) => self.value = value,
            Err((path, err)) => {
                if err.kind() == io::ErrorKind::NotFound {
                    self.value = AssetValue::default();
                }
                self.value.error = Some(format!("{}: {err}", path.display()));
            }
        }
        self.files = files;
        true
    }
}

type ReadError = (PathBuf, io::Error);

fn load(key: &AssetKey, record: &mut impl FnMut(&Path)) -> Result<AssetValue, ReadError> {
    if let AssetKey::Texture(path) = key {
        return Ok(AssetValue {
            image: Some(texture(path, record)?),
            ..AssetValue::default()
        });
    }
    let (definition, directory) = match key {
        AssetKey::Atlas(path) => (definition(path, record)?, parent(path)?),
        AssetKey::Legacy { manifest, name } => {
            #[derive(Deserialize)]
            #[serde(untagged)]
            enum Legacy {
                Alias { definition: PathBuf },
                Inline(Definition),
            }
            record(manifest);
            let mut entries: BTreeMap<String, Legacy> =
                read_json(manifest).map_err(|err| (manifest.clone(), err))?;
            let directory = parent(manifest)?;
            match entries.remove(name) {
                Some(Legacy::Alias { definition: path }) => {
                    let path = normalize(&directory.join(path));
                    (definition(&path, record)?, parent(&path)?)
                }
                Some(Legacy::Inline(definition)) => (definition, directory),
                None => {
                    return Err((
                        manifest.clone(),
                        io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("atlas {name:?} is absent"),
                        ),
                    ));
                }
            }
        }
        AssetKey::Texture(_) => unreachable!(),
    };
    let path = normalize(&directory.join(&definition.texture));
    let image = texture(&path, record)?;
    let tile_size = SurfaceSize::new(
        u32::from(definition.tile_width),
        u32::from(definition.tile_height),
    );
    let size = image.size();
    if tile_size.is_empty()
        || size.width % tile_size.width != 0
        || size.height % tile_size.height != 0
    {
        return Err((
            path,
            io::Error::other("the PNG must divide evenly into positive tile dimensions"),
        ));
    }
    Ok(AssetValue {
        atlas: Some(Arc::new(Atlas {
            texture: path,
            image,
            tile_size,
        })),
        ..AssetValue::default()
    })
}

fn parent(path: &Path) -> Result<PathBuf, ReadError> {
    path.parent().map(Path::to_path_buf).ok_or_else(|| {
        (
            path.to_path_buf(),
            io::Error::other("expected an asset file path"),
        )
    })
}

fn definition(path: &Path, record: &mut impl FnMut(&Path)) -> Result<Definition, ReadError> {
    record(path);
    read_json(path).map_err(|err| (path.to_path_buf(), err))
}

fn texture(path: &Path, record: &mut impl FnMut(&Path)) -> Result<Arc<Image>, ReadError> {
    record(path);
    if !is_png(path) {
        return Err((path.to_path_buf(), io::Error::other("expected a PNG file")));
    }
    Image::read_png(path)
        .map(Arc::new)
        .map_err(|err| (path.to_path_buf(), err))
}
