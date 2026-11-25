//! An external, versioned state timeline. `head.json` is the commit point.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

const FORMAT_VERSION: u32 = 3;
const KEYFRAME_INTERVAL_MS: u64 = 1_000;
const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const CAPTURE_QUEUE: usize = 64;

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
    TogglePlayPause,
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

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    previous: Option<u64>,
    at_ms: u64,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Deserialize)]
struct CheckpointLink {
    previous: Option<u64>,
    at_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Payload {
    Full { state: Value },
    Delta { delta: Delta },
}

impl Payload {
    fn restore(self, previous: Option<Value>) -> io::Result<Value> {
        match self {
            Self::Full { state } => Ok(state),
            Self::Delta { delta } => {
                let mut state = previous.ok_or_else(|| invalid_data("delta has no keyframe"))?;
                delta.apply(&mut state)?;
                Ok(state)
            }
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
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
}

pub struct Timeline {
    checkpoints: Checkpoints,
    _writer_lock: File,
    schema: StateSchema,
    head: Option<Head>,
    position: Option<Frame>,
    future: Option<Vec<Frame>>,
    seek_index: Option<Vec<Frame>>,
    selected_value: Option<Value>,
    base: Option<CaptureBase>,
    batch: Vec<Checkpoint>,
}

type Control = Box<dyn FnOnce(&mut Timeline, io::Result<()>) + Send>;

enum TimelineJob<S> {
    Capture(S, u64),
    Control(Control),
}

/// The bounded queue preserves every simulation tick in order, including during a slow flush.
pub(crate) struct TimelineWorker<S> {
    jobs: SyncSender<TimelineJob<S>>,
    commits: Receiver<io::Result<Frame>>,
}

impl<S: Serialize + Send + 'static> TimelineWorker<S> {
    pub(crate) fn start(mut timeline: Timeline) -> io::Result<Self> {
        let (sender, jobs) = mpsc::sync_channel(CAPTURE_QUEUE);
        let (committed, commits) = mpsc::channel();
        std::thread::Builder::new()
            .name("sycho-timeline".into())
            .spawn(move || {
                let mut deadline = None::<Instant>;
                loop {
                    let job = if let Some(at) = deadline {
                        let remaining = at.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            Err(RecvTimeoutError::Timeout)
                        } else {
                            jobs.recv_timeout(remaining)
                        }
                    } else {
                        jobs.recv().map_err(|_| RecvTimeoutError::Disconnected)
                    };
                    match job {
                        Ok(TimelineJob::Capture(state, at_ms)) => {
                            if let Err(err) = timeline.stage(&state, at_ms) {
                                let _ = committed.send(Err(err));
                                break;
                            }
                            deadline.get_or_insert_with(|| Instant::now() + FLUSH_INTERVAL);
                        }
                        Ok(TimelineJob::Control(operation)) => {
                            let flushed = timeline.flush();
                            let failed = flushed.is_err();
                            operation(&mut timeline, flushed);
                            if failed {
                                break;
                            }
                            deadline = None;
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            deadline = None;
                            match timeline.flush() {
                                Ok(()) => {
                                    if let Some(frame) = timeline.position() {
                                        let _ = committed.send(Ok(frame));
                                    }
                                }
                                Err(err) => {
                                    let _ = committed.send(Err(err));
                                    break;
                                }
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            if let Err(err) = timeline.flush() {
                                eprintln!("could not flush timeline: {err}");
                            }
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            jobs: sender,
            commits,
        })
    }

    pub(crate) fn capture(&self, state: S, at_ms: u64) -> io::Result<()> {
        self.send(TimelineJob::Capture(state, at_ms))
    }

    pub(crate) fn commits(&self) -> impl Iterator<Item = io::Result<Frame>> + '_ {
        self.commits.try_iter()
    }

    pub(crate) fn request<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Timeline) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        let (reply, result) = mpsc::channel();
        self.send(TimelineJob::Control(Box::new(move |timeline, flushed| {
            let _ = reply.send(flushed.and_then(|()| operation(timeline)));
        })))?;
        result
            .recv()
            .map_err(|_| io::Error::other("timeline writer stopped"))?
    }

    fn send(&self, job: TimelineJob<S>) -> io::Result<()> {
        self.jobs
            .send(job)
            .map_err(|_| io::Error::other("timeline writer stopped"))
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
        let checkpoints = Checkpoints::open(directory, head.as_ref(), true)?;
        let snapshot: Option<Snapshot<S>> = head.as_ref().map(|head| {
            checkpoints.snapshot(head.cursor).map_err(|err| {
                io::Error::new(err.kind(), format!(
                    "cannot restore checkpoint {} in {}: {err}. Use --fresh to start a separate timeline",
                    head.cursor, checkpoints.directory.display(),
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
            checkpoints,
            _writer_lock: lock,
            schema,
            head,
            position,
            future,
            seek_index: None,
            selected_value: None,
            base: None,
            batch: Vec::new(),
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
            self.checkpoints.frame(head.tip).map(Some)
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
        if self.is_paused() {
            return sync_directory(&self.checkpoints.directory);
        }
        self.append(state, at_ms, false, true)
    }

    /// Records a timed sample, using deltas between full snapshots.
    pub fn capture_at<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        if !self.is_paused() {
            self.append(state, at_ms, false, false)?;
        }
        Ok(())
    }

    pub fn pause<S: Serialize>(&mut self, state: &S) -> io::Result<()> {
        self.pause_at(state, self.position.map_or(0, |frame| frame.at_ms))
    }

    pub fn pause_at<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        if !self.is_paused() {
            self.append(state, at_ms, true, true)?;
        }
        Ok(())
    }

    pub fn pause_at_start<S: DeserializeOwned>(&mut self) -> io::Result<S> {
        let tip = self
            .head
            .as_ref()
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?
            .tip;
        let mut frames = self.checkpoints.backward_frames(tip, None)?;
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
        let link: CheckpointLink = self.checkpoints.link(current.id)?;
        let Some(previous) = previous_id(link.previous, current.id)? else {
            return Ok(None);
        };
        let frame = self.checkpoints.frame(previous)?;
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
            let mut frames = self.checkpoints.backward_frames(tip, None)?;
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
        let state = if let Some(mut state) = self.selected_value.take() {
            let future = self.future.as_ref().expect("future loaded");
            for next in future[index..].iter().rev() {
                state = self
                    .checkpoints
                    .checkpoint(next.id)?
                    .payload
                    .restore(Some(state))?;
            }
            self.select_value(frame, state)?
        } else {
            self.select(frame)?
        };
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
                self.checkpoints
                    .backward_frames(head.tip, Some(head.cursor))?
            });
        }
        Ok(self.future.as_deref().expect("future loaded"))
    }

    fn select<S: DeserializeOwned>(&mut self, frame: Frame) -> io::Result<S> {
        let (_, state) = self.checkpoints.snapshot_value(frame.id)?;
        self.select_value(frame, state)
    }

    fn select_value<S: DeserializeOwned>(&mut self, frame: Frame, value: Value) -> io::Result<S> {
        let state = S::deserialize(&value)?;
        let mut head = self.head.clone().expect("selection requires a head");
        head.cursor = frame.id;
        head.paused = true;
        self.commit_head(head)?;
        self.position = Some(frame);
        self.selected_value = Some(value);
        if let Err(err) = sync_directory(&self.checkpoints.directory) {
            eprintln!("could not sync timeline cursor: {err}");
        }
        Ok(state)
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
        self.selected_value = None;
        sync_directory(&self.checkpoints.directory)?;
        // The committed head already excludes these checkpoints. Cleanup is best effort.
        let mut abandoned = abandoned_tip;
        while abandoned > selected {
            let Ok(link) = self.checkpoints.link(abandoned) else {
                break;
            };
            let Some(previous) = link.previous.filter(|&id| id < abandoned && id >= selected)
            else {
                break;
            };
            let _ = fs::remove_file(checkpoint_path(&self.checkpoints.directory, abandoned));
            abandoned = previous;
        }
        self.checkpoints.discard_batches_after(selected);
        Ok(())
    }

    fn append<S: Serialize>(
        &mut self,
        state: &S,
        at_ms: u64,
        paused: bool,
        force: bool,
    ) -> io::Result<()> {
        self.flush()?;
        let id = self.next_id();
        let previous = self.head.as_ref().map(|head| head.tip);
        let at_ms = at_ms.max(self.position.map_or(0, |frame| frame.at_ms));
        let (checkpoint, base) = self.encode(state, previous, at_ms, force)?;
        let directory = &self.checkpoints.directory;
        write_json_atomic(&checkpoint_path(directory, id), &checkpoint)?;
        sync_directory(directory)?;
        self.publish(id, at_ms, paused)?;
        self.base = Some(base);
        sync_directory(&self.checkpoints.directory)
    }

    fn stage<S: Serialize>(&mut self, state: &S, at_ms: u64) -> io::Result<()> {
        let previous = if self.batch.is_empty() {
            self.head.as_ref().map(|head| head.tip)
        } else {
            Some(
                self.next_id()
                    .checked_add(self.batch.len() as u64 - 1)
                    .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?,
            )
        };
        let previous_time = self
            .batch
            .last()
            .map(|frame| frame.at_ms)
            .or(self.position.map(|frame| frame.at_ms))
            .unwrap_or(0);
        let (checkpoint, base) = self.encode(state, previous, at_ms.max(previous_time), false)?;
        self.batch.push(checkpoint);
        self.base = Some(base);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        let Some(last) = self.batch.last() else {
            return Ok(());
        };
        let first = self.next_id();
        let tip = first
            .checked_add(self.batch.len() as u64 - 1)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        let at_ms = last.at_ms;
        let directory = &self.checkpoints.directory;
        write_json_atomic(&batch_path(directory, first), &self.batch)?;
        sync_directory(directory)?;
        self.publish(tip, at_ms, false)?;
        self.checkpoints
            .remember_batch(first, std::mem::take(&mut self.batch));
        sync_directory(&self.checkpoints.directory)
    }

    fn encode<S: Serialize>(
        &self,
        state: &S,
        previous: Option<u64>,
        at_ms: u64,
        force: bool,
    ) -> io::Result<(Checkpoint, CaptureBase)> {
        let state = serde_json::to_value(state)?;
        let (payload, keyframe_at) = match &self.base {
            Some(base)
                if !force && at_ms.saturating_sub(base.keyframe_at) < KEYFRAME_INTERVAL_MS =>
            {
                (
                    Payload::Delta {
                        delta: Delta::between(&base.state, &state),
                    },
                    base.keyframe_at,
                )
            }
            _ => (
                Payload::Full {
                    state: state.clone(),
                },
                at_ms,
            ),
        };
        Ok((
            Checkpoint {
                previous,
                at_ms,
                payload,
            },
            CaptureBase { state, keyframe_at },
        ))
    }

    fn next_id(&self) -> u64 {
        self.head.as_ref().map_or(0, |head| head.next_id)
    }

    fn publish(&mut self, id: u64, at_ms: u64, paused: bool) -> io::Result<()> {
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        self.commit_head(Head {
            format: FORMAT_VERSION,
            game: self.schema.game.to_owned(),
            schema: self.schema.version,
            next_id,
            tip: id,
            cursor: id,
            paused,
        })?;
        self.position = Some(Frame { id, at_ms });
        self.seek_index = None;
        self.selected_value = None;
        Ok(())
    }

    fn commit_head(&mut self, head: Head) -> io::Result<()> {
        write_json_atomic(&self.checkpoints.directory.join("head.json"), &head)?;
        self.head = Some(head);
        Ok(())
    }
}

/// A stable, read-only view of a committed timeline, shared by tests and export.
pub struct TimelineReader {
    checkpoints: Checkpoints,
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
        let checkpoints = Checkpoints::open(directory, Some(&head), false)?;
        let mut frames = checkpoints.backward_frames(head.tip, None)?;
        frames.reverse();
        Ok(Self {
            checkpoints,
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
        self.checkpoints.snapshot(id)
    }

    pub fn snapshots<S: DeserializeOwned>(
        &self,
    ) -> impl Iterator<Item = io::Result<Snapshot<S>>> + '_ {
        let mut value = None;
        self.frames.iter().map(move |&frame| {
            let checkpoint: Checkpoint = self.checkpoints.checkpoint(frame.id)?;
            value = Some(checkpoint.payload.restore(value.take())?);
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
    if head.format != FORMAT_VERSION || head.game != schema.game || head.schema != schema.version {
        return Err(invalid_data(format!(
            "incompatible timeline {} ({} schema {}, format {}, expected {} schema {}, format {}). Start explicitly with --fresh or choose a new --timeline path",
            directory.display(),
            head.game,
            head.schema,
            head.format,
            schema.game,
            schema.version,
            FORMAT_VERSION,
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

/// Batches are indexed by their first checkpoint ID. Single checkpoint files
/// are used by synchronous captures and exact pause and handoff states.
struct Checkpoints {
    directory: PathBuf,
    batches: BTreeSet<u64>,
    cached_batch: RefCell<Option<(u64, Vec<Checkpoint>)>>,
}

impl Checkpoints {
    fn open(directory: PathBuf, head: Option<&Head>, writer: bool) -> io::Result<Self> {
        let mut batches = BTreeSet::new();
        let next_id = head.map_or(0, |head| head.next_id);
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(first) = name
                .to_str()
                .and_then(|name| name.strip_prefix("batch-"))
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|id| id.len() == 20)
                .and_then(|id| id.parse::<u64>().ok())
            else {
                continue;
            };
            if first < next_id {
                batches.insert(first);
            } else if writer {
                // An interrupted publish can leave this ID free for a future checkpoint.
                fs::remove_file(entry.path())?;
            }
        }
        Ok(Self {
            directory,
            batches,
            cached_batch: RefCell::new(None),
        })
    }

    fn remember_batch(&mut self, first: u64, frames: Vec<Checkpoint>) {
        self.batches.insert(first);
        self.cached_batch.replace(Some((first, frames)));
    }

    fn discard_batches_after(&mut self, selected: u64) {
        let removed = self.batches.split_off(&selected.saturating_add(1));
        self.cached_batch.replace(None);
        for first in removed {
            let _ = fs::remove_file(batch_path(&self.directory, first));
        }
    }

    fn from_batch<T>(&self, id: u64, read: impl FnOnce(&Checkpoint) -> T) -> io::Result<Option<T>> {
        let Some(&first) = self.batches.range(..=id).next_back() else {
            return Ok(None);
        };
        let mut cache = self.cached_batch.borrow_mut();
        if cache.as_ref().is_none_or(|(cached, _)| *cached != first) {
            let file = File::open(batch_path(&self.directory, first))?;
            let frames = serde_json::from_reader(BufReader::new(file))?;
            *cache = Some((first, frames));
        }
        let frames = &cache.as_ref().expect("batch loaded").1;
        Ok(usize::try_from(id - first)
            .ok()
            .and_then(|index| frames.get(index))
            .map(read))
    }

    fn checkpoint(&self, id: u64) -> io::Result<Checkpoint> {
        match self.from_batch(id, Checkpoint::clone)? {
            Some(checkpoint) => Ok(checkpoint),
            None => read_checkpoint(&self.directory, id),
        }
    }

    fn link(&self, id: u64) -> io::Result<CheckpointLink> {
        match self.from_batch(id, |checkpoint| CheckpointLink {
            previous: checkpoint.previous,
            at_ms: checkpoint.at_ms,
        })? {
            Some(link) => Ok(link),
            None => read_checkpoint(&self.directory, id),
        }
    }

    fn frame(&self, id: u64) -> io::Result<Frame> {
        Ok(Frame {
            id,
            at_ms: self.link(id)?.at_ms,
        })
    }

    fn backward_frames(&self, tip: u64, stop: Option<u64>) -> io::Result<Vec<Frame>> {
        let mut frames = Vec::new();
        let mut next = Some(tip);
        while let Some(id) = next {
            if Some(id) == stop {
                return Ok(frames);
            }
            let link: CheckpointLink = self.link(id)?;
            let frame = Frame {
                id,
                at_ms: link.at_ms,
            };
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

    fn snapshot<S: DeserializeOwned>(&self, id: u64) -> io::Result<Snapshot<S>> {
        let (frame, value) = self.snapshot_value(id)?;
        Ok(Snapshot {
            frame,
            state: serde_json::from_value(value)?,
        })
    }

    fn snapshot_value(&self, id: u64) -> io::Result<(Frame, Value)> {
        let mut checkpoint: Checkpoint = self.checkpoint(id)?;
        let frame = Frame {
            id,
            at_ms: checkpoint.at_ms,
        };
        let mut deltas = Vec::new();
        let mut current = id;
        let mut value = loop {
            match checkpoint.payload {
                Payload::Full { state } => break state,
                Payload::Delta { delta } => {
                    deltas.push(delta);
                    current = previous_id(checkpoint.previous, current)?
                        .ok_or_else(|| invalid_data("delta has no previous checkpoint"))?;
                    checkpoint = self.checkpoint(current)?;
                }
            }
        };
        for delta in deltas.into_iter().rev() {
            delta.apply(&mut value)?;
        }
        Ok((frame, value))
    }
}

fn batch_path(directory: &Path, first: u64) -> PathBuf {
    directory.join(format!("batch-{first:020}.json"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
            Self(std::env::temp_dir().join(format!(
                "sycho-batches-{}-{}",
                std::process::id(),
                stamp.as_nanos()
            )))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const SCHEMA: StateSchema = StateSchema {
        game: "test",
        version: 1,
    };

    #[test]
    fn writer_flushes_when_idle_and_keeps_every_tick_between_keyframes() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<Value>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&json!({"tick": 0}), 0).unwrap();
        let writer = TimelineWorker::start(timeline).unwrap();

        writer.capture(json!({"tick": 1}), 16).unwrap();
        let first = writer
            .commits
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert_eq!(first, Frame { id: 1, at_ms: 16 });

        for tick in 2..=72 {
            writer
                .capture(
                    json!({"tick": tick, "pressed": tick == 17}),
                    tick * 1000 / 60,
                )
                .unwrap();
        }

        writer
            .request(|timeline| {
                assert_eq!(timeline.position().unwrap().id, 72);
                assert!(matches!(
                    timeline.checkpoints.checkpoint(59)?.payload,
                    Payload::Delta { .. }
                ));
                assert!(matches!(
                    timeline.checkpoints.checkpoint(60)?.payload,
                    Payload::Full { .. }
                ));
                timeline.pause_at(&json!({"tick": 72}), 1201)?;
                let tap = timeline.seek::<Value>(17 * 1000 / 60)?.unwrap();
                assert_eq!(tap, json!({"tick": 17, "pressed": true}));
                let next = timeline.forward::<Value>()?.unwrap();
                assert_eq!(next, json!({"tick": 18, "pressed": false}));
                assert_eq!(timeline.tip_position()?.unwrap().id, 73);
                timeline.resume()?;
                timeline.stage(&json!({"branch": true}), 310)?;
                timeline.flush()?;
                assert_eq!(timeline.position(), Some(Frame { id: 74, at_ms: 310 }));
                let mut ids = timeline.checkpoints.backward_frames(74, None)?;
                ids.reverse();
                assert_eq!(
                    ids.into_iter().map(|frame| frame.id).collect::<Vec<_>>(),
                    (0..=18).chain([74]).collect::<Vec<_>>()
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn recovery_ignores_an_unpublished_batch_even_when_reusing_its_id_for_a_full_checkpoint() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&0u32, 0).unwrap();
        timeline.stage(&1u32, 16).unwrap();
        timeline.stage(&2u32, 33).unwrap();
        timeline.flush().unwrap();

        timeline.stage(&99u32, 50).unwrap();
        write_json_atomic(&batch_path(&directory.0, 3), &timeline.batch).unwrap();
        fs::write(directory.0.join("head.tmp"), b"{").unwrap();
        drop(timeline);
        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        assert_eq!(
            reader
                .snapshots::<u32>()
                .map(|item| item.unwrap().state)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
        drop(reader);

        let (mut timeline, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        assert_eq!(restored, Some(2));
        assert!(!batch_path(&directory.0, 3).exists());
        timeline.checkpoint_at(&3u32, 50).unwrap(); // Exact hot-reload handoff.
        drop(timeline);
        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        assert_eq!(reader.load::<u32>(3).unwrap().state, 3);
    }
}
