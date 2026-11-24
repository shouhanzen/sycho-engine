//! An external, versioned state timeline. `head.json` is the commit point.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

const FORMAT_VERSION: u32 = 2;
const LEGACY_INTERVAL_MS: u64 = 250;
const KEYFRAME_INTERVAL_MS: u64 = 1_000;
const MAX_DELTAS: u8 = 4;

#[derive(Clone, Copy)]
pub struct StateSchema {
    pub game: &'static str,
    pub version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimelineCommand {
    TogglePause,
    ResumeLive,
    Rewind,
    Forward,
    TogglePlayback,
    Seek(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub id: u64,
    pub at_ms: u64,
}

#[derive(Debug)]
pub struct Snapshot<S> {
    pub frame: Frame,
    pub state: S,
}

/// Resolves an existing session or reserves a separate directory for an explicit fresh run.
pub fn session_directory(
    default: PathBuf,
    path: Option<PathBuf>,
    fresh: bool,
) -> io::Result<PathBuf> {
    let explicit = path.is_some();
    let mut path = path.unwrap_or(default);
    if !fresh {
        return Ok(path);
    }
    if !explicit {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        name.push(format!("-{}-{}", timestamp.as_nanos(), std::process::id()));
        path.set_file_name(name);
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("cannot start a fresh timeline at {}: {err}", path.display()),
        )
    })?;
    Ok(path)
}

#[derive(Clone, Serialize, Deserialize)]
struct Head {
    format: u32,
    game: String,
    schema: u32,
    next_id: u64,
    tip: u64,
    cursor: u64,
    paused: bool,
}

#[derive(Serialize, Deserialize)]
struct Checkpoint {
    previous: Option<u64>,
    #[serde(default)]
    at_ms: Option<u64>,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Deserialize)]
struct CheckpointLink {
    previous: Option<u64>,
    #[serde(default)]
    at_ms: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Payload {
    Full { state: Value },
    Delta { delta: Delta },
}

#[derive(Serialize, Deserialize)]
enum Delta {
    Replace(Value),
    Object {
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        set: BTreeMap<String, Delta>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<String>,
    },
    Array(Vec<(usize, Delta)>),
    Unchanged,
}

impl Delta {
    fn between(before: &Value, after: &Value) -> Self {
        match (before, after) {
            (Value::Object(before), Value::Object(after)) => {
                let set: BTreeMap<_, _> = after
                    .iter()
                    .filter_map(|(key, value)| {
                        let delta = before.get(key).map_or_else(
                            || Self::Replace(value.clone()),
                            |previous| Self::between(previous, value),
                        );
                        (!matches!(delta, Self::Unchanged)).then(|| (key.clone(), delta))
                    })
                    .collect();
                let remove: Vec<_> = before
                    .keys()
                    .filter(|key| !after.contains_key(*key))
                    .cloned()
                    .collect();
                if set.is_empty() && remove.is_empty() {
                    Self::Unchanged
                } else {
                    Self::Object { set, remove }
                }
            }
            (Value::Array(before), Value::Array(after)) if before.len() == after.len() => {
                let set: Vec<_> = before
                    .iter()
                    .zip(after)
                    .enumerate()
                    .filter_map(|(index, (before, after))| {
                        let delta = Self::between(before, after);
                        (!matches!(delta, Self::Unchanged)).then_some((index, delta))
                    })
                    .collect();
                if set.is_empty() {
                    Self::Unchanged
                } else {
                    Self::Array(set)
                }
            }
            (Value::Number(before), Value::Number(after)) if before.is_f64() && after.is_f64() => {
                if before.as_f64().map(f64::to_bits) == after.as_f64().map(f64::to_bits) {
                    Self::Unchanged
                } else {
                    Self::Replace(Value::Number(after.clone()))
                }
            }
            (before, after) if before == after => Self::Unchanged,
            (_, after) => Self::Replace(after.clone()),
        }
    }

