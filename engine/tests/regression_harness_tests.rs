use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{
    GameLogic,
    graphics::CpuRenderer,
    recording::{Mp4Config, Mp4Recorder},
    regression::{VideoCaptureConfig, record_state_and_video_then_replay_and_compare},
    render::{CELL_SIZE, draw_board},
    surface::SurfaceSize,
};

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_regression_harness_{nanos}"))
}

#[derive(Debug, Clone)]
struct GridGame {
    width: usize,
    height: usize,
}

#[derive(Debug, Clone, Copy)]
struct SetCell {
    x: usize,
    y: usize,
    v: u8,
}

impl GameLogic for GridGame {
    type State = Vec<Vec<u8>>;
    type Input = SetCell;

    fn initial_state(&self) -> Self::State {
        vec![vec![0u8; self.width]; self.height]
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        let mut next = state.clone();
        if input.y < next.len() && input.x < next[input.y].len() {
            next[input.y][input.x] = input.v;
        }
        next
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

    let game = GridGame {
        width: 4,
        height: 4,
    };

    // Render the board into a tightly sized buffer (no extra padding needed for this test).
    let width = game.width as u32 * CELL_SIZE;
    let height = game.height as u32 * CELL_SIZE;
    let video = VideoCaptureConfig {
        mp4: Mp4Config {
            width,
            height,
            fps: 30,
        },
        hold_frames: 1,
    };

    let out_dir = unique_temp_dir();
    let inputs = [
        SetCell { x: 0, y: 0, v: 1 },
        SetCell { x: 1, y: 0, v: 2 },
        SetCell { x: 2, y: 1, v: 3 },
        SetCell { x: 3, y: 2, v: 4 },
    ];

    let artifacts = record_state_and_video_then_replay_and_compare(
        "engine_regression_harness_record_replay_video_roundtrips",
        &out_dir,
        game,
        inputs,
        video,
        |state: &Vec<Vec<u8>>, buf: &mut [u8], width: u32, height: u32| {
            let mut gfx = CpuRenderer::new(buf, SurfaceSize::new(width, height));
            draw_board(&mut gfx, state);
        },
    )
    .expect("regression harness should complete");

    // Clean up on success (keep artifacts if the test fails, for debugging).
    let _ = fs::remove_file(artifacts.state_json);
    let _ = fs::remove_file(artifacts.live_mp4);
    let _ = fs::remove_file(artifacts.replay_mp4);
    let _ = fs::remove_dir_all(out_dir);
}
