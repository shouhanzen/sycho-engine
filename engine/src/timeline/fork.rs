use std::{
    fs::{self, File},
    io::{self, BufReader},
    path::Path,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{BookmarkLedger, Checkpoints, StateSchema, read_head_matching, sync_directory};
use crate::persistence::{JsonWriteOptions, write_json_atomic};

pub(super) const SEED: &str = "seed.json";

/// A single state captured for a new recording, with its schema and pause mode.
#[derive(Clone, Serialize, Deserialize)]
pub struct ForkState {
    game: String,
    version: u32,
    pub(super) paused: bool,
    pub(super) state: Value,
}

impl ForkState {
    pub(crate) fn capture(
        schema: StateSchema,
        paused: bool,
        state: &impl Serialize,
    ) -> io::Result<Self> {
        Ok(Self {
            game: schema.game.into(),
            version: schema.version,
            paused,
            state: serde_json::to_value(state)?,
        })
    }

    pub(super) fn validate(&self, schema: StateSchema) -> io::Result<()> {
        if self.game != schema.game || self.version != schema.version {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the starting state uses a different game schema",
            ));
        }
        Ok(())
    }

    /// Capture a closed recording's selected state without modifying its history.
    pub fn from_closed(directory: &Path) -> io::Result<Option<Self>> {
        let lock = match File::open(directory.join("writer.lock")) {
            Ok(lock) => Some(lock),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(err),
        };
        if let Some(lock) = &lock {
            lock.try_lock_shared()
                .map_err(|err| io::Error::other(format!("the recording is still open: {err}")))?;
        }
        let head = match read_head_matching(directory, None) {
            Ok(head) => head,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return read_seed(directory),
            Err(err) => return Err(err),
        };
        let state = if let Some(id) = head.preview_bookmark {
            BookmarkLedger::state(directory, id)?
        } else {
            Checkpoints::open(directory.to_path_buf(), Some(&head), false)?
                .snapshot::<Value>(head.cursor)?
                .state
        };
        Ok(Some(Self {
            game: head.game,
            version: head.schema,
            paused: head.paused,
            state,
        }))
    }

    /// Install the starting state in a newly reserved recording directory.
    pub fn store(&self, directory: &Path) -> io::Result<()> {
        write_json_atomic(&directory.join(SEED), self, JsonWriteOptions::COMPACT)?;
        sync_directory(directory)
    }

    pub(crate) fn reply(path: &Path, result: Result<Self, String>) {
        if let Err(err) = write_json_atomic(path, &result, JsonWriteOptions::COMPACT) {
            eprintln!("could not return the duplicated game state: {err}");
        }
    }
}

pub(super) fn read_seed(directory: &Path) -> io::Result<Option<ForkState>> {
    match File::open(directory.join(SEED)) {
        Ok(file) => Ok(Some(serde_json::from_reader(BufReader::new(file))?)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub(super) fn remove_seed(directory: &Path) {
    let _ = fs::remove_file(directory.join(SEED));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Frame, Timeline, TimelineReader};

    #[test]
    fn a_rewound_state_starts_an_independent_recording_at_zero() {
        let root = std::env::temp_dir().join(format!(
            "sycho-fork-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("source");
        let copy = root.join("copy");
        let schema = StateSchema {
            game: "fork-test",
            version: 1,
        };
        let (mut timeline, _) = Timeline::open::<u32>(&source, schema).unwrap();
        timeline.checkpoint_at(&12, 0).unwrap();
        timeline.pause_at(&34, 500).unwrap();
        assert_eq!(timeline.seek::<u32>(0).unwrap(), Some(12));
        drop(timeline);

        let seed = ForkState::from_closed(&source).unwrap().unwrap();
        seed.store(&copy).unwrap();
        assert_eq!(ForkState::from_closed(&copy).unwrap().unwrap().state, 12);
        let (mut timeline, state) = Timeline::open::<u32>(&copy, schema).unwrap();
        assert_eq!(state, Some(12));
        assert_eq!(timeline.position(), Some(Frame { id: 0, at_ms: 0 }));
        assert_eq!(timeline.tip_position().unwrap(), timeline.position());
        assert!(timeline.is_paused());
        assert!(!copy.join(SEED).exists());
        timeline.resume().unwrap();
        timeline.checkpoint_at(&56, 200).unwrap();
        drop(timeline);

        let original = TimelineReader::open(&source, schema).unwrap();
        let duplicate = TimelineReader::open(&copy, schema).unwrap();
        assert_eq!(original.load::<u32>(1).unwrap().state, 34);
        assert_eq!(duplicate.load::<u32>(0).unwrap().state, 12);
        assert_eq!(duplicate.load::<u32>(1).unwrap().state, 56);
        drop((original, duplicate));
        fs::remove_dir_all(root).unwrap();
    }
}