    fn apply(self, value: &mut Value) -> io::Result<()> {
        match self {
            Self::Replace(next) => *value = next,
            Self::Object { set, remove } => {
                let object = value
                    .as_object_mut()
                    .ok_or_else(|| invalid_data("expected object for delta"))?;
                for key in remove {
                    object
                        .remove(&key)
                        .ok_or_else(|| invalid_data("missing field removed by delta"))?;
                }
                for (key, change) in set {
                    change.apply(object.entry(key).or_insert(Value::Null))?;
                }
            }
            Self::Array(set) => {
                let array = value
                    .as_array_mut()
                    .ok_or_else(|| invalid_data("expected array for delta"))?;
                for (index, change) in set {
                    change.apply(
                        array
                            .get_mut(index)
                            .ok_or_else(|| invalid_data("delta index out of bounds"))?,
                    )?;
                }
            }
            Self::Unchanged => {}
        }
        Ok(())
    }
}

struct CaptureBase {
    state: Value,
    keyframe_at: u64,
    deltas: u8,
}

pub struct Timeline {
    directory: PathBuf,
    _writer_lock: File,
    schema: StateSchema,
    head: Option<Head>,
    position: Option<Frame>,
    future: Option<Vec<Frame>>,
    seek_index: Option<Vec<Frame>>,
    base: Option<CaptureBase>,
}

type TimelineJob = Box<dyn FnOnce(&mut Timeline) + Send>;

/// Serializes timeline operations on one writer thread.
pub(crate) struct TimelineWorker(Sender<TimelineJob>);

impl TimelineWorker {
    pub(crate) fn start(mut timeline: Timeline) -> io::Result<Self> {
        let (sender, jobs) = mpsc::channel::<TimelineJob>();
        std::thread::Builder::new()
            .name("sycho-timeline".into())
            .spawn(move || {
                for job in jobs {
                    job(&mut timeline);
                }
            })?;
        Ok(Self(sender))
    }

    pub(crate) fn submit<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Timeline) -> io::Result<T> + Send + 'static,
    ) -> io::Result<Receiver<io::Result<T>>> {
        let (reply, result) = mpsc::channel();
        self.0
            .send(Box::new(move |timeline| {
                let _ = reply.send(operation(timeline));
            }))
            .map_err(|_| io::Error::other("timeline writer stopped"))?;
        Ok(result)
    }

    pub(crate) fn request<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Timeline) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        self.submit(operation)?
            .recv()
            .map_err(|_| io::Error::other("timeline writer stopped"))?
    }
}

