use engine::HeadlessRunner;
use engine::graphics::CpuRenderer;
use engine::recording::{Mp4Config, Mp4Recorder};
use engine::regression::{VideoCaptureConfig, record_state_and_video_then_replay_and_compare};
use engine::render::{CELL_SIZE, draw_board};
use engine::surface::SurfaceSize;

use game::playtest::{InputAction, TetrisLogic};
use game::state::GameState;
use game::tetris_core::{BOARD_HEIGHT, BOARD_WIDTH, Piece, Vec2i};

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Once;

static WARNED_FFMPEG_MISSING: Once = Once::new();
static WARNED_E2E_IMAGE_CAPTURE_FAILED: Once = Once::new();

struct E2eMp4 {
    rec: Mp4Recorder,
    width: u32,
    height: u32,
    hold_frames: usize,
    buf: Vec<u8>,
}

impl E2eMp4 {
    fn maybe_start(test_name: &str) -> Option<Self> {
        if !env_flag("ROLLOUT_E2E_RECORD_MP4") {
            return None;
        }

        if !Mp4Recorder::ffmpeg_available() {
            let msg = "ROLLOUT_E2E_RECORD_MP4 is set, but `ffmpeg` was not found (install ffmpeg or set ROLLOUT_FFMPEG_BIN to a full path)";
            if env_flag("ROLLOUT_E2E_RECORD_STRICT") {
                panic!("{msg}");
            }
            WARNED_FFMPEG_MISSING
                .call_once(|| eprintln!("warning: {msg}; recording disabled for this run"));
            return None;
        }

        let fps = env_u32("ROLLOUT_E2E_RECORD_FPS").unwrap_or(30);
        let hold_frames = env_usize("ROLLOUT_E2E_RECORD_HOLD_FRAMES")
            .unwrap_or(10)
            .max(1);

        let (width, height) = board_capture_dimensions();

        let out = default_record_dir().join(format!("{}.mp4", sanitize_filename(test_name)));
        let config = Mp4Config { width, height, fps };
        let rec = Mp4Recorder::start(&out, config).unwrap_or_else(|e| {
            panic!(
                "failed to start mp4 recorder via ffmpeg at {}: {e}",
                out.display()
            )
        });

        eprintln!("mp4 recording: {}", rec.output().display());

        Some(Self {
            rec,
            width,
            height,
            hold_frames,
            buf: vec![0u8; config.rgba_frame_len()],
        })
    }

    fn capture(&mut self, runner: &HeadlessRunner<TetrisLogic>) {
        render_board_frame(runner, self.width, self.height, self.buf.as_mut_slice());
        for _ in 0..self.hold_frames {
            self.rec
                .push_rgba_frame(&self.buf)
                .expect("failed writing frame to ffmpeg");
        }
    }

    fn finish(mut self) -> PathBuf {
        let out = self.rec.output().to_path_buf();
        self.rec
            .finish()
            .expect("ffmpeg failed while finalizing the mp4");
        let meta = fs::metadata(&out).expect("mp4 output missing after ffmpeg finished");
        assert!(meta.len() > 0, "mp4 output was empty");
        out
    }
}

struct E2eImageCapture {
    test_name: String,
    out_dir: PathBuf,
    width: u32,
    height: u32,
    frame_idx: usize,
    buf: Vec<u8>,
}

impl E2eImageCapture {
    fn maybe_start(test_name: &str) -> Option<Self> {
        if !env_flag("ROLLOUT_E2E_CAPTURE_IMAGES") {
            return None;
        }

        let out_dir = default_image_dir();
        if let Err(err) = fs::create_dir_all(&out_dir) {
            let msg = format!(
                "ROLLOUT_E2E_CAPTURE_IMAGES is set, but image directory {} could not be created: {err}",
                out_dir.display()
            );
            if env_flag("ROLLOUT_E2E_CAPTURE_STRICT") {
                panic!("{msg}");
            }
            WARNED_E2E_IMAGE_CAPTURE_FAILED
                .call_once(|| eprintln!("warning: {msg}; image capture disabled for this run"));
            return None;
        }

        let (width, height) = board_capture_dimensions();
        Some(Self {
            test_name: sanitize_filename(test_name),
            out_dir,
            width,
            height,
            frame_idx: 0,
            buf: vec![0u8; (width as usize) * (height as usize) * 4],
        })
    }

