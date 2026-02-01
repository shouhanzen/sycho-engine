use engine::HeadlessRunner;
use engine::recording::{Mp4Config, Mp4Recorder};
use engine::render::{draw_board, CELL_SIZE};

use game::playtest::{InputAction, TetrisLogic};
use game::tetris_core::{Piece, Vec2i, BOARD_HEIGHT, BOARD_WIDTH};

use std::fs;
use std::path::PathBuf;
use std::sync::Once;

static WARNED_FFMPEG_MISSING: Once = Once::new();

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
        let hold_frames = env_usize("ROLLOUT_E2E_RECORD_HOLD_FRAMES").unwrap_or(10).max(1);

        // Render just the board, with a 1-cell padding so the outline is visible.
        let padding_cells = 1u32;
        let width = (BOARD_WIDTH as u32 + padding_cells * 2) * CELL_SIZE;
        let height = (BOARD_HEIGHT as u32 + padding_cells * 2) * CELL_SIZE;

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
        let board = runner.state().board_with_active_piece();
        draw_board(&mut self.buf, self.width, self.height, &board);
        for _ in 0..self.hold_frames {
            self.rec
                .push_rgba_frame(&self.buf)
                .expect("failed writing frame to ffmpeg");
        }
    }

    fn finish(mut self) {
        let out = self.rec.output().to_path_buf();
        self.rec
            .finish()
            .expect("ffmpeg failed while finalizing the mp4");
        let meta = fs::metadata(&out).expect("mp4 output missing after ffmpeg finished");
        assert!(meta.len() > 0, "mp4 output was empty");
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
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
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

#[test]
fn e2e_hard_drop_places_o_piece() {
    let logic = TetrisLogic::new(0, vec![Piece::O]);
    let mut runner = HeadlessRunner::new(logic);
    let mut rec = E2eMp4::maybe_start("e2e_hard_drop_places_o_piece");

    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }

    runner.step(InputAction::HardDrop);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    let snapshot = runner.state().snapshot();

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
    assert_eq!(snapshot.current_piece_pos, Vec2i::new(4, BOARD_HEIGHT as i32));
    assert!(!snapshot.game_over);

    if let Some(r) = rec.take() {
        r.finish();
    }
}

#[test]
fn e2e_rewind_restores_previous_frame() {
    let logic = TetrisLogic::new(123, vec![Piece::T]);
    let mut runner = HeadlessRunner::new(logic);
    let mut rec = E2eMp4::maybe_start("e2e_rewind_restores_previous_frame");

    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }

    runner.step(InputAction::MoveLeft);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    runner.step(InputAction::RotateCw);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    let after_rotate = runner.state().snapshot();
    assert_eq!(after_rotate.current_piece_rotation, 1);

    runner.step(InputAction::MoveRight);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    let after_move_right = runner.state().snapshot();
    assert_ne!(
        after_rotate.current_piece_pos,
        after_move_right.current_piece_pos
    );

    runner.rewind(1);
    if let Some(r) = rec.as_mut() {
        r.capture(&runner);
    }
    let rewound = runner.state().snapshot();
    assert_eq!(rewound.current_piece_pos, after_rotate.current_piece_pos);
    assert_eq!(
        rewound.current_piece_rotation,
        after_rotate.current_piece_rotation
    );

    if let Some(r) = rec.take() {
        r.finish();
    }
}