impl Timeline {
    /// Opens a timeline for one writer. A compatible timeline restores its selected state.
    pub fn open<S: DeserializeOwned>(
        directory: impl Into<PathBuf>,
        schema: StateSchema,
    ) -> io::Result<(Self, Option<S>)> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("writer.lock"))?;
        lock.try_lock().map_err(|err| {
            io::Error::other(format!(
                "cannot lock timeline {}: {err}",
                directory.display()
            ))
        })?;

        let head = match read_head(&directory, schema) {
            Ok(head) => Some(head),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        let snapshot: Option<Snapshot<S>> = head.as_ref().map(|head| {
            read_snapshot(&directory, head.cursor).map_err(|err| {
                io::Error::new(err.kind(), format!(
                    "cannot restore checkpoint {} in {}: {err}. Use --fresh to start a separate timeline",
                    head.cursor, directory.display(),
                ))
            })
        }).transpose()?;
        let position = snapshot.as_ref().map(|snapshot| snapshot.frame);
        let future = if head.as_ref().is_some_and(|head| head.cursor != head.tip) {
            None
        } else {
            Some(Vec::new())
        };
        let timeline = Self {
            directory,
            _writer_lock: lock,
            schema,
            head,
            position,
            future,
            seek_index: None,
            base: None,
        };
        Ok((timeline, snapshot.map(|snapshot| snapshot.state)))
    }

    pub fn cursor(&self) -> Option<u64> {
        self.position.map(|frame| frame.id)
    }

    pub fn position(&self) -> Option<Frame> {
        self.position
    }

    pub fn tip_position(&self) -> io::Result<Option<Frame>> {
        let Some(head) = &self.head else {
            return Ok(None);
        };
        if head.cursor == head.tip {
            Ok(self.position)
        } else {
            read_frame(&self.directory, head.tip).map(Some)
        }
    }

    pub fn is_paused(&self) -> bool {
        self.head.as_ref().is_some_and(|head| head.paused)
    }

    /// Publishes an exact full checkpoint (used at handoff). Paused state is already committed.
    pub fn checkpoint<S: Serialize>(&mut self, state: &S) -> io::Result<()> {
        self.checkpoint_at(state, self.position.map_or(0, |frame| frame.at_ms))
    }

    pub fn checkpoint_at<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        if !self.is_paused() {
            self.append(state, at_ms, false, true)?;
        }
        sync_directory(&self.directory)
    }

    /// Records a timed sample, using deltas between full snapshots.
    pub fn capture_at<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        if !self.is_paused() {
            self.append(state, at_ms, false, false)?;
            sync_directory(&self.directory)?;
        }
        Ok(())
    }

    pub fn pause<S: Serialize>(&mut self, state: &S) -> io::Result<()> {
        self.pause_at(state, self.position.map_or(0, |frame| frame.at_ms))
    }

    pub fn pause_at<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        if !self.is_paused() {
            self.append(state, at_ms, true, true)?;
            sync_directory(&self.directory)?;
        }
        Ok(())
    }

    pub fn pause_at_start<S: DeserializeOwned>(&mut self) -> io::Result<S> {
        let tip = self
            .head
            .as_ref()
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?
            .tip;
        let mut frames = backward_frames(&self.directory, tip, None)?;
        let first = frames
            .pop()
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?;
        let state = self.select(first)?;
        self.future = Some(frames);
        Ok(state)
    }

    pub fn rewind<S: DeserializeOwned>(&mut self) -> io::Result<Option<S>> {
        if !self.is_paused() {
            return Ok(None);
        }
        let current = self.position.expect("paused timeline has a checkpoint");
        let link: CheckpointLink = read_checkpoint(&self.directory, current.id)?;
        let Some(previous) = previous_id(link.previous, current.id)? else {
            return Ok(None);
        };
        let frame = read_frame(&self.directory, previous)?;
        let state = self.select(frame)?;
        if let Some(future) = &mut self.future {
            future.push(current);
        }
        Ok(Some(state))
    }

    /// Selects the last recorded sample at or before the requested time without branching.
    pub fn seek<S: DeserializeOwned>(&mut self, at_ms: u64) -> io::Result<Option<S>> {
        if !self.is_paused() {
            return Ok(None);
        }
        if self.seek_index.is_none() {
            let tip = self.head.as_ref().expect("paused timeline has a head").tip;
            let mut frames = backward_frames(&self.directory, tip, None)?;
            frames.reverse();
            self.seek_index = Some(frames);
        }
        let frames = self.seek_index.as_ref().expect("seek index loaded");
        let index = frames
            .partition_point(|frame| frame.at_ms <= at_ms)
            .saturating_sub(1);
        let frame = frames[index];
        if self.position == Some(frame) {
            return Ok(None);
        }
        let state = self.select(frame)?;
        // Rebuild the forward cache on demand so drag events don't copy the remainder of history.
        self.future = None;
        Ok(Some(state))
    }

    pub fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        Ok(self.future()?.last().copied())
    }

    pub fn forward<S: DeserializeOwned>(&mut self) -> io::Result<Option<S>> {
        self.advance_where(|_| true, true)
    }

    /// Advances a playback cursor through snapshots due at the given recorded time.
    pub fn play_until<S: DeserializeOwned>(&mut self, at_ms: u64) -> io::Result<Option<S>> {
        self.advance_where(|frame| frame.at_ms <= at_ms, false)
    }

    fn advance_where<S: DeserializeOwned>(
        &mut self,
        due: impl Fn(&Frame) -> bool,
        one: bool,
    ) -> io::Result<Option<S>> {
        if !self.is_paused() {
            return Ok(None);
        }
        let future = self.future()?;
        let count = future
            .iter()
            .rev()
            .take(if one { 1 } else { usize::MAX })
            .take_while(|frame| due(frame))
            .count();
        if count == 0 {
            return Ok(None);
        }
        let index = future.len() - count;
        let frame = future[index];
        let state = self.select(frame)?;
        self.future.as_mut().expect("future loaded").truncate(index);
        Ok(Some(state))
    }

    fn future(&mut self) -> io::Result<&[Frame]> {
        if self.future.is_none() {
            let head = self.head.as_ref().expect("future requires a head");
            self.future = Some(if let Some(frames) = &self.seek_index {
                let next = frames.partition_point(|frame| frame.id <= head.cursor);
                frames[next..].iter().rev().copied().collect()
            } else {
                backward_frames(&self.directory, head.tip, Some(head.cursor))?
            });
        }
        Ok(self.future.as_deref().expect("future loaded"))
    }

    fn select<S: DeserializeOwned>(&mut self, frame: Frame) -> io::Result<S> {
        let snapshot: Snapshot<S> = read_snapshot(&self.directory, frame.id)?;
        let mut head = self.head.clone().expect("selection requires a head");
        head.cursor = frame.id;
        head.paused = true;
        self.commit_head(head)?;
        self.position = Some(frame);
        if let Err(err) = sync_directory(&self.directory) {
            eprintln!("could not sync timeline cursor: {err}");
        }
        Ok(snapshot.state)
    }

    /// Makes the selected checkpoint the live tip before the game advances from it.
    pub fn resume(&mut self) -> io::Result<()> {
        let Some(mut head) = self.head.clone().filter(|head| head.paused) else {
            return Ok(());
        };
        let abandoned_tip = head.tip;
        head.paused = false;
        head.tip = head.cursor;
        let selected = head.cursor;
        self.commit_head(head)?;
        self.future = Some(Vec::new());
        self.seek_index = None;
        self.base = None;
        sync_directory(&self.directory)?;
        // The committed head already excludes these checkpoints. Cleanup is best effort.
        let mut abandoned = abandoned_tip;
        while abandoned > selected {
            let Ok(link) = read_checkpoint::<CheckpointLink>(&self.directory, abandoned) else {
                break;
            };
            let Some(previous) = link.previous.filter(|&id| id < abandoned && id >= selected)
            else {
                break;
            };
            let _ = fs::remove_file(checkpoint_path(&self.directory, abandoned));
            abandoned = previous;
        }
        Ok(())
    }

    fn append<S: Serialize>(
        &mut self,
        state: &S,
        at_ms: u64,
        paused: bool,
        force: bool,
    ) -> io::Result<()> {
        let mut head = self.head.clone().unwrap_or_else(|| Head {
            format: FORMAT_VERSION,
            game: self.schema.game.to_owned(),
            schema: self.schema.version,
            next_id: 0,
            tip: 0,
            cursor: 0,
            paused: false,
        });
        let id = head.next_id;
        head.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        let previous = self.head.as_ref().map(|head| head.tip);
        let at_ms = at_ms.max(self.position.map_or(0, |frame| frame.at_ms));
        let state = serde_json::to_value(state)?;
        let (payload, keyframe_at, deltas) = match &self.base {
            Some(base)
                if !force
                    && base.deltas < MAX_DELTAS
                    && at_ms.saturating_sub(base.keyframe_at) < KEYFRAME_INTERVAL_MS =>
            {
                (
                    Payload::Delta {
                        delta: Delta::between(&base.state, &state),
                    },
                    base.keyframe_at,
                    base.deltas + 1,
                )
            }
            _ => (
                Payload::Full {
                    state: state.clone(),
                },
                at_ms,
                0,
            ),
        };
        let checkpoint = Checkpoint {
            previous,
            at_ms: Some(at_ms),
            payload,
        };
        write_json_atomic(&checkpoint_path(&self.directory, id), &checkpoint)?;
        sync_directory(&self.directory)?;
        head.format = FORMAT_VERSION;
        head.tip = id;
        head.cursor = id;
        head.paused = paused;
        self.commit_head(head)?;
        self.position = Some(Frame { id, at_ms });
        self.seek_index = None;
        self.base = Some(CaptureBase {
            state,
            keyframe_at,
            deltas,
        });
        Ok(())
    }

    fn commit_head(&mut self, head: Head) -> io::Result<()> {
        write_json_atomic(&self.directory.join("head.json"), &head)?;
        self.head = Some(head);
        Ok(())
    }
}

