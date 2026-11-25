//! Engine-level regression testing helpers.
//!
//! These utilities help you:
//! - record a filesystem timeline and a matching MP4 (via ffmpeg),
//! - replay the recording from disk, produce a second MP4, and
//! - assert the decoded video frames match.
//!
//! The engine stays game-agnostic by requiring a caller-provided renderer closure.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use std::io::Write;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    GameLogic, HeadlessRunner,
    recording::{Mp4Config, Mp4Recorder},
    surface::SurfaceSize,
    timeline::{StateSchema, Timeline, TimelineReader, session_directory},
};

/// Environment flag helper: accepts `1/true/yes/on` (case-insensitive).
pub fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

/// If set, regression tests may update golden files in-place.
pub fn update_goldens_enabled() -> bool {
    env_flag("ROLLOUT_UPDATE_GOLDENS")
}

#[derive(Debug, Clone, Copy)]
pub struct VideoCaptureConfig {
    pub mp4: Mp4Config,
    /// How many identical video frames to emit per engine frame/state.
    pub hold_frames: usize,
}

#[derive(Debug, Clone)]
pub struct RecordReplayArtifacts {
    pub timeline: PathBuf,
    pub live_mp4: PathBuf,
    pub replay_mp4: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RecordReplayHashArtifacts {
    pub timeline: PathBuf,
    pub live_hashes: Vec<String>,
    pub replay_hashes: Vec<String>,
}

fn ffmpeg_bin() -> OsString {
    std::env::var_os("ROLLOUT_FFMPEG_BIN").unwrap_or_else(|| OsString::from("ffmpeg"))
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[macro_export]
macro_rules! regression_golden_path {
    ($name:expr) => {{
        let base = $crate::regression::sanitize_filename($name);
        ::std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("goldens")
            .join(format!("{base}.json"))
    }};
}

pub fn rgba_sha256_hex(rgba: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rgba);
    let digest = hasher.finalize();
    hex::encode(digest)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FrameHashGolden {
    pub version: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hash_alg: String,
    /// One hash per logical engine frame / state.
    pub hashes: Vec<String>,
}

impl FrameHashGolden {
    pub fn new(name: impl Into<String>, width: u32, height: u32, hashes: Vec<String>) -> Self {
        Self {
            version: 1,
            name: name.into(),
            width,
            height,
            hash_alg: "sha256".to_string(),
            hashes,
        }
    }
}

pub fn load_golden_json(path: impl AsRef<Path>) -> io::Result<FrameHashGolden> {
    let path = path.as_ref();
    let file = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed parsing golden json {}: {e}", path.display()),
        )
    })
}

pub fn save_golden_json(path: impl AsRef<Path>, golden: &FrameHashGolden) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file = fs::File::create(path)?;
    let mut writer = io::BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, golden)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    writer.flush()?;
    Ok(())
}

pub fn assert_or_update_golden_json(
    path: impl AsRef<Path>,
    golden: &FrameHashGolden,
    update: bool,
) -> io::Result<()> {
    let path = path.as_ref();
    let exists = path.exists();

    if update || !exists {
        save_golden_json(path, golden)?;
        if !exists {
            eprintln!("wrote golden: {}", path.display());
        } else {
            eprintln!("updated golden: {}", path.display());
        }
        return Ok(());
    }

    let expected = load_golden_json(path)?;
    if expected.version != golden.version
        || expected.hash_alg != golden.hash_alg
        || expected.width != golden.width
        || expected.height != golden.height
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "golden metadata mismatch at {}:\nexpected: v{} alg={} {}x{}\nactual:   v{} alg={} {}x{}\n(hint: set ROLLOUT_UPDATE_GOLDENS=1 to rewrite)",
                path.display(),
                expected.version,
                expected.hash_alg,
                expected.width,
                expected.height,
                golden.version,
                golden.hash_alg,
                golden.width,
                golden.height
            ),
        ));
    }

    if expected.hashes.len() != golden.hashes.len() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "golden frame count mismatch at {}: expected {} hashes, got {}\n(hint: set ROLLOUT_UPDATE_GOLDENS=1 to rewrite)",
                path.display(),
                expected.hashes.len(),
                golden.hashes.len()
            ),
        ));
    }

    for (i, (a, b)) in expected.hashes.iter().zip(golden.hashes.iter()).enumerate() {
        if a != b {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "golden mismatch at {} (frame {i}):\nexpected: {a}\nactual:   {b}\n(hint: set ROLLOUT_UPDATE_GOLDENS=1 to rewrite)",
                    path.display()
                ),
            ));
        }
    }

    Ok(())
}

