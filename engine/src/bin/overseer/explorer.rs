use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::project::Project;

use super::board::{Bounds, Point};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Browse,
    Create,
    Open,
    Atlas,
    Texture,
}

impl Mode {
    pub fn title(self) -> &'static str {
        match self {
            Self::Browse => "Explorer",
            Self::Create => "New Game",
            Self::Open => "Open Game",
            Self::Atlas => "Open Atlas",
            Self::Texture => "Open Texture",
        }
    }
    pub fn accepts_asset(self, path: &Path) -> bool {
        match self {
            Self::Atlas => engine::tilesets::is_atlas(path),
            Self::Texture => engine::tilesets::is_png(path),
            _ => false,
        }
    }

    pub fn picks_asset(self) -> bool {
        matches!(self, Self::Atlas | Self::Texture)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Field {
    Path,
    Name,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum EntryKind {
    Project,
    Folder,
    Manifest,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Directory {
    pub path: PathBuf,
    pub is_project: bool,
    pub entries: Vec<Entry>,
}

impl Directory {
    pub fn read(path: &Path, mode: Mode) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let is_project = path.join("Cargo.toml").is_file();
        let mut entries = Vec::new();
        for file in fs::read_dir(&path)? {
            let file = file?;
            let name = file.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let target = file.path();
            let kind = file.file_type()?;
            let directory = kind.is_dir() || kind.is_symlink() && target.is_dir();
            let kind = if directory {
                if target.join("Cargo.toml").is_file() {
                    EntryKind::Project
                } else {
                    EntryKind::Folder
                }
            } else if name == "Cargo.toml" {
                EntryKind::Manifest
            } else {
                EntryKind::File
            };
            if match mode {
                Mode::Browse => true,
                Mode::Create => directory,
                Mode::Open => directory || kind == EntryKind::Manifest,
                Mode::Atlas | Mode::Texture => directory || mode.accepts_asset(&target),
            } {
                entries.push(Entry {
                    path: target,
                    name,
                    kind,
                });
            }
        }
        entries.sort_by(|a, b| {
            let is_file =
                |entry: &Entry| matches!(entry.kind, EntryKind::Manifest | EntryKind::File);
            is_file(a)
                .cmp(&is_file(b))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(Self {
            path,
            is_project,
            entries,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Explorer {
    pub mode: Mode,
    pub directory: Option<Arc<Directory>>,
    pub path: String,
    pub name: String,
    pub field: Option<Field>,
    pub targets: Arc<[Project]>,
    pub offset: usize,
    pub selected: Option<usize>,
    pub submitting: bool,
    pub error: Option<String>,
    loading: Option<u64>,
    sequence: u64,
    scroll: f32,
    select_all: bool,
}

pub(super) enum Action {
    List(PathBuf),
    Submit(PathBuf),
    Choose(Project),
}

impl Explorer {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            directory: None,
            path: String::new(),
            name: String::new(),
            field: None,
            targets: Arc::default(),
            offset: 0,
            selected: None,
            submitting: false,
            error: None,
            loading: None,
            sequence: 0,
            scroll: 0.0,
            select_all: false,
        }
    }

    pub fn busy(&self) -> bool {
        self.submitting || self.loading.is_some()
    }

    pub fn start(&mut self, path: &Path) -> u64 {
        self.sequence += 1;
        self.loading = Some(self.sequence);
        self.path = path.display().to_string();
        self.field = None;
        self.error = None;
        self.sequence
    }

    pub fn complete(&mut self, request: u64, result: Result<Directory, String>) -> bool {
        if self.loading != Some(request) {
            return false;
        }
        self.loading = None;
        match result {
            Ok(directory) => {
                self.path = directory.path.display().to_string();
                self.directory = Some(Arc::new(directory));
                self.offset = 0;
                self.selected = None;
                self.scroll = 0.0;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }

    pub fn targets(&mut self, projects: Vec<Project>) {
        self.targets = projects.into();
        self.field = None;
        self.offset = 0;
        self.selected = Some(0);
    }

    pub fn back(&mut self) {
        self.targets = Arc::default();
        self.offset = 0;
        self.selected = None;
        self.error = None;
    }

    pub fn rows(&self) -> usize {
        if !self.targets.is_empty() {
            self.targets.len()
        } else {
            self.directory
                .as_ref()
                .map_or(0, |directory| directory.entries.len())
        }
    }

    pub fn scroll(&mut self, amount: f32, visible: usize) {
        self.scroll -= amount;
        let steps = self.scroll.trunc() as isize;
        self.scroll -= steps as f32;
        self.offset = self
            .offset
            .saturating_add_signed(steps)
            .min(self.rows().saturating_sub(visible));
    }

    pub fn select_step(&mut self, difference: isize, visible: usize) {
        if self.rows() == 0 {
            return;
        }
        let index = self
            .selected
            .map_or(0, |index| index.saturating_add_signed(difference))
            .min(self.rows() - 1);
        self.selected = Some(index);
        if index < self.offset {
            self.offset = index;
        } else if index >= self.offset + visible {
            self.offset = index + 1 - visible;
        }
    }

    pub fn current_offset(&self, visible: usize) -> usize {
        self.offset.min(self.rows().saturating_sub(visible))
    }

    pub fn select_field(&mut self, field: Field) {
        if self.field != Some(field) {
            self.field = Some(field);
            self.select_all = field == Field::Path;
        }
    }

    pub fn mark_all(&mut self) {
        if self.field.is_some() {
            self.select_all = true;
        }
    }

    pub fn all_selected(&self) -> bool {
        self.select_all && self.field.is_some()
    }

    fn edit(&mut self) -> Option<&mut String> {
        let text = match self.field? {
            Field::Path => &mut self.path,
            Field::Name => &mut self.name,
        };
        if std::mem::take(&mut self.select_all) {
            text.clear();
        }
        self.error = None;
        Some(text)
    }

    pub fn input(&mut self, text: &str) {
        if !self.busy()
            && let Some(field) = self.edit()
        {
            field.extend(text.chars().filter(|ch| !ch.is_control()));
        }
    }

    pub fn backspace(&mut self) {
        if !self.busy()
            && let Some(field) = self.edit()
        {
            field.pop();
        }
    }

    pub fn parent(&self) -> Option<Action> {
        self.directory
            .as_ref()?
            .path
            .parent()
            .map(|parent| Action::List(parent.into()))
    }

    fn input_path(&self) -> Option<PathBuf> {
        let text = self.path.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(suffix) = text.strip_prefix("~/") {
            return Some(home()?.join(suffix));
        }
        if text == "~" {
            return home();
        }
        let path = PathBuf::from(text);
        if path.is_absolute() {
            Some(path)
        } else {
            Some(self.directory.as_ref()?.path.join(path))
        }
    }

    fn destination(&self) -> Option<PathBuf> {
        let directory = self.input_path()?;
        let name = self.name.trim();
        let mut components = Path::new(name).components();
        if matches!(components.next(), Some(Component::Normal(part)) if part == name)
            && components.next().is_none()
        {
            Some(directory.join(name))
        } else {
            None
        }
    }

    pub fn confirm(&mut self) -> Option<Action> {
        if self.busy() {
            return None;
        }
        match self.mode {
            Mode::Browse => None,
            Mode::Open => self.input_path().map(Action::Submit),
            Mode::Atlas | Mode::Texture => {
                if let Some(entry) = self
                    .selected
                    .and_then(|index| self.directory.as_ref()?.entries.get(index))
                    && self.mode.accepts_asset(&entry.path)
                    && self.field.is_none()
                {
                    return Some(Action::Submit(entry.path.clone()));
                }
                self.input_path().map(|path| {
                    if self.mode.accepts_asset(&path) {
                        Action::Submit(path)
                    } else {
                        Action::List(path)
                    }
                })
            }
            Mode::Create => {
                let destination = self.destination();
                if destination.is_none() {
                    self.error = Some("Enter a single folder name for the game".into());
                }
                destination.map(Action::Submit)
            }
        }
    }

    pub fn enter(&mut self) -> Option<Action> {
        if self.busy() {
            return None;
        }
        match self.field {
            Some(Field::Path) => self.input_path().map(|path| {
                if self.mode.accepts_asset(&path)
                    || self.mode == Mode::Open
                        && path.file_name().is_some_and(|name| name == "Cargo.toml")
                {
                    Action::Submit(path)
                } else {
                    Action::List(path)
                }
            }),
            Some(Field::Name) => self.confirm(),
            None => self
                .selected
                .and_then(|index| self.activate(index))
                .or_else(|| self.confirm()),
        }
    }

    pub fn activate(&mut self, index: usize) -> Option<Action> {
        if self.busy() || index >= self.rows() {
            return None;
        }
        self.field = None;
        self.selected = Some(index);
        if let Some(project) = self.targets.get(index) {
            return Some(Action::Choose(project.clone()));
        }
        let entry = self.directory.as_ref()?.entries.get(index)?;
        match entry.kind {
            EntryKind::Project | EntryKind::Folder => Some(Action::List(entry.path.clone())),
            EntryKind::Manifest if self.mode == Mode::Open => {
                Some(Action::Submit(entry.path.clone()))
            }
            EntryKind::File if self.mode.accepts_asset(&entry.path) => {
                Some(Action::Submit(entry.path.clone()))
            }
            _ => None,
        }
    }
}

pub(super) struct Shortcut {
    pub name: String,
    pub path: PathBuf,
}

pub(super) fn games() -> PathBuf {
    crate::engine_root()
        .parent()
        .unwrap_or(crate::engine_root())
        .join("games")
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub(super) fn shortcuts(recent: &[PathBuf]) -> Vec<Shortcut> {
    let mut entries = vec![Shortcut {
        name: "Games".into(),
        path: games(),
    }];
    entries.push(Shortcut {
        name: "Home".into(),
        path: home().unwrap_or_else(games),
    });
    entries.extend(recent.iter().map(|path| Shortcut {
        name: path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path: path.clone(),
    }));
    entries
}

pub(super) struct Layout {
    pub path: Bounds,
    pub up: Bounds,
    pub name: Bounds,
    pub confirm: Bounds,
    pub list: Bounds,
    pub sidebar: Bounds,
    pub message: Bounds,
}

impl Layout {
    const ROW_HEIGHT: f64 = 25.0;

    pub fn new(bounds: Bounds) -> Self {
        let left = bounds.x + 12.0;
        let right = bounds.right() - 12.0;
        let toolbar = bounds.y + 40.0;
        let list_top = toolbar + 36.0;
        let footer = bounds.bottom() - 40.0;
        let list_height = footer - 27.0 - list_top;
        Self {
            up: Bounds::new(left, toolbar, 34.0, 28.0),
            path: Bounds::new(left + 42.0, toolbar, right - left - 42.0, 28.0),
            sidebar: Bounds::new(left, list_top, 122.0, list_height),
            list: Bounds::new(left + 131.0, list_top, right - left - 131.0, list_height),
            message: Bounds::new(left, footer - 23.0, right - left, 21.0),
            name: Bounds::new(left, footer, right - left - 136.0, 28.0),
            confirm: Bounds::new(right - 126.0, footer, 126.0, 28.0),
        }
    }

    pub fn visible(&self) -> usize {
        (self.list.h / Self::ROW_HEIGHT).floor() as usize
    }

    pub fn row(&self, index: usize) -> Bounds {
        Bounds::new(
            self.list.x,
            self.list.y + index as f64 * Self::ROW_HEIGHT,
            self.list.w,
            Self::ROW_HEIGHT,
        )
    }

    pub fn shortcut(&self, index: usize) -> Bounds {
        let offset = index as f64 * Self::ROW_HEIGHT + if index >= 2 { 22.0 } else { 0.0 };
        Bounds::new(
            self.sidebar.x,
            self.sidebar.y + offset,
            self.sidebar.w,
            Self::ROW_HEIGHT,
        )
    }

    pub fn row_at(&self, point: Point) -> Option<usize> {
        let index = ((point.y - self.list.y) / Self::ROW_HEIGHT).floor() as usize;
        (self.list.contains(point) && index < self.visible()).then_some(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Staging;

    #[test]
    fn directory_marks_cargo_projects_and_matches_each_mode() {
        let staging = Staging::new().unwrap();
        let project = staging.0.join("example");
        let ordinary = staging.0.join("folder");
        fs::create_dir(&project).unwrap();
        fs::create_dir(&ordinary).unwrap();
        fs::write(project.join("Cargo.toml"), "").unwrap();
        fs::write(staging.0.join("README.md"), "notes").unwrap();
        let browse = Directory::read(&staging.0, Mode::Browse).unwrap();
        assert_eq!(
            browse
                .entries
                .iter()
                .map(|entry| entry.kind)
                .collect::<Vec<_>>(),
            [EntryKind::Project, EntryKind::Folder, EntryKind::File]
        );
        let create = Directory::read(&staging.0, Mode::Create).unwrap();
        assert_eq!(create.entries.len(), 2);
        let open = Directory::read(&project, Mode::Open).unwrap();
        assert!(open.is_project);
        assert_eq!(open.entries[0].kind, EntryKind::Manifest);
    }

    #[test]
    fn asset_pickers_filter_and_open_files_by_type() {
        let staging = Staging::new().unwrap();
        let atlas = staging.0.join("brick.atlas.json");
        let png = staging.0.join("brick.PNG");
        for path in [&atlas, &png] {
            fs::write(path, "").unwrap();
        }
        fs::create_dir(staging.0.join("art")).unwrap();
        for (mode, expected) in [(Mode::Atlas, atlas), (Mode::Texture, png)] {
            let mut explorer = Explorer::new(mode);
            let request = explorer.start(&staging.0);
            let directory = Directory::read(&staging.0, mode).unwrap();
            assert_eq!(directory.entries.len(), 2);
            assert_eq!(directory.entries[0].kind, EntryKind::Folder);
            assert_eq!(
                directory.entries[1].name,
                expected.file_name().unwrap().to_str().unwrap()
            );
            assert!(explorer.complete(request, Ok(directory)));
            assert!(
                matches!(explorer.activate(1), Some(Action::Submit(path)) if path == expected.canonicalize().unwrap())
            );
        }
    }

    #[test]
    fn late_directory_results_leave_the_latest_location_and_create_requires_one_name() {
        let staging = Staging::new().unwrap();
        let before = staging.0.join("before");
        fs::create_dir(&before).unwrap();
        let mut explorer = Explorer::new(Mode::Create);
        let stale = explorer.start(&before);
        let request = explorer.start(&staging.0);
        assert!(!explorer.complete(stale, Ok(Directory::read(&before, Mode::Create).unwrap())));
        assert!(explorer.complete(
            request,
            Ok(Directory::read(&staging.0, Mode::Create).unwrap())
        ));
        assert_eq!(
            explorer.directory.as_ref().unwrap().path,
            staging.0.canonicalize().unwrap()
        );
        explorer.select_field(Field::Name);
        explorer.input("../escaped");
        assert!(explorer.confirm().is_none());
        explorer.mark_all();
        explorer.input("starter");
        assert!(
            matches!(explorer.confirm(), Some(Action::Submit(path)) if path == staging.0.canonicalize().unwrap().join("starter"))
        );
    }
}
