use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::TimeMachine;

fn unique_temp_json_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_test_timemachine_{nanos}.json"))
}

#[test]
fn timemachine_save_and_load_roundtrips_history_and_frame() {
    let mut tm = TimeMachine::new(0i32);
    tm.record(1);
    tm.record(2);

    // Rewind and branch to ensure truncate-on-record behavior persists.
    tm.rewind(1);
    tm.record(99);

    assert_eq!(tm.frame(), 2);
    assert_eq!(tm.history(), &[0, 1, 99]);

    let out = unique_temp_json_path();
    tm.save_json_file(&out).expect("save timemachine json");

    let loaded = TimeMachine::<i32>::load_json_file(&out).expect("load timemachine json");
    assert_eq!(loaded.frame(), tm.frame());
    assert_eq!(loaded.history(), tm.history());
    assert_eq!(loaded.state(), tm.state());

    let _ = fs::remove_file(out);
}