pub fn assert_or_update_golden_hashes(
    path: impl AsRef<Path>,
    name: &str,
    width: u32,
    height: u32,
    hashes: Vec<String>,
    update: bool,
) -> io::Result<()> {
    let golden = FrameHashGolden::new(name, width, height, hashes);
    assert_or_update_golden_json(path, &golden, update)
}

/// If set, regression helpers will write `.ppm` images for the first mismatching frame.
pub fn dump_ppm_on_mismatch_enabled() -> bool {
    env_flag("ROLLOUT_REGRESSION_DUMP_PPM")
}

pub fn write_ppm_rgb(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> io::Result<()> {
    let path = path.as_ref();
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if rgba.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "rgba len {} != expected {} for {}x{}",
                rgba.len(),
                expected,
                width,
                height
            ),
        ));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = fs::File::create(path)?;
    // Binary PPM: https://netpbm.sourceforge.net/doc/ppm.html
    write!(file, "P6\n{} {}\n255\n", width, height)?;
    for px in rgba.chunks_exact(4) {
        file.write_all(&px[0..3])?;
    }
    Ok(())
}

fn write_ppm_rgb_diff(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    a_rgba: &[u8],
    b_rgba: &[u8],
) -> io::Result<()> {
    let path = path.as_ref();
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    if a_rgba.len() != expected || b_rgba.len() != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "rgba lens {} and {} != expected {} for {}x{}",
                a_rgba.len(),
                b_rgba.len(),
                expected,
                width,
                height
            ),
        ));
    }

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let mut file = fs::File::create(path)?;
    write!(file, "P6\n{} {}\n255\n", width, height)?;
    for (a, b) in a_rgba.chunks_exact(4).zip(b_rgba.chunks_exact(4)) {
        let dr = a[0].abs_diff(b[0]);
        let dg = a[1].abs_diff(b[1]);
        let db = a[2].abs_diff(b[2]);
        file.write_all(&[dr, dg, db])?;
    }
    Ok(())
}

pub fn video_framemd5s(path: impl AsRef<Path>) -> io::Result<Vec<String>> {
    let path = path.as_ref();
    let output = Command::new(ffmpeg_bin())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-vf")
        .arg("format=rgba")
        .arg("-f")
        .arg("framemd5")
        .arg("-")
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "ffmpeg framemd5 failed for {} (status {}):\n{}",
                path.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let hashes = stdout
        .lines()
        .filter_map(|line| {
            if line.starts_with('#') {
                return None;
            }
            let (_, hash) = line.rsplit_once(',')?;
            let hash = hash.trim();
            if hash.is_empty() {
                None
            } else {
                Some(hash.to_string())
            }
        })
        .collect();

    Ok(hashes)
}

