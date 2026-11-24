use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{
    HeadlessRunner,
    timeline::{Timeline, TimelineReader},
};
use game::{
    playtest::{InputAction, TetrisLogic},
    state::{GameState, TIMELINE_SCHEMA},
    tetris_core::Piece,
};

fn unique_temp_timeline_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("sycho-tetris-fixture-{nanos}"))
}

#[test]
fn tetris_timeline_can_be_loaded_as_a_test_fixture() {
    let logic = TetrisLogic::new(123, vec![Piece::T]);
    let mut runner = HeadlessRunner::new(logic.clone());

    runner.step(InputAction::MoveLeft);
    runner.step(InputAction::RotateCw);
    runner.step(InputAction::MoveRight);

    let out = unique_temp_timeline_path();
    let (mut timeline, _) = Timeline::open::<GameState>(&out, TIMELINE_SCHEMA).unwrap();
    for (i, state) in runner.history().iter().enumerate() {
        timeline.capture_at(state, i as u64 * 200).unwrap();
    }
    drop(timeline);

    let reader = TimelineReader::open(&out, TIMELINE_SCHEMA).unwrap();
    let snapshots = reader
        .snapshots::<GameState>()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(snapshots.len(), runner.history().len());
    for (snapshot, original) in snapshots.iter().zip(runner.history()) {
        assert_eq!(
            serde_json::to_value(&snapshot.state).unwrap(),
            serde_json::to_value(original).unwrap()
        );
    }

    let fixture = reader.load::<GameState>(2).unwrap();
    let mut restored = HeadlessRunner::from_state(logic, fixture.state);
    restored.step(InputAction::MoveRight);
    assert_eq!(
        restored.state().tetris.snapshot(),
        runner.state().tetris.snapshot()
    );
    drop(reader);
    fs::remove_dir_all(out).unwrap();
}
