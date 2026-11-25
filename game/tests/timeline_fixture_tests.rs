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

    let out = unique_temp_timeline_path();
    let (mut timeline, _) = Timeline::open::<GameState>(&out, TIMELINE_SCHEMA).unwrap();
    timeline.capture_at(runner.state(), 0).unwrap();
    let mut expected = vec![serde_json::to_value(runner.state()).unwrap()];
    for input in [
        InputAction::MoveLeft,
        InputAction::RotateCw,
        InputAction::MoveRight,
    ] {
        let frame = runner.step(input);
        timeline
            .capture_at(runner.state(), frame as u64 * 200)
            .unwrap();
        expected.push(serde_json::to_value(runner.state()).unwrap());
    }
    drop(timeline);

    let reader = TimelineReader::open(&out, TIMELINE_SCHEMA).unwrap();
    let snapshots = reader
        .snapshots::<GameState>()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    let actual: Vec<_> = snapshots
        .iter()
        .map(|snapshot| serde_json::to_value(&snapshot.state).unwrap())
        .collect();
    assert_eq!(actual, expected);

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