    fn capture(&mut self, runner: &HeadlessRunner<TetrisLogic>, label: &str) {
        render_board_frame(runner, self.width, self.height, self.buf.as_mut_slice());

        let file_name = format!(
            "{}__{:03}_{}.png",
            self.test_name,
            self.frame_idx,
            sanitize_filename(label)
        );
        let path = self.out_dir.join(file_name);
        write_png_rgba(&path, self.width, self.height, self.buf.as_slice());
        eprintln!("image capture: {}", path.display());
        self.frame_idx = self.frame_idx.saturating_add(1);
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn env_usize(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| v.parse().ok())
}

fn sanitize_filename(name: &str) -> String {
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

fn board_capture_dimensions() -> (u32, u32) {
    // Render just the board, with a 1-cell padding so the outline is visible.
    let padding_cells = 1u32;
    let width = (BOARD_WIDTH as u32 + padding_cells * 2) * CELL_SIZE;
    let height = (BOARD_HEIGHT as u32 + padding_cells * 2) * CELL_SIZE;
    (width, height)
}

fn render_board_frame(
    runner: &HeadlessRunner<TetrisLogic>,
    width: u32,
    height: u32,
    buf: &mut [u8],
) {
    let board = runner.state().tetris.board_with_active_piece();
    let mut gfx = CpuRenderer::new(buf, SurfaceSize::new(width, height));
    draw_board(&mut gfx, &board);
}

fn write_png_rgba(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let file = fs::File::create(path).unwrap_or_else(|err| {
        panic!("failed to create png at {}: {err}", path.display());
    });
    let writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder.write_header().unwrap_or_else(|err| {
        panic!("failed to write png header at {}: {err}", path.display());
    });
    png_writer.write_image_data(rgba).unwrap_or_else(|err| {
        panic!("failed to write png data at {}: {err}", path.display());
    });
}

fn default_record_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ROLLOUT_E2E_RECORD_DIR") {
        return PathBuf::from(dir);
    }

    // `CARGO_MANIFEST_DIR` is `.../rollout_engine/game`; the workspace `target/` lives at `..`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("e2e_recordings")
}

fn default_image_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("ROLLOUT_E2E_IMAGE_DIR") {
        return PathBuf::from(dir);
    }

    // `CARGO_MANIFEST_DIR` is `.../rollout_engine/game`; the workspace `target/` lives at `..`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("e2e_images")
}

#[test]
fn e2e_hard_drop_places_o_piece() {
    let logic = TetrisLogic::new(0, vec![Piece::O]);
    let mut runner = HeadlessRunner::new(logic);
    let mut rec = E2eMp4::maybe_start("e2e_hard_drop_places_o_piece");
    let mut images = E2eImageCapture::maybe_start("e2e_hard_drop_places_o_piece");

    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "initial");
    }

    runner.step(InputAction::HardDrop);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "after_hard_drop");
    }
    let snapshot = runner.state().tetris.snapshot();

    let filled = snapshot
        .board
        .iter()
        .flatten()
        .filter(|&&cell| cell != 0)
        .count();
    assert_eq!(filled, 4);

    let expected = [(4usize, 0usize), (5, 0), (4, 1), (5, 1)];
    for (x, y) in expected {
        assert_eq!(snapshot.board[y][x], 2);
    }

    assert_eq!(snapshot.current_piece, Some(Piece::O));
    assert_eq!(snapshot.next_piece, Some(Piece::O));
    assert_eq!(
        snapshot.current_piece_pos,
        Vec2i::new(4, BOARD_HEIGHT as i32)
    );
    assert!(!snapshot.game_over);

    if let Some(r) = rec.take() {
        r.finish();
    }
}

