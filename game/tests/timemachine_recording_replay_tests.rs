use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{HeadlessRunner, TimeMachine};
use game::{
    playtest::{InputAction, TetrisLogic},
    tetris_core::{Piece, TetrisCore},
};

fn unique_temp_json_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_test_tetris_timemachine_{nanos}.json"))
}

#[test]
fn tetris_timemachine_can_be_saved_and_replayed_from_disk() {
    let logic = TetrisLogic::new(123, vec![Piece::T]);
    let mut runner = HeadlessRunner::new(logic.clone());

    runner.step(InputAction::MoveLeft);
    runner.step(InputAction::RotateCw);
    runner.step(InputAction::MoveRight);

    let out = unique_temp_json_path();
    runner
        .timemachine()
        .save_json_file(&out)
        .expect("save tetris timemachine json");

    let loaded_tm = TimeMachine::<TetrisCore>::load_json_file(&out).expect("load tetris timemachine json");
    let replay_runner = HeadlessRunner::from_timemachine(logic, loaded_tm);

    assert_eq!(replay_runner.frame(), runner.frame());
    assert_eq!(replay_runner.state().snapshot(), runner.state().snapshot());

    let orig_tm = runner.timemachine();
    let replay_tm = replay_runner.timemachine();
    assert_eq!(replay_tm.len(), orig_tm.len());

    for frame in 0..orig_tm.len() {
        let a = orig_tm.state_at(frame).unwrap().snapshot();
        let b = replay_tm.state_at(frame).unwrap().snapshot();
        assert_eq!(a, b, "snapshot mismatch at frame {frame}");
    }

    let _ = fs::remove_file(out);
}

