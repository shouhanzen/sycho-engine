use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{
    GameLogic,
    recording::{Mp4Config, Mp4Recorder},
    regression::{VideoCaptureConfig, record_state_and_export_video, video_framemd5s},
};

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_regression_harness_{nanos}"))
}

struct Counter;

impl GameLogic for Counter {
    type State = u8;
    type Input = u8;

    fn initial_state(&self) -> Self::State {
        0
    }

    fn step(&self, state: &mut Self::State, input: Self::Input) {
        *state += input;
    }
}

#[test]
fn engine_regression_harness_record_replay_video_roundtrips() {
    if !engine::regression::env_flag("ROLLOUT_REGRESSION_VIDEO") {
        eprintln!("skipping: set ROLLOUT_REGRESSION_VIDEO=1 to enable video regression tests");
        return;
    }

    if !Mp4Recorder::ffmpeg_available() {
        eprintln!("skipping: ffmpeg not found on PATH");
        return;
    }

    let (width, height) = (96, 96);
    let video = VideoCaptureConfig {
        mp4: Mp4Config {
            width,
            height,
            fps: 25,
        },
        hold_frames: 2,
    };

    let out_dir = unique_temp_dir();
    let artifacts = record_state_and_export_video(
        "engine_regression_harness_record_replay_video_roundtrips",
        &out_dir,
        Counter,
        [20, 30, 40, 50],
        video,
        |state, gfx| gfx.clear([*state, 0, 0, 255]),
    )
    .expect("regression harness should complete");

    let hashes = video_framemd5s(&artifacts.video).expect("decode exported MP4");
    assert_eq!(hashes.len(), 9);
    for frames in hashes.windows(3).step_by(2) {
        assert_eq!(frames[0], frames[1]);
        assert_ne!(frames[1], frames[2]);
    }

    // Keep artifacts if the test fails, for debugging.
    let _ = fs::remove_dir_all(out_dir);
}