/// A stable, read-only view of a committed timeline, shared by tests and export.
pub struct TimelineReader {
    directory: PathBuf,
    _reader_lock: File,
    frames: Vec<Frame>,
}

impl TimelineReader {
    pub fn open(directory: impl Into<PathBuf>, schema: StateSchema) -> io::Result<Self> {
        let directory = directory.into();
        let lock = File::open(directory.join("writer.lock"))?;
        lock.try_lock_shared().map_err(|err| {
            io::Error::other(format!(
                "cannot read timeline {} while it is open for writing: {err}",
                directory.display()
            ))
        })?;
        let head = read_head(&directory, schema)?;
        let mut frames = backward_frames(&directory, head.tip, None)?;
        frames.reverse();
        Ok(Self {
            directory,
            _reader_lock: lock,
            frames,
        })
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    pub fn load<S: DeserializeOwned>(&self, id: u64) -> io::Result<Snapshot<S>> {
        self.frames
            .binary_search_by_key(&id, |frame| frame.id)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "checkpoint is outside the committed history",
                )
            })?;
        read_snapshot(&self.directory, id)
    }

    pub fn snapshots<S: DeserializeOwned>(
        &self,
    ) -> impl Iterator<Item = io::Result<Snapshot<S>>> + '_ {
        let mut value = None;
        self.frames.iter().map(move |&frame| {
            let checkpoint: Checkpoint = read_checkpoint(&self.directory, frame.id)?;
            match checkpoint.payload {
                Payload::Full { state } => value = Some(state),
                Payload::Delta { delta } => {
                    let mut current = value
                        .take()
                        .ok_or_else(|| invalid_data("delta has no keyframe"))?;
                    delta.apply(&mut current)?;
                    value = Some(current);
                }
            }
            let state = S::deserialize(value.as_ref().expect("snapshot reconstructed"))?;
            Ok(Snapshot { frame, state })
        })
    }
}

