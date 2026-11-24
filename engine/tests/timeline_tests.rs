use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use engine::timeline::{StateSchema, Timeline, session_directory};

const SCHEMA: StateSchema = StateSchema {
    game: "test",
    version: 1,
};
static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let index = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(std::env::temp_dir().join(format!(
            "sycho-timeline-{}-{}-{index}",
            std::process::id(),
            timestamp.as_nanos()
        )))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn reload_restores_live_checkpoint_and_rewind_only_branches_when_resumed() {
    let directory = TestDirectory::new();
    let (mut timeline, initial) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(initial, None);
    timeline.checkpoint(&10u32).unwrap();
    timeline.checkpoint(&20u32).unwrap();
    timeline.checkpoint(&29u32).unwrap(); // Exact handoff value between scheduled checkpoints.
    drop(timeline);

    let (mut timeline, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(restored, Some(29));
    timeline.pause(&30u32).unwrap();
    assert_eq!(timeline.rewind::<u32>().unwrap(), Some(29));
    assert_eq!(timeline.rewind::<u32>().unwrap(), Some(20));
    timeline.checkpoint(&99u32).unwrap(); // A handoff while paused preserves the selected state.
    drop(timeline);

    let (mut timeline, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(restored, Some(20));
    assert!(timeline.is_paused());
    let old_tip = directory.0.join("00000000000000000003.json");
    assert!(old_tip.is_file());
    timeline.resume().unwrap();
    assert!(!old_tip.exists());
    timeline.checkpoint(&41u32).unwrap();
    timeline.pause(&42u32).unwrap();
    assert_eq!(timeline.rewind::<u32>().unwrap(), Some(41));
    assert_eq!(timeline.rewind::<u32>().unwrap(), Some(20));
    assert_eq!(timeline.rewind::<u32>().unwrap(), Some(10));
    assert_eq!(timeline.rewind::<u32>().unwrap(), None);
}

#[test]
fn incompatible_schema_and_interrupted_writes_preserve_the_committed_state() {
    let directory = TestDirectory::new();
    let (mut timeline, _) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    timeline.checkpoint(&10u32).unwrap();
    let committed = fs::read(directory.0.join("head.json")).unwrap();
    assert!(Timeline::open::<u32>(&directory.0, SCHEMA).is_err());
    drop(timeline);

    // A crash can leave a partial metadata write and a complete but unpublished snapshot.
    fs::write(directory.0.join("head.tmp"), b"{ \"tip\":").unwrap();
    fs::write(
        directory.0.join("00000000000000000001.json"),
        br#"{"previous":0,"state":999}"#,
    )
    .unwrap();
    let newer = StateSchema {
        version: 2,
        ..SCHEMA
    };
    let error = Timeline::open::<u32>(&directory.0, newer).err().unwrap();
    assert!(error.to_string().contains("incompatible timeline"));
    assert_eq!(fs::read(directory.0.join("head.json")).unwrap(), committed);
    assert!(session_directory(directory.0.clone(), Some(directory.0.clone()), true).is_err());

    let (mut timeline, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(restored, Some(10));
    timeline.checkpoint(&20u32).unwrap();
    drop(timeline);
    let (_, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(restored, Some(20));
}

#[test]
fn timed_deltas_reconstruct_nested_states_and_playback_branches_only_on_resume() {
    use engine::timeline::TimelineReader;
    use serde_json::{Value, json};

    let directory = TestDirectory::new();
    let (mut timeline, _) = Timeline::open::<Value>(&directory.0, SCHEMA).unwrap();
    let expected: Vec<_> = (0..=6).map(|i| {
        let mut value = json!({
            "level": {"board": [[1, i, 0], [0, 0, 0]], "definitions": ["same", "every", "frame"]},
            "cursor": {"angle": if i == 1 { -0.0f64 } else { 0.0f64 }},
        });
        if i % 2 == 0 {
            value["level"]["unlock"] = Value::Null;
        }
        value
    }).collect();
    for (i, state) in expected.iter().enumerate() {
        timeline.capture_at(state, i as u64 * 200).unwrap();
    }
    let stored = |i: u64| -> Value {
        serde_json::from_slice(&fs::read(directory.0.join(format!("{i:020}.json"))).unwrap())
            .unwrap()
    };
    for i in 0..=6 {
        assert_eq!(stored(i).get("state").is_some(), matches!(i, 0 | 5));
    }
    assert!(!stored(1)["delta"].to_string().contains("definitions"));
    assert!(TimelineReader::open(&directory.0, SCHEMA).is_err());
    drop(timeline);

    let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
    assert_eq!(reader.frames()[4].at_ms, 800);
    assert!(
        reader.load::<Value>(1).unwrap().state["cursor"]["angle"]
            .as_f64()
            .unwrap()
            .is_sign_negative()
    );
    assert_eq!(
        reader
            .snapshots::<Value>()
            .map(|s| s.unwrap().state)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(reader.load::<Value>(4).unwrap().state, expected[4]);
    drop(reader);

    let (mut timeline, latest) = Timeline::open::<Value>(&directory.0, SCHEMA).unwrap();
    assert_eq!(latest.unwrap(), expected[6]);
    assert_eq!(timeline.pause_at_start::<Value>().unwrap(), expected[0]);
    assert_eq!(timeline.next_frame().unwrap().unwrap().at_ms, 200);
    assert_eq!(
        timeline.forward::<Value>().unwrap(),
        Some(expected[1].clone())
    );
    assert_eq!(
        timeline.play_until::<Value>(799).unwrap(),
        Some(expected[3].clone())
    );
    assert_eq!(
        timeline.play_until::<Value>(1_200).unwrap(),
        Some(expected[6].clone())
    );
    assert!(timeline.next_frame().unwrap().is_none());
    assert!(timeline.is_paused());
    assert_eq!(
        timeline.rewind::<Value>().unwrap(),
        Some(expected[5].clone())
    );
    assert_eq!(
        timeline.rewind::<Value>().unwrap(),
        Some(expected[4].clone())
    );
    assert_eq!(
        timeline.rewind::<Value>().unwrap(),
        Some(expected[3].clone())
    );
    assert_eq!(
        timeline.forward::<Value>().unwrap(),
        Some(expected[4].clone())
    );
    timeline.resume().unwrap();
    timeline.capture_at(&json!({"branch": true}), 1000).unwrap();
    timeline
        .checkpoint_at(&json!({"branch": "handoff"}), 1125)
        .unwrap();
    assert!(stored(8).get("state").is_some());
    drop(timeline);

    let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
    assert_eq!(
        reader
            .frames()
            .iter()
            .map(|frame| frame.id)
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 4, 7, 8]
    );
    assert!(reader.load::<Value>(5).is_err());
    assert_eq!(
        reader.load::<Value>(8).unwrap().state,
        json!({"branch": "handoff"})
    );
}

#[test]
fn format_one_timelines_open_with_nominal_timing_and_can_continue() {
    use engine::timeline::TimelineReader;

    let directory = TestDirectory::new();
    fs::create_dir_all(&directory.0).unwrap();
    fs::File::create(directory.0.join("writer.lock")).unwrap();
    fs::write(
        directory.0.join("head.json"),
        br#"{"format":1,"game":"test","schema":1,"next_id":2,"tip":1,"cursor":1,"paused":false}"#,
    )
    .unwrap();
    fs::write(
        directory.0.join("00000000000000000000.json"),
        br#"{"previous":null,"state":42}"#,
    )
    .unwrap();
    fs::write(
        directory.0.join("00000000000000000001.json"),
        br#"{"previous":0,"state":45}"#,
    )
    .unwrap();

    let (mut timeline, restored) = Timeline::open::<u32>(&directory.0, SCHEMA).unwrap();
    assert_eq!(restored, Some(45));
    assert_eq!(timeline.position().unwrap().at_ms, 250);
    timeline.capture_at(&47u32, 450).unwrap();
    drop(timeline);
    let reader = TimelineReader::open(&directory.0, SCHEMA).unwrap();
    assert_eq!(
        reader
            .snapshots::<u32>()
            .map(|s| s.unwrap().state)
            .collect::<Vec<_>>(),
        [42, 45, 47]
    );
    assert_eq!(
        reader
            .frames()
            .iter()
            .map(|frame| frame.at_ms)
            .collect::<Vec<_>>(),
        [0, 250, 450]
    );
}
