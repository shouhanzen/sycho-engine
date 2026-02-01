use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::recording::{Mp4Config, Mp4Recorder};

fn unique_temp_mp4_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_test_recording_{nanos}.mp4"))
}

#[test]
fn mp4_recorder_produces_file_when_ffmpeg_is_available() {
    if !Mp4Recorder::ffmpeg_available() {
        eprintln!("skipping: ffmpeg not found on PATH");
        return;
    }

    let out = unique_temp_mp4_path();
    let config = Mp4Config {
        width: 64,
        height: 64,
        fps: 30,
    };

    let mut rec = Mp4Recorder::start(&out, config).expect("failed to start mp4 recorder");
    let mut frame = vec![0u8; config.rgba_frame_len()];

    // Solid blue-ish frame (RGBA).
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&[40, 80, 200, 255]);
    }
    rec.push_rgba_frame(&frame).expect("failed writing frame");
    rec.push_rgba_frame(&frame).expect("failed writing frame");

    rec.finish().expect("failed finalizing mp4");

    let meta = fs::metadata(&out).expect("mp4 output file missing");
    assert!(meta.len() > 0, "mp4 output file was empty");

    let _ = fs::remove_file(out);
}