fn read_head(directory: &Path, schema: StateSchema) -> io::Result<Head> {
    let file = File::open(directory.join("head.json"))?;
    let head: Head = serde_json::from_reader(BufReader::new(file)).map_err(|err| {
        invalid_data(format!(
            "cannot read timeline {}: {err}. Use --fresh to start a separate timeline",
            directory.display()
        ))
    })?;
    if !matches!(head.format, 1 | FORMAT_VERSION)
        || head.game != schema.game
        || head.schema != schema.version
    {
        return Err(invalid_data(format!(
            "incompatible timeline {} ({} schema {}, format {}, expected {} schema {}). Start explicitly with --fresh or choose a new --timeline path",
            directory.display(),
            head.game,
            head.schema,
            head.format,
            schema.game,
            schema.version,
        )));
    }
    if head.cursor > head.tip
        || head.tip >= head.next_id
        || (!head.paused && head.cursor != head.tip)
    {
        return Err(invalid_data("invalid timeline cursor"));
    }
    Ok(head)
}

fn previous_id(previous: Option<u64>, id: u64) -> io::Result<Option<u64>> {
    if previous.is_some_and(|previous| previous >= id) {
        return Err(invalid_data("invalid previous checkpoint ID"));
    }
    Ok(previous)
}

fn backward_frames(directory: &Path, tip: u64, stop: Option<u64>) -> io::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    let mut next = Some(tip);
    while let Some(id) = next {
        if Some(id) == stop {
            return Ok(frames);
        }
        let link: CheckpointLink = read_checkpoint(directory, id)?;
        let frame = frame_at(id, link.at_ms);
        if frames
            .last()
            .is_some_and(|next: &Frame| next.at_ms < frame.at_ms)
        {
            return Err(invalid_data("timeline timestamps run backwards"));
        }
        frames.push(frame);
        next = previous_id(link.previous, id)?;
    }
    if stop.is_some() {
        return Err(invalid_data("timeline cursor is outside committed history"));
    }
    Ok(frames)
}

fn frame_at(id: u64, at_ms: Option<u64>) -> Frame {
    Frame {
        id,
        at_ms: at_ms.unwrap_or_else(|| id.saturating_mul(LEGACY_INTERVAL_MS)),
    }
}

fn read_frame(directory: &Path, id: u64) -> io::Result<Frame> {
    let link: CheckpointLink = read_checkpoint(directory, id)?;
    Ok(frame_at(id, link.at_ms))
}

fn read_snapshot<S: DeserializeOwned>(directory: &Path, id: u64) -> io::Result<Snapshot<S>> {
    let mut checkpoint: Checkpoint = read_checkpoint(directory, id)?;
    let frame = frame_at(id, checkpoint.at_ms);
    let mut deltas = Vec::new();
    let mut current = id;
    let mut value = loop {
        match checkpoint.payload {
            Payload::Full { state } => break state,
            Payload::Delta { delta } => {
                if deltas.len() >= usize::from(MAX_DELTAS) {
                    return Err(invalid_data("delta chain is missing a keyframe"));
                }
                deltas.push(delta);
                current = previous_id(checkpoint.previous, current)?
                    .ok_or_else(|| invalid_data("delta has no previous checkpoint"))?;
                checkpoint = read_checkpoint(directory, current)?;
            }
        }
    };
    for delta in deltas.into_iter().rev() {
        delta.apply(&mut value)?;
    }
    Ok(Snapshot {
        frame,
        state: serde_json::from_value(value)?,
    })
}

fn checkpoint_path(directory: &Path, id: u64) -> PathBuf {
    directory.join(format!("{id:020}.json"))
}

fn read_checkpoint<S: DeserializeOwned>(directory: &Path, id: u64) -> io::Result<S> {
    let file = File::open(checkpoint_path(directory, id))?;
    Ok(serde_json::from_reader(BufReader::new(file))?)
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn write_json_atomic(path: &Path, content: &impl Serialize) -> io::Result<()> {
    let temporary = path.with_extension("tmp");
    let mut writer = BufWriter::new(File::create(&temporary)?);
    serde_json::to_writer(&mut writer, content)?;
    writer.flush()?;
    writer.get_ref().sync_all()?;
    drop(writer);
    fs::rename(temporary, path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
