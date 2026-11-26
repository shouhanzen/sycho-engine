use super::*;
use std::thread::{self, JoinHandle};

const DENSE_MS: u64 = 2_000;
const BOOKMARK_RADIUS_MS: u64 = 1_000;
const COMPACT_INTERVAL: Duration = Duration::from_secs(2);
const ARCHIVE_KEYFRAME_SAMPLES: usize = 64;

/// An immutable, sparse prefix for the part of a timeline recorded with retention.
/// The last archived ID stays reachable from the unmodified next checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Archive {
    pub first: u64,
    pub through: u64,
    pub generation: u64,
}

#[derive(Clone)]
pub(super) struct ArchivedCheckpoints {
    pub range: Archive,
    frames: Arc<Vec<(u64, Checkpoint)>>,
}

impl ArchivedCheckpoints {
    pub fn read(directory: &Path, range: Archive) -> io::Result<Self> {
        let file = File::open(path(directory, range.generation))?;
        let frames: Vec<(u64, Checkpoint)> = serde_json::from_reader(BufReader::new(file))?;
        if frames.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || frames.first().is_none_or(|(id, _)| *id < range.first)
            || frames
                .binary_search_by_key(&range.through, |(id, _)| *id)
                .is_err()
        {
            return Err(invalid_data("invalid timeline archive"));
        }
        Ok(Self {
            range,
            frames: Arc::new(frames),
        })
    }

    pub fn get(&self, id: u64) -> io::Result<&Checkpoint> {
        self.frames
            .binary_search_by_key(&id, |(id, _)| *id)
            .map(|index| &self.frames[index].1)
            .map_err(|_| invalid_data("checkpoint is outside retained history"))
    }
}

struct Compaction {
    file: PathBuf,
    input_archive: Option<Archive>,
    branch: u64,
    bookmark_revision: u64,
    output: ArchivedCheckpoints,
}

#[derive(Default)]
pub(super) struct Archiver {
    next: Option<Instant>,
    pending: Option<JoinHandle<io::Result<Option<Compaction>>>>,
}

impl Archiver {
    /// Commit a finished job between writes. Building and writing the replacement
    /// can take longer than the capture queue, so it runs on its own thread.
    pub fn after_flush(&mut self, timeline: &mut Timeline) -> io::Result<()> {
        let Some(start) = timeline.retention_start else {
            return Ok(());
        };
        if self.pending.as_ref().is_some_and(|job| job.is_finished()) {
            let job = self.pending.take().unwrap();
            if let Some(result) = job
                .join()
                .map_err(|_| io::Error::other("timeline compactor stopped"))??
            {
                timeline.install_archive(result)?;
            }
        }
        let Some(head) = timeline
            .head
            .clone()
            .filter(|head| !head.paused && head.tip >= start)
        else {
            return Ok(());
        };
        let tip = timeline.position.expect("live timeline has a tip");
        let now = Instant::now();
        if self.pending.is_some()
            || self.next.is_some_and(|next| next > now)
            || tip.at_ms < DENSE_MS
        {
            return Ok(());
        }
        self.next = Some(now + COMPACT_INTERVAL);
        let checkpoints = timeline.checkpoints.background_view();
        let ledger = timeline.bookmarks.clone();
        self.pending = Some(
            thread::Builder::new()
                .name("sycho-timeline-compact".into())
                .spawn(move || {
                    let prepared = prepare(&checkpoints, &head, start, tip, &ledger)?;
                    if let Some(result) = &prepared {
                        let directory = &checkpoints.directory;
                        write_json_atomic(
                            &result.file,
                            &*result.output.frames,
                            JsonWriteOptions::COMPACT,
                        )?;
                        sync_directory(directory)?;
                    }
                    Ok(prepared)
                })?,
        );
        Ok(())
    }
}

impl Drop for Archiver {
    fn drop(&mut self) {
        if let Some(job) = self.pending.take()
            && let Ok(Ok(Some(result))) = job.join()
        {
            let _ = fs::remove_file(result.file);
        }
    }
}

impl Timeline {
    /// Adopt retention prospectively. Imported recordings and direct fixture
    /// captures keep their original density.
    pub(crate) fn enable_retention(&mut self) -> io::Result<()> {
        if self.retention_start.is_none() {
            let start = self.next_id();
            if let Some(mut head) = self.head.clone() {
                head.retention_start = Some(start);
                self.commit_head(head)?;
                sync_directory(&self.checkpoints.directory)?;
            }
            self.retention_start = Some(start);
        }
        Ok(())
    }

