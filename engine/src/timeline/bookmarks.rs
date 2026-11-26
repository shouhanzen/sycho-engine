use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum BookmarkEdit {
    Save(String),
    Remove(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: u64,
    pub name: String,
    pub frame: Frame,
}

/// The lightweight index is safe to read while the game writes the timeline.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct BookmarkLedger {
    revision: u64,
    entries: Vec<Bookmark>,
}

impl BookmarkLedger {
    pub fn read(directory: &Path) -> io::Result<Self> {
        match File::open(directory.join("bookmarks.json")) {
            Ok(file) => Ok(serde_json::from_reader(BufReader::new(file))?),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err),
        }
    }

    pub fn entries(&self) -> &[Bookmark] {
        &self.entries
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn next_revision(&self) -> io::Result<u64> {
        self.revision
            .checked_add(1)
            .ok_or_else(|| io::Error::other("bookmark ledger exhausted its IDs"))
    }

    pub(super) fn save(&self, directory: &Path) -> io::Result<()> {
        write_json_atomic(
            &directory.join("bookmarks.json"),
            self,
            JsonWriteOptions::PRETTY,
        )
    }

    pub(super) fn get(&self, id: u64) -> io::Result<&Bookmark> {
        self.entries
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "bookmark does not exist"))
    }

    pub(super) fn state(directory: &Path, id: u64) -> io::Result<Value> {
        let file = File::open(state_path(directory, id))?;
        Ok(serde_json::from_reader(BufReader::new(file))?)
    }
}

impl Timeline {
    /// Save the exact selected sample, independently of the active history.
    pub fn save_bookmark(&mut self, name: &str) -> io::Result<Bookmark> {
        self.flush()?;
        if self.detached_bookmark().is_some() {
            return Err(invalid_data(
                "return to the timeline before saving a bookmark",
            ));
        }
        let frame = self
            .position
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?;
        let name = name.trim();
        if name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err(invalid_data("bookmark names must fit in 80 characters"));
        }
        let id = self.bookmarks.next_revision()?;
        let bookmark = Bookmark {
            id,
            name: if name.is_empty() {
                format!("Bookmark {id}")
            } else {
                name.to_owned()
            },
            frame,
        };
        let directory = &self.checkpoints.directory;
        let (_, state) = self.checkpoints.snapshot_value(frame.id)?;
        let path = state_path(directory, id);
        write_json_atomic(&path, &state, JsonWriteOptions::COMPACT)?;
        sync_directory(path.parent().unwrap())?;
        let mut ledger = self.bookmarks.clone();
        ledger.revision = id;
        ledger.entries.push(bookmark.clone());
        ledger.save(directory)?;
        self.bookmarks = ledger;
        sync_directory(directory)?;
        Ok(bookmark)
    }

    pub fn remove_bookmark(&mut self, id: u64) -> io::Result<()> {
        if self.detached_bookmark() == Some(id) {
            return Err(invalid_data(
                "return to the timeline before removing this bookmark",
            ));
        }
        self.bookmarks.get(id)?;
        let directory = &self.checkpoints.directory;
        let mut ledger = self.bookmarks.clone();
        ledger.revision = ledger.next_revision()?;
        ledger.entries.retain(|entry| entry.id != id);
        ledger.save(directory)?;
        self.bookmarks = ledger;
        sync_directory(directory)?;
        let _ = fs::remove_file(state_path(directory, id));
        Ok(())
    }

    pub fn bookmark_revision(&self) -> u64 {
        self.bookmarks.revision()
    }

    pub fn detached_bookmark(&self) -> Option<u64> {
        self.head.as_ref()?.preview_bookmark
    }

    pub fn open_bookmark<S: DeserializeOwned>(&mut self, id: u64) -> io::Result<S> {
        self.flush()?;
        let bookmark = self.bookmarks.get(id)?.clone();
        let mut head = self
            .head
            .clone()
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?;
        let value = BookmarkLedger::state(&self.checkpoints.directory, id)?;
        if !head.detached_bookmarks.contains(&id) {
            let frame = Frame {
                id: head
                    .bookmark_links
                    .get(&id)
                    .copied()
                    .unwrap_or(bookmark.frame.id),
                ..bookmark.frame
            };
            let state = self.select_value(frame, value)?;
            self.future = None;
            return Ok(state);
        }
        let state = S::deserialize(&value)?;
        head.paused = true;
        head.preview_bookmark = Some(id);
        self.commit_head(head)?;
        self.position = Some(bookmark.frame);
        self.future = None;
        self.selected_value = None;
        sync_directory(&self.checkpoints.directory)?;
        Ok(state)
    }

    pub fn return_from_bookmark<S: DeserializeOwned>(&mut self) -> io::Result<Option<S>> {
        let Some(head) = self
            .head
            .as_ref()
            .filter(|head| head.preview_bookmark.is_some())
        else {
            return Ok(None);
        };
        let frame = self.checkpoints.frame(head.cursor)?;
        let state = self.select(frame)?;
        self.future = None;
        Ok(Some(state))
    }
}

fn state_path(directory: &Path, id: u64) -> PathBuf {
    directory.join("bookmarks").join(format!("{id:020}.json"))
}