pub fn assert_mp4_videos_match_ffmpeg(
    live_mp4: impl AsRef<Path>,
    replay_mp4: impl AsRef<Path>,
) -> io::Result<()> {
    let live_mp4 = live_mp4.as_ref();
    let replay_mp4 = replay_mp4.as_ref();

    let live_md5s = video_framemd5s(live_mp4)?;
    let replay_md5s = video_framemd5s(replay_mp4)?;

    if live_md5s.len() != replay_md5s.len() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "video frame counts differed: live={} replay={} ({} vs {})",
                live_md5s.len(),
                replay_md5s.len(),
                live_mp4.display(),
                replay_mp4.display()
            ),
        ));
    }

    for (i, (a, b)) in live_md5s.iter().zip(replay_md5s.iter()).enumerate() {
        if a != b {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!(
                    "video frame {i} differed (md5 live != replay):\n  live:   {a}\n  replay: {b}\n  live mp4:   {}\n  replay mp4: {}",
                    live_mp4.display(),
                    replay_mp4.display()
                ),
            ));
        }
    }

    Ok(())
}

/// Records the live scenario and verifies the video rendered from the saved timeline.
pub fn record_state_and_video_then_replay_and_compare<G, Render>(
    name: &str,
    out_dir: impl AsRef<Path>,
    game: G,
    inputs: impl IntoIterator<Item = G::Input>,
    video: VideoCaptureConfig,
    mut render: Render,
) -> io::Result<RecordReplayArtifacts>
where
    G: GameLogic,
    G::State: Serialize + DeserializeOwned,
    Render: FnMut(&G::State, &mut [u8], u32, u32),
{
    let out_dir = out_dir.as_ref();
    let base = sanitize_filename(name);
    let live_mp4 = out_dir.join(format!("{base}__live.mp4"));
    let replay_mp4 = out_dir.join(format!("{base}__replay.mp4"));
    let size = SurfaceSize::new(video.mp4.width, video.mp4.height);
    let hold_frames = video.hold_frames.max(1);
    let spacing_ms = (hold_frames as u64).saturating_mul(1_000) / u64::from(video.mp4.fps.max(1));
    let capture = |recorder: &mut Mp4Recorder, pixels: &[u8]| -> io::Result<()> {
        for _ in 0..hold_frames {
            recorder.push_rgba_frame(pixels)?;
        }
        Ok(())
    };

    let mut live_rec = Mp4Recorder::start(&live_mp4, video.mp4)?;
    let live = record_frames(
        out_dir.join(format!("{base}.timeline")),
        game,
        inputs,
        size,
        spacing_ms.max(1),
        &mut render,
        |pixels| capture(&mut live_rec, pixels),
    )?;
    live_rec.finish()?;

    let mut replay_rec = Mp4Recorder::start(&replay_mp4, video.mp4)?;
    replay_frames::<G::State>(name, &live, size, &mut render, |pixels| {
        capture(&mut replay_rec, pixels)
    })?;
    replay_rec.finish()?;
    assert_mp4_videos_match_ffmpeg(&live_mp4, &replay_mp4)?;

    Ok(RecordReplayArtifacts {
        timeline: live.timeline,
        live_mp4,
        replay_mp4,
    })
}

/// Verifies the rendered frames against snapshots loaded from a committed timeline.
pub fn record_state_then_replay_and_compare_render_hashes<G, Render>(
    name: &str,
    out_dir: impl AsRef<Path>,
    game: G,
    inputs: impl IntoIterator<Item = G::Input>,
    width: u32,
    height: u32,
    mut render: Render,
) -> io::Result<RecordReplayHashArtifacts>
where
    G: GameLogic,
    G::State: Serialize + DeserializeOwned,
    Render: FnMut(&G::State, &mut [u8], u32, u32),
{
    let base = sanitize_filename(name);
    let size = SurfaceSize::new(width, height);
    let live = record_frames(
        out_dir.as_ref().join(format!("{base}.timeline")),
        game,
        inputs,
        size,
        200,
        &mut render,
        |_| Ok(()),
    )?;
    let replay_hashes = replay_frames::<G::State>(name, &live, size, &mut render, |_| Ok(()))?;

    Ok(RecordReplayHashArtifacts {
        timeline: live.timeline,
        live_hashes: live.hashes,
        replay_hashes,
    })
}

const FIXTURE_SCHEMA: StateSchema = StateSchema {
    game: "test-fixture",
    version: 1,
};

