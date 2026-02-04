//! Engine-level regression testing helpers.
//!
//! These utilities help you:
//! - record a frame-by-frame `TimeMachine` (JSON) and a matching MP4 (via ffmpeg),
//! - replay the recording from disk, produce a second MP4, and
//! - assert the decoded video frames match.
//!
//! The engine stays game-agnostic by requiring a caller-provided renderer closure.

use std::{
    ffi::OsString,
    fs,
    io,
    path::{Path, PathBuf},
    process::Command,
};

use std::io::Write;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    recording::{Mp4Config, Mp4Recorder},
    GameLogic, HeadlessRunner, TimeMachine,
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

impl VideoCaptureConfig {
    pub fn rgba_frame_len(&self) -> usize {
        self.mp4.rgba_frame_len()
    }
}

#[derive(Debug, Clone)]
pub struct RecordReplayArtifacts {
    pub state_json: PathBuf,
    pub live_mp4: PathBuf,
    pub replay_mp4: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RecordReplayHashArtifacts {
    pub state_json: PathBuf,
    pub live_hashes: Vec<String>,
    pub replay_hashes: Vec<String>,
}

fn ffmpeg_bin() -> OsString {
    std::env::var_os("ROLLOUT_FFMPEG_BIN").unwrap_or_else(|| OsString::from("ffmpeg"))
}

pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
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

pub fn write_ppm_rgb(path: impl AsRef<Path>, width: u32, height: u32, rgba: &[u8]) -> io::Result<()> {
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

pub fn render_frame_hashes_for_timemachine<State, Render>(
    tm: &TimeMachine<State>,
    width: u32,
    height: u32,
    render: &mut Render,
) -> Vec<String>
where
    Render: FnMut(&State, &mut [u8], u32, u32),
{
    let expected = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(4);
    let mut buf = vec![0u8; expected];
    let mut hashes = Vec::with_capacity(tm.len());

    for frame in 0..tm.len() {
        let state = tm
            .state_at(frame)
            .unwrap_or_else(|| panic!("timemachine missing state at frame {frame}"));
        render(state, &mut buf, width, height);
        hashes.push(rgba_sha256_hex(&buf));
    }

    hashes
}

fn first_hash_mismatch(a: &[String], b: &[String]) -> Option<usize> {
    let n = a.len().min(b.len());
    for i in 0..n {
        if a[i] != b[i] {
            return Some(i);
        }
    }
    if a.len() != b.len() {
        return Some(n);
    }
    None
}

fn compare_render_hashes_and_maybe_dump<State, Render>(
    name: &str,
    out_dir: &Path,
    width: u32,
    height: u32,
    live_tm: &TimeMachine<State>,
    replay_tm: &TimeMachine<State>,
    live_hashes: &[String],
    replay_hashes: &[String],
    render: &mut Render,
) -> io::Result<()>
where
    Render: FnMut(&State, &mut [u8], u32, u32),
{
    let Some(i) = first_hash_mismatch(live_hashes, replay_hashes) else {
        return Ok(());
    };

    let mut msg = String::new();
    if live_hashes.len() != replay_hashes.len() {
        msg.push_str(&format!(
            "render-hash frame count mismatch: live={} replay={}",
            live_hashes.len(),
            replay_hashes.len()
        ));
        msg.push('\n');
    }

    let live_h = live_hashes.get(i).map(String::as_str).unwrap_or("<none>");
    let replay_h = replay_hashes.get(i).map(String::as_str).unwrap_or("<none>");
    msg.push_str(&format!(
        "render-hash mismatch at frame {i} (scenario {name}):\n  live:   {live_h}\n  replay: {replay_h}\n"
    ));

    if dump_ppm_on_mismatch_enabled()
        && i < live_tm.len()
        && i < replay_tm.len()
        && width > 0
        && height > 0
    {
        let base = sanitize_filename(name);
        let live_ppm = out_dir.join(format!("{base}__mismatch_{i}__live.ppm"));
        let replay_ppm = out_dir.join(format!("{base}__mismatch_{i}__replay.ppm"));
        let diff_ppm = out_dir.join(format!("{base}__mismatch_{i}__diff.ppm"));

        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        let mut a = vec![0u8; expected_len];
        let mut b = vec![0u8; expected_len];

        let a_state = live_tm.state_at(i).expect("checked bounds");
        let b_state = replay_tm.state_at(i).expect("checked bounds");
        render(a_state, &mut a, width, height);
        render(b_state, &mut b, width, height);

        // Best-effort dumps; failures should not hide the underlying mismatch.
        let _ = write_ppm_rgb(&live_ppm, width, height, &a);
        let _ = write_ppm_rgb(&replay_ppm, width, height, &b);
        let _ = write_ppm_rgb_diff(&diff_ppm, width, height, &a, &b);

        msg.push_str(&format!(
            "ppm dumps:\n  live:   {}\n  replay: {}\n  diff:   {}\n",
            live_ppm.display(),
            replay_ppm.display(),
            diff_ppm.display()
        ));
    } else if dump_ppm_on_mismatch_enabled() {
        msg.push_str("ppm dumps: skipped (out of bounds or invalid size)\n");
    }

    Err(io::Error::new(io::ErrorKind::Other, msg))
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

/// Engine-level regression helper:
/// - run a scenario live, capturing MP4 frames and saving a `TimeMachine` JSON recording
/// - load the JSON recording, replay frame-by-frame, capture a second MP4
/// - decode both MP4s and assert every frame matches (via ffmpeg `framemd5`)
///
/// The caller provides a `render` function that fills an RGBA frame for a given state.
pub fn record_state_and_video_then_replay_and_compare<G, Render>(
    name: &str,
    out_dir: impl AsRef<Path>,
    game: G,
    inputs: impl IntoIterator<Item = G::Input>,
    video: VideoCaptureConfig,
    mut render: Render,
) -> io::Result<RecordReplayArtifacts>
where
    G: GameLogic + Clone,
    G::State: Serialize + DeserializeOwned,
    Render: FnMut(&G::State, &mut [u8], u32, u32),
{
    let out_dir = out_dir.as_ref();
    fs::create_dir_all(out_dir)?;

    let base = sanitize_filename(name);
    let state_json = out_dir.join(format!("{base}.json"));
    let live_mp4 = out_dir.join(format!("{base}__live.mp4"));
    let replay_mp4 = out_dir.join(format!("{base}__replay.mp4"));

    let hold_frames = video.hold_frames.max(1);
    let mut live_hashes: Vec<String> = Vec::new();
    let mut replay_hashes: Vec<String> = Vec::new();

    // Live record.
    let mut live_runner = HeadlessRunner::new(game.clone());
    let mut live_rec = Mp4Recorder::start(&live_mp4, video.mp4)?;
    {
        let mut buf = vec![0u8; video.rgba_frame_len()];
        let mut capture = |rec: &mut Mp4Recorder, state: &G::State, hashes: &mut Vec<String>| -> io::Result<()> {
            render(state, &mut buf, video.mp4.width, video.mp4.height);
            hashes.push(rgba_sha256_hex(&buf));
            for _ in 0..hold_frames {
                rec.push_rgba_frame(&buf)?;
            }
            Ok(())
        };

        capture(&mut live_rec, live_runner.state(), &mut live_hashes)?;
        for input in inputs {
            live_runner.step(input);
            capture(&mut live_rec, live_runner.state(), &mut live_hashes)?;
        }
    }
    live_rec.finish()?;

    live_runner.timemachine().save_json_file(&state_json)?;

    // Replay record.
    let tm = TimeMachine::<G::State>::load_json_file(&state_json)?;
    let mut replay_runner = HeadlessRunner::from_timemachine(game, tm);
    let mut replay_rec = Mp4Recorder::start(&replay_mp4, video.mp4)?;
    {
        let mut buf = vec![0u8; video.rgba_frame_len()];
        let mut capture = |rec: &mut Mp4Recorder, state: &G::State, hashes: &mut Vec<String>| -> io::Result<()> {
            render(state, &mut buf, video.mp4.width, video.mp4.height);
            hashes.push(rgba_sha256_hex(&buf));
            for _ in 0..hold_frames {
                rec.push_rgba_frame(&buf)?;
            }
            Ok(())
        };

        let frames = replay_runner.history().len();
        for frame in 0..frames {
            replay_runner.seek(frame);
            capture(&mut replay_rec, replay_runner.state(), &mut replay_hashes)?;
        }
    }
    replay_rec.finish()?;

    compare_render_hashes_and_maybe_dump(
        name,
        out_dir,
        video.mp4.width,
        video.mp4.height,
        live_runner.timemachine(),
        replay_runner.timemachine(),
        &live_hashes,
        &replay_hashes,
        &mut render,
    )?;

    assert_mp4_videos_match_ffmpeg(&live_mp4, &replay_mp4)?;

    Ok(RecordReplayArtifacts {
        state_json,
        live_mp4,
        replay_mp4,
    })
}

