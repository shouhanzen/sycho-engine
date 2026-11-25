use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{
    GameLogic,
    graphics::{CpuRenderer, Renderer2d},
    recording::{Mp4Config, Mp4Recorder},
    regression::{VideoCaptureConfig, record_state_and_video_then_replay_and_compare},
    surface::SurfaceSize,
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
            fps: 30,
        },
        hold_frames: 1,
    };

    let out_dir = unique_temp_dir();
    let artifacts = record_state_and_video_then_replay_and_compare(
        "engine_regression_harness_record_replay_video_roundtrips",
        &out_dir,
        Counter,
        [20, 30, 40, 50],
        video,
        |state: &u8, buf: &mut [u8], width: u32, height: u32| {
            let mut gfx = CpuRenderer::new(buf, SurfaceSize::new(width, height));
            gfx.clear([*state, 0, 0, 255]);
        },
    )
    .expect("regression harness should complete");

    // Clean up on success (keep artifacts if the test fails, for debugging).
    let _ = fs::remove_dir_all(artifacts.timeline);
    let _ = fs::remove_file(artifacts.live_mp4);
    let _ = fs::remove_file(artifacts.replay_mp4);
    let _ = fs::remove_dir_all(out_dir);
}