struct RecordedFrames {
    timeline: PathBuf,
    hashes: Vec<String>,
    pixels: Option<Vec<Vec<u8>>>,
}

fn record_frames<G: GameLogic>(
    path: PathBuf,
    game: G,
    inputs: impl IntoIterator<Item = G::Input>,
    size: SurfaceSize,
    spacing_ms: u64,
    render: &mut impl FnMut(&G::State, &mut [u8], u32, u32),
    mut capture: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<RecordedFrames>
where
    G::State: Serialize + DeserializeOwned,
{
    let timeline = session_directory(path, None, true)?;
    let (mut writer, _) = Timeline::open::<G::State>(&timeline, FIXTURE_SCHEMA)?;
    let mut live = RecordedFrames {
        timeline,
        hashes: Vec::new(),
        pixels: (dump_ppm_on_mismatch_enabled() && size.width > 0 && size.height > 0)
            .then(Vec::new),
    };
    let mut runner = HeadlessRunner::new(game);
    let mut pixels = vec![0; size.rgba_len()];

    for input in std::iter::once(None).chain(inputs.into_iter().map(Some)) {
        if let Some(input) = input {
            runner.step(input);
        }
        let state = runner.state();
        writer.capture_at(state, (runner.frame() as u64).saturating_mul(spacing_ms))?;
        render(state, &mut pixels, size.width, size.height);
        live.hashes.push(rgba_sha256_hex(&pixels));
        if let Some(frames) = &mut live.pixels {
            frames.push(pixels.clone());
        }
        capture(&pixels)?;
    }
    Ok(live)
}

fn replay_frames<S: DeserializeOwned>(
    name: &str,
    live: &RecordedFrames,
    size: SurfaceSize,
    render: &mut impl FnMut(&S, &mut [u8], u32, u32),
    mut capture: impl FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<Vec<String>> {
    let reader = TimelineReader::open(&live.timeline, FIXTURE_SCHEMA)?;
    let count = reader.frames().len();
    if live.hashes.len() != count {
        return Err(io::Error::other(format!(
            "render-hash frame count mismatch: live={} replay={count}",
            live.hashes.len()
        )));
    }

    let mut hashes = Vec::with_capacity(count);
    let mut pixels = vec![0; size.rgba_len()];
    for (i, snapshot) in reader.snapshots::<S>().enumerate() {
        render(&snapshot?.state, &mut pixels, size.width, size.height);
        let replay_hash = rgba_sha256_hex(&pixels);
        let live_hash = &live.hashes[i];
        if *live_hash != replay_hash {
            let mut message = format!(
                "render-hash mismatch at frame {i} (scenario {name}):\n  live:   {live_hash}\n  replay: {replay_hash}\n"
            );
            if let Some(frames) = &live.pixels {
                let out_dir = live.timeline.parent().unwrap_or_else(|| Path::new("."));
                let base = sanitize_filename(name);
                let live_ppm = out_dir.join(format!("{base}__mismatch_{i}__live.ppm"));
                let replay_ppm = out_dir.join(format!("{base}__mismatch_{i}__replay.ppm"));
                let diff_ppm = out_dir.join(format!("{base}__mismatch_{i}__diff.ppm"));
                let (width, height) = (size.width, size.height);
                // Best-effort dumps; failures should not hide the mismatch.
                let _ = write_ppm_rgb(&live_ppm, width, height, &frames[i]);
                let _ = write_ppm_rgb(&replay_ppm, width, height, &pixels);
                let _ = write_ppm_rgb_diff(&diff_ppm, width, height, &frames[i], &pixels);
                message.push_str(&format!(
                    "ppm dumps:\n  live:   {}\n  replay: {}\n  diff:   {}\n",
                    live_ppm.display(),
                    replay_ppm.display(),
                    diff_ppm.display()
                ));
            }
            return Err(io::Error::other(message));
        }
        capture(&pixels)?;
        hashes.push(replay_hash);
    }
    Ok(hashes)
}