    fn install_archive(&mut self, result: Compaction) -> io::Result<()> {
        let Some(mut head) = self.head.clone().filter(|head| {
            !head.paused
                && head.branch == result.branch
                && head.archive == result.input_archive
                && self.bookmarks.revision() == result.bookmark_revision
        }) else {
            let _ = fs::remove_file(result.file);
            return Ok(());
        };
        let range = result.output.range;
        head.archive = Some(range);
        self.commit_head(head)?;
        if let Some(frames) = &mut self.seek_index {
            let first = frames.partition_point(|frame| frame.id < range.first);
            let after = frames.partition_point(|frame| frame.id <= range.through);
            frames.splice(
                first..after,
                result.output.frames.iter().map(|(id, checkpoint)| Frame {
                    id: *id,
                    at_ms: checkpoint.at_ms,
                }),
            );
        }
        self.checkpoints.archive = Some(result.output);
        self.future = Some(Vec::new());
        sync_directory(&self.checkpoints.directory)?;
        self.checkpoints.retire_archived();
        if let Some(old) = result.input_archive {
            self.checkpoints.remove_archive(old);
        }
        Ok(())
    }
}

fn prepare(
    checkpoints: &Checkpoints,
    head: &Head,
    first: u64,
    tip: Frame,
    ledger: &BookmarkLedger,
) -> io::Result<Option<Compaction>> {
    let mut latest = Some(tip.id);
    let mut source = Vec::new();
    let mut through = None;
    while let Some(id) = latest.filter(|&id| id >= first) {
        let checkpoint = checkpoints.checkpoint(id)?;
        latest = previous_id(checkpoint.previous, id)?;
        if through.is_none() {
            if tip.at_ms.saturating_sub(checkpoint.at_ms) < DENSE_MS {
                continue;
            }
            if head.archive.is_some_and(|archive| id <= archive.through) {
                return Ok(None);
            }
            through = Some(id);
        }
        source.push((id, checkpoint));
    }
    let Some(through) = through else {
        return Ok(None);
    };
    let mut marks: Vec<_> = ledger
        .entries()
        .iter()
        .filter(|mark| !head.detached_bookmarks.contains(&mark.id))
        .map(|mark| mark.frame.at_ms)
        .collect();
    marks.sort_unstable();
    let mut marks = marks.into_iter().peekable();
    let mut last_bucket = [None; 64];
    let mut previous = None;
    let mut kept = Vec::new();
    let mut kept_state = None;
    let mut state = None;
    for (id, checkpoint) in source.into_iter().rev() {
        if kept.is_empty() {
            previous = checkpoint.previous;
            if matches!(checkpoint.payload, Payload::Delta { .. }) {
                let id =
                    previous.ok_or_else(|| invalid_data("archive delta has no prior state"))?;
                state = Some(checkpoints.snapshot_value(id)?.1);
            }
        }
        let at_ms = checkpoint.at_ms;
        state = Some(checkpoint.payload.restore(state.take())?);
        while marks
            .peek()
            .is_some_and(|&at| at.saturating_add(BOOKMARK_RADIUS_MS) < at_ms)
        {
            marks.next();
        }
        let protected = marks
            .peek()
            .is_some_and(|&at| at.abs_diff(at_ms) <= BOOKMARK_RADIUS_MS);
        // Fixed, nested time buckets keep the first sample in each bucket at
        // approximately 60 / 2^tier Hz even when captures have irregular spacing.
        let band = (tip.at_ms.saturating_sub(at_ms) / DENSE_MS + 1).ilog2() as usize;
        let tick = (u128::from(at_ms) * 60 / 1_000) as u64;
        let bucket = tick >> band;
        let occupied = last_bucket[band].replace(bucket) == Some(bucket);
        if id != through && occupied && !protected && !kept.is_empty() {
            continue;
        }
        let value = state.as_ref().unwrap();
        let payload = match &kept_state {
            Some(prior) if kept.len() % ARCHIVE_KEYFRAME_SAMPLES != 0 => Payload::Delta {
                delta: Delta::between(prior, value),
            },
            _ => Payload::Full {
                state: value.clone(),
            },
        };
        kept.push((
            id,
            Checkpoint {
                previous,
                at_ms,
                payload,
            },
        ));
        kept_state = Some(value.clone());
        previous = Some(id);
    }
    Ok(Some(Compaction {
        file: path(&checkpoints.directory, head.next_id),
        input_archive: head.archive,
        branch: head.branch,
        bookmark_revision: ledger.revision(),
        output: ArchivedCheckpoints {
            range: Archive {
                first,
                through,
                generation: head.next_id,
            },
            frames: Arc::new(kept),
        },
    }))
}

pub(super) fn path(directory: &Path, generation: u64) -> PathBuf {
    directory.join(format!("archive-{generation:020}.json"))
}

#[cfg(test)]
impl Timeline {
    pub(super) fn compact_now(&mut self) {
        self.flush().unwrap();
        let result = prepare(
            &self.checkpoints,
            self.head.as_ref().unwrap(),
            self.retention_start.unwrap(),
            self.position.unwrap(),
            &self.bookmarks,
        )
        .unwrap()
        .unwrap();
        write_json_atomic(
            &result.file,
            &*result.output.frames,
            JsonWriteOptions::COMPACT,
        )
        .unwrap();
        self.install_archive(result).unwrap();
    }
}