#[test]
fn e2e_state_recording_replay_video_matches_live_video() {
    if !env_flag("ROLLOUT_E2E_VERIFY_STATE_REPLAY") {
        return;
    }

    if !env_flag("ROLLOUT_E2E_RECORD_MP4") {
        eprintln!("skipping: ROLLOUT_E2E_VERIFY_STATE_REPLAY requires ROLLOUT_E2E_RECORD_MP4=1");
        return;
    }

    if !Mp4Recorder::ffmpeg_available() {
        eprintln!("skipping: ffmpeg not found on PATH; cannot record/verify videos");
        return;
    }

    let test_name = "e2e_state_recording_replay_video_matches_live_video";
    let fps = env_u32("ROLLOUT_E2E_RECORD_FPS").unwrap_or(30);
    let hold_frames = env_usize("ROLLOUT_E2E_RECORD_HOLD_FRAMES")
        .unwrap_or(10)
        .max(1);

    let (width, height) = board_capture_dimensions();

    let video = VideoCaptureConfig {
        mp4: Mp4Config { width, height, fps },
        hold_frames,
    };

    // Deterministic, forward-only run (so replay = iterate 0..history_len).
    let logic = TetrisLogic::new(123, vec![Piece::T, Piece::O, Piece::I]);

    let actions = [
        InputAction::MoveLeft,
        InputAction::RotateCw,
        InputAction::SoftDrop,
        InputAction::SoftDrop,
        InputAction::MoveRight,
        InputAction::HardDrop,
        InputAction::MoveLeft,
        InputAction::RotateCcw,
        InputAction::SoftDrop,
        InputAction::HardDrop,
        InputAction::MoveRight,
        InputAction::Rotate180,
        InputAction::SoftDrop,
        InputAction::SoftDrop,
        InputAction::HardDrop,
    ];

    let artifacts = record_state_and_video_then_replay_and_compare(
        test_name,
        default_record_dir(),
        logic,
        actions,
        video,
        |state: &GameState, buf: &mut [u8], width: u32, height: u32| {
            let board = state.tetris.board_with_active_piece();
            let mut gfx = CpuRenderer::new(buf, SurfaceSize::new(width, height));
            draw_board(&mut gfx, &board);
        },
    )
    .expect("record/replay regression should succeed");

    eprintln!("state recording: {}", artifacts.state_json.display());
    eprintln!("live mp4:       {}", artifacts.live_mp4.display());
    eprintln!("replay mp4:     {}", artifacts.replay_mp4.display());
}

#[test]
fn e2e_rewind_restores_previous_frame() {
    let logic = TetrisLogic::new(123, vec![Piece::T]);
    let mut runner = HeadlessRunner::new(logic);
    let mut rec = E2eMp4::maybe_start("e2e_rewind_restores_previous_frame");
    let mut images = E2eImageCapture::maybe_start("e2e_rewind_restores_previous_frame");

    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "initial");
    }

    runner.step(InputAction::MoveLeft);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "after_move_left");
    }
    runner.step(InputAction::RotateCw);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "after_rotate");
    }
    let after_rotate = runner.state().tetris.snapshot();
    assert_eq!(after_rotate.current_piece_rotation, 1);

    runner.step(InputAction::MoveRight);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "after_move_right");
    }
    let after_move_right = runner.state().tetris.snapshot();
    assert_ne!(
        after_rotate.current_piece_pos,
        after_move_right.current_piece_pos
    );

    runner.rewind(1);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    if let Some(capture) = images.as_mut() {
        capture.capture(&runner, "after_rewind");
    }
    let rewound = runner.state().tetris.snapshot();
    assert_eq!(rewound.current_piece_pos, after_rotate.current_piece_pos);
    assert_eq!(
        rewound.current_piece_rotation,
        after_rotate.current_piece_rotation
    );

    if let Some(r) = rec.take() {
        r.finish();
    }
}
