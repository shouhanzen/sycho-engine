//! An external, versioned state timeline. `head.json` is the commit point.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::persistence::{JsonWriteOptions, write_json_atomic};

mod bookmarks;
mod fork;
pub use fork::ForkState;
mod retention;
pub use bookmarks::{Bookmark, BookmarkEdit, BookmarkLedger};
use retention::{Archive, ArchivedCheckpoints, Archiver};

const FORMAT_VERSION: u32 = 4;
const PREVIOUS_FORMAT_VERSION: u32 = 3;
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
    OpenBookmark(u64),
    ReturnFromBookmark,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reset_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retention_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archive: Option<Archive>,
    #[serde(default)]
    branch: u64,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    detached_bookmarks: BTreeSet<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bookmark_links: BTreeMap<u64, u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preview_bookmark: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    previous: Option<u64>,
    at_ms: u64,
    #[serde(flatten)]
    payload: Payload,
}

#[derive(Clone, Copy, Deserialize)]
struct CheckpointLink {
    previous: Option<u64>,
    at_ms: u64,
}

impl From<&Checkpoint> for CheckpointLink {
    fn from(checkpoint: &Checkpoint) -> Self {
        Self {
            previous: checkpoint.previous,
            at_ms: checkpoint.at_ms,
        }
    }
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

struct SeekIndexJob {
    tip: u64,
    branch: u64,
    archive: Option<Archive>,
    frames: std::thread::JoinHandle<io::Result<Vec<Frame>>>,
}

pub struct Timeline {
    checkpoints: Checkpoints,
    _writer_lock: File,
    schema: StateSchema,
    head: Option<Head>,
    position: Option<Frame>,
    future: Option<Vec<Frame>>,
    seek_index: Option<Vec<Frame>>,
    seek_index_job: Option<SeekIndexJob>,
    selected_value: Option<Value>,
    base: Option<CaptureBase>,
    batch: Vec<Checkpoint>,
    retention_start: Option<u64>,
    bookmarks: BookmarkLedger,
}

type Control = Box<dyn FnOnce(&mut Timeline, io::Result<()>) + Send>;

enum TimelineJob<S> {
    Capture(Arc<S>, u64),
    Control(Control),
    Reset(S, mpsc::Sender<io::Result<Frame>>),
}

/// The bounded queue preserves every simulation tick in order, including during a slow flush.
pub(crate) struct TimelineWorker<S> {
    jobs: SyncSender<TimelineJob<S>>,
    commits: Receiver<io::Result<Frame>>,
}

impl<S: Serialize + Send + Sync + 'static> TimelineWorker<S> {
    pub(crate) fn start(mut timeline: Timeline) -> io::Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<TimelineJob<S>>(CAPTURE_QUEUE);
        let (committed, commits) = mpsc::channel();
        std::thread::Builder::new()
            .name("sycho-timeline".into())
            .spawn(move || {
                let mut deadline = None::<Instant>;
                let mut capture_failed = false;
                let mut archiver = Archiver::default();
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
                            if capture_failed {
                                continue;
                            }
                            if let Err(err) = timeline.stage(state.as_ref(), at_ms) {
                                let _ = committed.send(Err(err));
                                capture_failed = true;
                                continue;
                            }
                            deadline.get_or_insert_with(|| Instant::now() + FLUSH_INTERVAL);
                        }
                        Ok(TimelineJob::Control(operation)) => {
                            let flushed = timeline.flush();
                            timeline.adopt_ready_seek_index();
                            operation(&mut timeline, flushed);
                            deadline = None;
                        }
                        Ok(TimelineJob::Reset(state, reply)) => {
                            let result = timeline.reset(&state);
                            if result.is_ok() {
                                deadline = None;
                                capture_failed = false;
                            }
                            let _ = reply.send(result);
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            deadline = None;
                            match timeline.flush() {
                                Ok(()) => {
                                    timeline.adopt_ready_seek_index();
                                    if let Err(err) = archiver.after_flush(&mut timeline) {
                                        eprintln!("could not compact timeline: {err}");
                                    }
                                    if let Some(frame) = timeline.position() {
                                        let _ = committed.send(Ok(frame));
                                    }
                                }
                                Err(err) => {
                                    let _ = committed.send(Err(err));
                                    // Keep the pending batch and accept explicit checkpoint retries.
                                    capture_failed = true;
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

    pub(crate) fn capture(&self, state: Arc<S>, at_ms: u64) -> io::Result<()> {
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

    pub(crate) fn reset(&self, snapshot: S) -> io::Result<Frame> {
        let (reply, result) = mpsc::channel();
        self.send(TimelineJob::Reset(snapshot, reply))?;
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
        let bookmarks = BookmarkLedger::read(&directory)?;
        let checkpoints = Checkpoints::open(directory, head.as_ref(), true)?;
        let snapshot: Option<Snapshot<S>> = head.as_ref().map(|head| {
            let snapshot = match head.preview_bookmark {
                Some(id) => bookmarks.get(id).and_then(|bookmark| Ok(Snapshot {
                    frame: bookmark.frame,
                    state: serde_json::from_value(BookmarkLedger::state(&checkpoints.directory, id)?)?,
                })),
                None => checkpoints.snapshot(head.cursor),
            };
            snapshot.map_err(|err| {
                io::Error::new(err.kind(), format!(
                    "cannot restore checkpoint {} in {}: {err}. Use --fresh to start a separate timeline",
                    head.cursor, checkpoints.directory.display(),
                ))
            })
        }).transpose()?;
        let position = snapshot.as_ref().map(|snapshot| snapshot.frame);
        let future = if head
            .as_ref()
            .is_some_and(|head| head.cursor != head.tip || head.preview_bookmark.is_some())
        {
            None
        } else {
            Some(Vec::new())
        };
        let retention_start = head.as_ref().and_then(|head| head.retention_start);
        let mut timeline = Self {
            checkpoints,
            _writer_lock: lock,
            schema,
            head,
            position,
            future,
            seek_index: None,
            seek_index_job: None,
            selected_value: None,
            base: None,
            batch: Vec::new(),
            retention_start,
            bookmarks,
        };
        if timeline.head.is_none()
            && let Some(seed) = fork::read_seed(&timeline.checkpoints.directory)?
        {
            seed.validate(schema)?;
            let state = S::deserialize(&seed.state)?;
            if seed.paused {
                timeline.pause_at(&seed.state, 0)?;
            } else {
                timeline.checkpoint_at(&seed.state, 0)?;
            }
            fork::remove_seed(&timeline.checkpoints.directory);
            return Ok((timeline, Some(state)));
        }
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
        if head.cursor == head.tip && head.preview_bookmark.is_none() {
            Ok(self.position)
        } else {
            self.checkpoints.frame(head.tip).map(Some)
        }
    }

    pub fn is_paused(&self) -> bool {
        self.head.as_ref().is_some_and(|head| head.paused)
    }

    /// Start a new recording at time zero. Standalone bookmarks remain in the ledger.
    pub fn reset<S: Serialize>(&mut self, state: &S) -> io::Result<Frame> {
        let pending = self.next_id();
        // A failed publish can leave an unpublished batch on disk. Never reuse
        // one of its IDs for the new root, even if the batch was not committed.
        let id = pending
            .checked_add(self.batch.len() as u64)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        let position = Frame { id, at_ms: 0 };
        let value = serde_json::to_value(state)?;
        let directory = &self.checkpoints.directory;
        write_json_atomic(
            &checkpoint_path(directory, id),
            &Checkpoint {
                previous: None,
                at_ms: 0,
                payload: Payload::Full {
                    state: value.clone(),
                },
            },
            JsonWriteOptions::COMPACT,
        )?;
        sync_directory(directory)?;

        let mut head = self.initial_head(id, next_id, false);
        head.reset_at = Some(id);
        head.retention_start = self.retention_start.map(|_| id);
        head.branch = next_id;
        head.detached_bookmarks = self
            .bookmarks
            .entries()
            .iter()
            .map(|bookmark| bookmark.id)
            .collect();
        let retention_start = head.retention_start;
        self.commit_head(head)?;

        let mut old_batches = std::mem::take(&mut self.checkpoints.batches);
        if !self.batch.is_empty() {
            old_batches.insert(pending);
        }
        let old_singles = std::mem::replace(&mut self.checkpoints.singles, BTreeSet::from([id]));
        let old_archive = self.checkpoints.archive.take();
        self.checkpoints.cached_batch.replace(None);
        self.checkpoints.cached_links.replace(None);
        self.retention_start = retention_start;
        self.batch.clear();
        self.position = Some(position);
        self.future = Some(Vec::new());
        self.seek_index = Some(vec![position]);
        self.seek_index_job = None;
        self.selected_value = None;
        self.base = Some(CaptureBase {
            state: value,
            keyframe_at: 0,
        });

        let directory = self.checkpoints.directory.clone();
        // The reset is already published. If syncing fails, leave the old
        // files intact so either head can be recovered after a crash.
        if let Err(err) = sync_directory(&directory) {
            eprintln!("could not sync timeline reset: {err}");
        } else if let Err(err) = std::thread::Builder::new()
            .name("sycho-timeline-reset".into())
            .spawn(move || {
                for first in old_batches {
                    let _ = remove_if_present(&batch_path(&directory, first));
                }
                for old in old_singles.range(..id) {
                    let _ = remove_if_present(&checkpoint_path(&directory, *old));
                }
                if let Some(archive) = old_archive {
                    let _ =
                        remove_if_present(&retention::path(&directory, archive.range.generation));
                }
            })
        {
            eprintln!("could not remove discarded timeline files: {err}");
        }
        Ok(position)
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
        let first = self
            .seek_frames()?
            .first()
            .copied()
            .ok_or_else(|| invalid_data("the timeline has no checkpoints"))?;
        let state = self.select(first)?;
        self.future = None;
        Ok(state)
    }

    pub fn rewind<S: DeserializeOwned>(&mut self) -> io::Result<Option<S>> {
        if self.detached_bookmark().is_some() {
            return self.return_from_bookmark();
        }
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
        let tip = self.tip_position()?.expect("paused timeline has a tip");
        let frame = if at_ms >= tip.at_ms {
            tip
        } else {
            let frames = self.seek_frames()?;
            let index = frames
                .partition_point(|frame| frame.at_ms <= at_ms)
                .saturating_sub(1);
            frames[index]
        };
        if self.position == Some(frame) && self.detached_bookmark().is_none() {
            return Ok(None);
        }
        let state = self.select(frame)?;
        // Rebuild the forward cache on demand so drag events don't copy the remainder of history.
        self.future = None;
        Ok(Some(state))
    }

    /// Prepare the time index during game startup so a click can use it as
    /// soon as the preview is visible. Only committed links are scanned.
    pub(crate) fn warm_seek_index(&mut self) {
        let Some(head) = &self.head else { return };
        if self.seek_index.is_some() || self.seek_index_job.is_some() {
            return;
        }
        let (tip, branch, archive) = (head.tip, head.branch, head.archive);
        let checkpoints = self.checkpoints.background_view();
        if let Ok(frames) = std::thread::Builder::new()
            .name("sycho-timeline-index".into())
            .spawn(move || checkpoints.forward_frames(tip))
        {
            self.seek_index_job = Some(SeekIndexJob {
                tip,
                branch,
                archive,
                frames,
            });
        }
    }

    fn adopt_ready_seek_index(&mut self) {
        if self
            .seek_index_job
            .as_ref()
            .is_some_and(|job| job.frames.is_finished())
        {
            self.adopt_seek_index();
        }
    }

    fn adopt_seek_index(&mut self) {
        let Some(job) = self.seek_index_job.take() else {
            return;
        };
        let Some(head) = &self.head else { return };
        if head.branch != job.branch || head.archive != job.archive {
            return;
        }
        if let Ok(Ok(mut frames)) = job.frames.join()
            && let Ok(tail) = self.checkpoints.backward_frames(head.tip, Some(job.tip))
        {
            frames.extend(tail.into_iter().rev());
            self.seek_index = Some(frames);
        }
    }

    fn seek_frames(&mut self) -> io::Result<&[Frame]> {
        self.adopt_seek_index();
        if self.seek_index.is_none()
            && let Some(head) = &self.head
        {
            self.seek_index = Some(self.checkpoints.forward_frames(head.tip)?);
        }
        Ok(self.seek_index.as_deref().unwrap_or_default())
    }

    pub fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        if self.detached_bookmark().is_some() {
            return Ok(None);
        }
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
        if !self.is_paused() || self.detached_bookmark().is_some() {
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
            self.adopt_ready_seek_index();
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
        head.preview_bookmark = None;
        self.commit_head(head)?;
        self.position = Some(frame);
        self.selected_value = Some(value);
        if let Err(err) = sync_directory(&self.checkpoints.directory) {
            eprintln!("could not sync timeline cursor: {err}");
        }
        Ok(state)
    }

    /// Resume at the latest checkpoint, preserving all recorded history.
    pub(crate) fn resume_tip<S: DeserializeOwned>(&mut self) -> io::Result<Option<S>> {
        if !self.is_paused() {
            return Ok(None);
        }
        let tip = self.tip_position()?.expect("paused timeline has a tip");
        let state = if self.position != Some(tip) || self.detached_bookmark().is_some() {
            Some(self.select(tip)?)
        } else {
            None
        };
        self.resume()?;
        Ok(state)
    }

    /// Makes the selected checkpoint the live tip before the game advances from it.
    pub fn resume(&mut self) -> io::Result<()> {
        let Some(mut head) = self.head.clone().filter(|head| head.paused) else {
            return Ok(());
        };
        let abandoned_tip = head.tip;
        let old_archive = head.archive;
        let restored_bookmark = head.preview_bookmark.take();
        let (selected, new_position) = if let Some(bookmark_id) = restored_bookmark {
            let bookmark = self.bookmarks.get(bookmark_id)?.clone();
            let state = BookmarkLedger::state(&self.checkpoints.directory, bookmark_id)?;
            let frames = self.seek_frames()?;
            let next = frames.partition_point(|frame| frame.at_ms < bookmark.frame.at_ms);
            let previous = next.checked_sub(1).map(|index| frames[index].id);
            let id = head.next_id;
            head.next_id = id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
            let checkpoint = Checkpoint {
                previous,
                at_ms: bookmark.frame.at_ms,
                payload: Payload::Full { state },
            };
            let directory = &self.checkpoints.directory;
            write_json_atomic(
                &checkpoint_path(directory, id),
                &checkpoint,
                JsonWriteOptions::COMPACT,
            )?;
            sync_directory(directory)?;
            head.cursor = id;
            (
                checkpoint.previous,
                Some(Frame {
                    id,
                    at_ms: checkpoint.at_ms,
                }),
            )
        } else {
            (Some(head.cursor), None)
        };
        head.paused = false;
        head.tip = head.cursor;
        let branching = selected != Some(abandoned_tip) || new_position.is_some();
        if branching {
            head.branch = head.next_id;
            head.detached_bookmarks.extend(
                self.bookmarks
                    .entries()
                    .iter()
                    .filter(|mark| {
                        let frame = head
                            .bookmark_links
                            .get(&mark.id)
                            .copied()
                            .unwrap_or(mark.frame.id);
                        selected.is_none_or(|id| frame > id)
                    })
                    .map(|mark| mark.id),
            );
            head.bookmark_links
                .retain(|id, _| !head.detached_bookmarks.contains(id));
            if let Some(archive) = &mut head.archive {
                match selected {
                    Some(id) if id >= archive.first => archive.through = archive.through.min(id),
                    _ => head.archive = None,
                }
            }
        }
        if let Some((id, frame)) = restored_bookmark.zip(new_position) {
            head.detached_bookmarks.remove(&id);
            head.bookmark_links.insert(id, frame.id);
        }
        let archive = head.archive;
        self.commit_head(head)?;
        if let Some(frame) = new_position {
            self.checkpoints.singles.insert(frame.id);
            self.position = Some(frame);
        }
        if let Some(checkpoints) = &mut self.checkpoints.archive {
            if let Some(archive) = archive {
                checkpoints.range = archive;
            } else {
                self.checkpoints.archive = None;
            }
        }
        self.future = Some(Vec::new());
        if let Some(frames) = &mut self.seek_index {
            if branching {
                frames.truncate(
                    frames.partition_point(|frame| selected.is_some_and(|id| frame.id <= id)),
                );
            }
            frames.extend(new_position);
        }
        self.base = None;
        self.selected_value = None;
        sync_directory(&self.checkpoints.directory)?;
        if branching {
            self.checkpoints
                .discard_regular(selected.map_or(0, |id| id + 1), abandoned_tip, false);
            if let Some(old) = old_archive.filter(|_| archive.is_none()) {
                self.checkpoints.remove_archive(old);
            }
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
        self.flush()?;
        let id = self.next_id();
        let previous = self.head.as_ref().map(|head| head.tip);
        let at_ms = at_ms.max(self.position.map_or(0, |frame| frame.at_ms));
        let (checkpoint, base) = self.encode(state, previous, at_ms, force)?;
        let directory = &self.checkpoints.directory;
        write_json_atomic(
            &checkpoint_path(directory, id),
            &checkpoint,
            JsonWriteOptions::COMPACT,
        )?;
        sync_directory(directory)?;
        self.publish(id, at_ms, paused)?;
        if let Some(frames) = &mut self.seek_index {
            frames.push(Frame { id, at_ms });
        }
        self.checkpoints.singles.insert(id);
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
        write_json_atomic(
            &batch_path(directory, first),
            &self.batch,
            JsonWriteOptions::COMPACT,
        )?;
        sync_directory(directory)?;
        self.publish(tip, at_ms, false)?;
        if let Some(frames) = &mut self.seek_index {
            frames.extend(self.batch.iter().enumerate().map(|(offset, batch)| Frame {
                id: first + offset as u64,
                at_ms: batch.at_ms,
            }));
        }
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

    fn initial_head(&self, id: u64, next_id: u64, paused: bool) -> Head {
        Head {
            format: FORMAT_VERSION,
            game: self.schema.game.to_owned(),
            schema: self.schema.version,
            next_id,
            tip: id,
            cursor: id,
            paused,
            reset_at: None,
            retention_start: self.retention_start,
            archive: None,
            branch: 0,
            detached_bookmarks: BTreeSet::new(),
            bookmark_links: BTreeMap::new(),
            preview_bookmark: None,
        }
    }

    fn publish(&mut self, id: u64, at_ms: u64, paused: bool) -> io::Result<()> {
        let next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("timeline exhausted checkpoint IDs"))?;
        let mut head = self
            .head
            .clone()
            .unwrap_or_else(|| self.initial_head(id, next_id, paused));
        head.next_id = next_id;
        head.tip = id;
        head.cursor = id;
        head.paused = paused;
        self.commit_head(head)?;
        self.position = Some(Frame { id, at_ms });
        self.selected_value = None;
        Ok(())
    }

    fn commit_head(&mut self, mut head: Head) -> io::Result<()> {
        head.format = FORMAT_VERSION;
        write_json_atomic(
            &self.checkpoints.directory.join("head.json"),
            &head,
            JsonWriteOptions::COMPACT,
        )?;
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
        let frames = checkpoints.forward_frames(head.tip)?;
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
    read_head_matching(directory, Some(schema))
}

fn read_head_matching(directory: &Path, expected: Option<StateSchema>) -> io::Result<Head> {
    let file = File::open(directory.join("head.json"))?;
    let head: Head = serde_json::from_reader(BufReader::new(file)).map_err(|err| {
        invalid_data(format!(
            "cannot read timeline {}: {err}. Use --fresh to start a separate timeline",
            directory.display()
        ))
    })?;
    let (game, version) = expected.map_or((head.game.as_str(), head.schema), |schema| {
        (schema.game, schema.version)
    });
    if !matches!(head.format, FORMAT_VERSION | PREVIOUS_FORMAT_VERSION)
        || head.game != game
        || head.schema != version
    {
        return Err(invalid_data(format!(
            "incompatible timeline {} ({} schema {}, format {}, expected {} schema {}, format {}). Start explicitly with --fresh or choose a new --timeline path",
            directory.display(),
            head.game,
            head.schema,
            head.format,
            game,
            version,
            FORMAT_VERSION,
        )));
    }
    if head.cursor > head.tip
        || head.tip >= head.next_id
        || (!head.paused && head.cursor != head.tip)
        || head.reset_at.is_some_and(|id| id > head.cursor)
        || head
            .preview_bookmark
            .is_some_and(|id| !head.paused || !head.detached_bookmarks.contains(&id))
        || head
            .retention_start
            .is_some_and(|start| start > head.next_id)
        || head.archive.is_some_and(|archive| {
            Some(archive.first) != head.retention_start
                || archive.first > archive.through
                || archive.through > head.tip
                || archive.through >= archive.generation
                || archive.generation > head.next_id
        })
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
    singles: BTreeSet<u64>,
    cached_batch: RefCell<Option<(u64, Vec<Checkpoint>)>>,
    cached_links: RefCell<Option<(u64, Vec<CheckpointLink>)>>,
    archive: Option<ArchivedCheckpoints>,
}

impl Checkpoints {
    fn open(directory: PathBuf, head: Option<&Head>, writer: bool) -> io::Result<Self> {
        let mut batches = BTreeSet::new();
        let mut singles = BTreeSet::new();
        let next_id = head.map_or(0, |head| head.next_id);
        let first_id = head.and_then(|head| head.reset_at).unwrap_or(0);
        let range = head.and_then(|head| head.archive);
        let archive = range
            .map(|range| ArchivedCheckpoints::read(&directory, range))
            .transpose()?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            if let Some(first) = file_id(&name, "batch-") {
                if first < first_id {
                    if writer {
                        let _ = remove_if_present(&entry.path());
                    }
                } else if first < next_id {
                    batches.insert(first);
                } else if writer {
                    // An interrupted publish can leave this ID free for a future checkpoint.
                    fs::remove_file(entry.path())?;
                }
            } else if writer {
                if let Some(id) = file_id(&name, "") {
                    if (first_id..next_id).contains(&id) {
                        singles.insert(id);
                    } else {
                        let _ = remove_if_present(&entry.path());
                    }
                } else if file_id(&name, "archive-").is_some_and(|generation| {
                    range.is_none_or(|archive| archive.generation != generation)
                }) {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
        let mut checkpoints = Self {
            directory,
            batches,
            singles,
            cached_batch: RefCell::new(None),
            cached_links: RefCell::new(None),
            archive,
        };
        if writer {
            checkpoints.retire_archived();
        }
        Ok(checkpoints)
    }

    fn remember_batch(&mut self, first: u64, frames: Vec<Checkpoint>) {
        self.batches.insert(first);
        self.cached_batch.replace(Some((first, frames)));
    }

    fn background_view(&self) -> Self {
        Self {
            directory: self.directory.clone(),
            batches: self.batches.clone(),
            singles: BTreeSet::new(),
            cached_batch: RefCell::new(None),
            cached_links: RefCell::new(None),
            archive: self.archive.clone(),
        }
    }

    fn remove_archive(&self, archive: Archive) {
        let _ = fs::remove_file(retention::path(&self.directory, archive.generation));
    }

    fn retire_archived(&mut self) {
        if let Some(archive) = self.archive.as_ref().map(|archive| archive.range) {
            self.discard_regular(archive.first, archive.through, true);
        }
    }

    fn discard_regular(&mut self, first: u64, last: u64, preserve_batch_tail: bool) {
        if first > last {
            return;
        }
        let mut removed: Vec<_> = self.batches.range(first..=last).copied().collect();
        if preserve_batch_tail && let Some(&boundary) = removed.last() {
            let fully_covered = self.from_batch(boundary, |_| ()).ok().flatten().is_some()
                && self
                    .cached_batch
                    .borrow()
                    .as_ref()
                    .is_some_and(|(_, frames)| frames.len() as u64 <= last - boundary + 1);
            if !fully_covered {
                removed.pop();
            }
        }
        self.cached_batch.replace(None);
        self.cached_links.replace(None);
        for first in removed {
            if remove_if_present(&batch_path(&self.directory, first)) {
                self.batches.remove(&first);
            }
        }
        let singles: Vec<_> = self.singles.range(first..=last).copied().collect();
        for id in singles {
            if remove_if_present(&checkpoint_path(&self.directory, id)) {
                self.singles.remove(&id);
            }
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
        Ok(cached_entry(&cache, id).map(read))
    }

    fn checkpoint(&self, id: u64) -> io::Result<Checkpoint> {
        if let Some(archive) = self
            .archive
            .as_ref()
            .filter(|archive| (archive.range.first..=archive.range.through).contains(&id))
        {
            return archive.get(id).cloned();
        }
        match self.from_batch(id, Checkpoint::clone)? {
            Some(checkpoint) => Ok(checkpoint),
            None => read_checkpoint(&self.directory, id),
        }
    }

    fn link(&self, id: u64) -> io::Result<CheckpointLink> {
        if let Some(archive) = self
            .archive
            .as_ref()
            .filter(|archive| (archive.range.first..=archive.range.through).contains(&id))
        {
            return Ok(archive.get(id)?.into());
        }
        let mut links = self.cached_links.borrow_mut();
        if let Some(link) = cached_entry(&links, id) {
            return Ok(*link);
        }
        if let Some(&first) = self.batches.range(..=id).next_back() {
            if let Some(checkpoint) = cached_entry(&self.cached_batch.borrow(), id) {
                return Ok(checkpoint.into());
            }
            if links.as_ref().is_none_or(|(cached, _)| *cached != first) {
                // Deserialize just the links. Parsing full game states in every
                // batch makes the first seek grow with the whole recording.
                let bytes = fs::read(batch_path(&self.directory, first))?;
                *links = Some((first, serde_json::from_slice(&bytes)?));
            }
            if let Some(link) = cached_entry(&links, id) {
                return Ok(*link);
            }
        }
        read_checkpoint(&self.directory, id)
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

    fn forward_frames(&self, tip: u64) -> io::Result<Vec<Frame>> {
        let mut frames = self.backward_frames(tip, None)?;
        frames.reverse();
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

fn cached_entry<T>(cache: &Option<(u64, Vec<T>)>, id: u64) -> Option<&T> {
    let (first, items) = cache.as_ref()?;
    items.get(usize::try_from(id.checked_sub(*first)?).ok()?)
}

fn file_id(name: &std::ffi::OsStr, prefix: &str) -> Option<u64> {
    let id = name.to_str()?.strip_prefix(prefix)?.strip_suffix(".json")?;
    (id.len() == 20).then(|| id.parse().ok()).flatten()
}

fn remove_if_present(path: &Path) -> bool {
    match fs::remove_file(path) {
        Ok(()) => true,
        Err(err) => err.kind() == io::ErrorKind::NotFound,
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

        writer.capture(Arc::new(json!({"tick": 1})), 16).unwrap();
        let first = writer
            .commits
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
        assert_eq!(first, Frame { id: 1, at_ms: 16 });

        for tick in 2..=72 {
            writer
                .capture(
                    Arc::new(json!({"tick": tick, "pressed": tick == 17})),
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
    fn writer_can_retry_checkpoint_after_a_batch_cannot_be_written() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&0u32, 0).unwrap();
        let blocked = batch_path(&directory.0, 1);
        fs::create_dir(&blocked).unwrap();
        let writer = TimelineWorker::start(timeline).unwrap();
        writer.capture(Arc::new(1u32), 16).unwrap();
        assert!(
            writer
                .request(|timeline| timeline.checkpoint_at(&2u32, 32))
                .is_err()
        );

        fs::remove_dir(&blocked).unwrap();
        writer
            .request(|timeline| timeline.checkpoint_at(&2u32, 32))
            .unwrap();
        writer
            .request(|timeline| {
                assert_eq!(timeline.position(), Some(Frame { id: 2, at_ms: 32 }));
                let intermediate = timeline.checkpoints.snapshot::<u32>(1)?;
                assert_eq!(intermediate.state, 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn resuming_at_the_tip_preserves_history_after_rewinding() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&0u32, 0).unwrap();
        timeline.capture_at(&1u32, 16).unwrap();
        timeline.pause_at(&2u32, 33).unwrap();
        assert_eq!(timeline.rewind::<u32>().unwrap(), Some(1));
        drop(timeline);

        let (mut timeline, selected) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        assert_eq!(selected, Some(1));
        assert_eq!(timeline.resume_tip::<u32>().unwrap(), Some(2));
        assert!(!timeline.is_paused());
        timeline.checkpoint_at(&3u32, 50).unwrap();
        drop(timeline);

        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        assert_eq!(
            reader
                .snapshots::<u32>()
                .map(|s| s.unwrap().state)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
    }

    #[test]
    fn reset_starts_at_zero_and_keeps_bookmarks_across_reopen() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.enable_retention().unwrap();
        timeline.checkpoint_at(&10u32, 0).unwrap();
        timeline.capture_at(&20u32, 1_000).unwrap();
        let bookmark = timeline.save_bookmark("Old game").unwrap();
        timeline.pause_at(&30u32, 2_000).unwrap();
        timeline.seek::<u32>(1_000).unwrap();
        let old_tip = timeline.tip_position().unwrap().unwrap();
        let root = timeline.reset(&0u32).unwrap();
        assert!(root.id > old_tip.id);
        assert_eq!(
            timeline.position(),
            Some(Frame {
                id: root.id,
                at_ms: 0
            })
        );
        assert!(!timeline.is_paused());
        assert_eq!(timeline.open_bookmark::<u32>(bookmark.id).unwrap(), 20);
        assert_eq!(timeline.detached_bookmark(), Some(bookmark.id));
        assert_eq!(timeline.return_from_bookmark::<u32>().unwrap(), Some(0));
        assert!(timeline.rewind::<u32>().unwrap().is_none());
        drop(timeline);

        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        assert_eq!(reader.frames(), [root]);
        assert!(reader.load::<u32>(old_tip.id).is_err());
        drop(reader);
        let (mut timeline, state) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        assert_eq!(state, Some(0));
        assert_eq!(timeline.open_bookmark::<u32>(bookmark.id).unwrap(), 20);
    }

    #[test]
    fn reset_skips_unpublished_batches_and_preserves_history_if_publication_fails() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&10u32, 0).unwrap();
        timeline.stage(&20u32, 1_000).unwrap();
        let pending = timeline.next_id();
        write_json_atomic(
            &batch_path(&directory.0, pending),
            &timeline.batch,
            JsonWriteOptions::COMPACT,
        )
        .unwrap();
        let blocked = checkpoint_path(&directory.0, pending + 1);
        fs::create_dir(&blocked).unwrap();
        assert!(timeline.reset(&0u32).is_err());
        assert_eq!(timeline.position().unwrap().id, 0);
        assert_eq!(timeline.batch.len(), 1);
        fs::remove_dir(&blocked).unwrap();

        let root = timeline.reset(&0u32).unwrap();
        assert_eq!(root.id, pending + 1);
        assert!(timeline.batch.is_empty());
        drop(timeline);
        let (timeline, state) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        assert_eq!(timeline.position(), Some(root));
        assert_eq!(state, Some(0));
    }

    #[test]
    fn warmed_seek_index_catches_up_to_batches_and_keeps_only_the_active_branch() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
        timeline.checkpoint_at(&1u32, 10).unwrap();
        timeline.warm_seek_index();
        timeline.stage(&2u32, 20).unwrap();
        timeline.flush().unwrap();
        timeline.pause_at(&3u32, 30).unwrap();

        assert_eq!(timeline.seek::<u32>(20).unwrap(), Some(2));
        assert_eq!(timeline.seek_index.as_ref().unwrap().len(), 3);
        timeline.resume().unwrap();
        timeline.stage(&4u32, 21).unwrap();
        timeline.flush().unwrap();
        timeline.pause_at(&5u32, 22).unwrap();
        let cached = timeline.seek_index.as_ref().unwrap();
        assert_eq!(
            cached.iter().map(|frame| frame.id).collect::<Vec<_>>(),
            [0, 1, 3, 4]
        );
        assert_eq!(timeline.seek::<u32>(21).unwrap(), Some(4));
        assert_eq!(timeline.seek::<u32>(u64::MAX).unwrap(), Some(5));
    }

    #[test]
    fn compaction_keeps_exact_states_with_geometrically_spaced_history() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<Value>(&directory.0, SCHEMA).unwrap();
        let state_at = |tick: u64| {
            let mut state = json!({ "tick": tick, "motion": [tick % 7, tick / 9, tick % 19] });
            if tick % 3 != 0 {
                state["transient"] = json!({ "tick": tick % 5 });
            }
            state
        };
        for tick in 0..=12 {
            timeline.stage(&state_at(tick), tick * 1000 / 60).unwrap();
        }
        timeline.flush().unwrap();
        // Reproduce an existing format 3 recording, including a delta at the
        // first sample covered by the newly enabled retention policy.
        let mut old = timeline.head.clone().unwrap();
        old.format = PREVIOUS_FORMAT_VERSION;
        write_json_atomic(
            &directory.0.join("head.json"),
            &old,
            JsonWriteOptions::COMPACT,
        )
        .unwrap();
        assert_eq!(
            read_head(&directory.0, SCHEMA).unwrap().format,
            PREVIOUS_FORMAT_VERSION
        );
        timeline.enable_retention().unwrap();
        timeline.seek_frames().unwrap();
        let mut mark = None;
        for tick in 13..=1_800 {
            timeline.stage(&state_at(tick), tick * 1000 / 60).unwrap();
            if tick % 12 == 0 {
                timeline.flush().unwrap();
            }
            if tick == 240 {
                mark = Some(timeline.save_bookmark("Pinned time").unwrap());
            }
            if [720, 1_200, 1_800].contains(&tick) {
                timeline.compact_now();
                assert_eq!(
                    timeline.seek_index.as_ref().unwrap(),
                    &timeline.checkpoints.forward_frames(tick).unwrap()
                );
            }
        }
        let mark = mark.unwrap();
        let position = timeline.position();
        let archive = timeline.head.as_ref().unwrap().archive.unwrap();
        assert!(archive.through > mark.frame.id);
        assert!(!batch_path(&directory.0, 24).exists());
        assert!(batch_path(&directory.0, 0).exists());
        // A replacement file that never made it into head.json is an orphan.
        let orphan = retention::path(&directory.0, archive.generation + 1);
        fs::write(&orphan, b"incomplete").unwrap();
        drop(timeline);

        let (timeline, restored) = Timeline::open::<Value>(&directory.0, SCHEMA).unwrap();
        assert_eq!(timeline.position(), position);
        assert_eq!(restored, Some(state_at(1_800)));
        assert!(!orphan.exists());
        drop(timeline);

        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        let frames = reader.frames();
        let count = |since, until| {
            frames
                .iter()
                .filter(|frame| (since..until).contains(&frame.at_ms))
                .count()
        };
        assert_eq!(count(0, 200), 12);
        assert_eq!(count(3_000, 5_000), 120);
        assert_eq!(count(28_000, 30_000), 120);
        assert!((29..=32).contains(&count(10_000, 14_000)));
        assert!(
            frames.len() < 720,
            "retained {} of 1801 frames",
            frames.len()
        );
        for snapshot in reader.snapshots::<Value>() {
            let snapshot = snapshot.unwrap();
            assert_eq!(snapshot.state, state_at(snapshot.frame.id));
            assert_eq!(
                reader.load::<Value>(snapshot.frame.id).unwrap().state,
                snapshot.state
            );
        }
    }

    #[test]
    fn bookmarks_survive_branching_and_can_be_opened_without_replacing_the_tip() {
        let directory = TestDirectory::new();
        let (mut timeline, _) = Timeline::open::<u64>(&directory.0, SCHEMA).unwrap();
        timeline.enable_retention().unwrap();
        for tick in 0..=480 {
            timeline.stage(&tick, tick * 1000 / 60).unwrap();
            if tick % 12 == 0 {
                timeline.flush().unwrap();
            }
            if tick == 120 {
                timeline.save_bookmark("First").unwrap();
            }
            if tick == 300 {
                timeline.save_bookmark("Independent").unwrap();
            }
        }
        timeline.compact_now();
        timeline.pause_at(&480u64, 8_000).unwrap();
        let ledger = BookmarkLedger::read(&directory.0).unwrap();
        let (first, saved) = (ledger.entries()[0].id, ledger.entries()[1].id);
        assert_eq!(timeline.open_bookmark::<u64>(first).unwrap(), 120);
        assert!(timeline.detached_bookmark().is_none());
        timeline.resume().unwrap();
        timeline.capture_at(&999u64, 5_000).unwrap();
        let tip = timeline.tip_position().unwrap();
        assert_eq!(timeline.open_bookmark::<u64>(saved).unwrap(), 300);
        assert_eq!(timeline.detached_bookmark(), Some(saved));
        assert_eq!(timeline.tip_position().unwrap(), tip);
        assert!(timeline.remove_bookmark(saved).is_err());
        drop(timeline);

        let (mut timeline, shown) = Timeline::open::<u64>(&directory.0, SCHEMA).unwrap();
        assert_eq!(shown, Some(300));
        assert_eq!(timeline.return_from_bookmark::<u64>().unwrap(), Some(999));
        assert_eq!(timeline.open_bookmark::<u64>(saved).unwrap(), 300);
        timeline.resume().unwrap();
        assert_eq!(timeline.position().unwrap().at_ms, 5_000);
        assert!(timeline.position().unwrap().id > tip.unwrap().id);
        assert!(timeline.detached_bookmark().is_none());
        let resumed = timeline.position();
        assert_eq!(timeline.open_bookmark::<u64>(saved).unwrap(), 300);
        assert!(timeline.detached_bookmark().is_none());
        assert_eq!(timeline.position(), resumed);
        timeline.resume().unwrap();
        drop(timeline);

        let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
        assert_eq!(
            reader.snapshots::<u64>().last().unwrap().unwrap().state,
            300
        );
        assert!(reader.load::<u64>(tip.unwrap().id).is_err());
        drop(reader);
        let (mut timeline, _) = Timeline::open::<u64>(&directory.0, SCHEMA).unwrap();
        timeline.remove_bookmark(first).unwrap();
        assert_eq!(
            BookmarkLedger::read(&directory.0).unwrap().entries()[0].id,
            saved
        );
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
        write_json_atomic(
            &batch_path(&directory.0, 3),
            &timeline.batch,
            JsonWriteOptions::COMPACT,
        )
        .unwrap();
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
