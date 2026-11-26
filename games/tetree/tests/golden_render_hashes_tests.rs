use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use engine::{
    GameLogic,
    regression::{
        assert_or_update_golden_hashes, record_state_then_replay_and_compare_render_hashes,
        update_goldens_enabled,
    },
};
use game::board_render::{CELL_SIZE, draw_board};

fn unique_temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("rollout_engine_golden_hashes_{nanos}"))
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

    fn step(&self, state: &mut Self::State, input: Self::Input) {
        state[input.y][input.x] = input.v;
    }
}

#[test]
fn golden_gridgame_render_hashes_are_stable() {
    let name = "golden_gridgame_render_hashes_are_stable";
    let out_dir = unique_temp_dir();

    let game = GridGame {
        width: 4,
        height: 4,
    };
    let width = game.width as u32 * CELL_SIZE;
    let height = game.height as u32 * CELL_SIZE;

    let inputs = [
        SetCell { x: 0, y: 0, v: 1 },
        SetCell { x: 1, y: 0, v: 2 },
        SetCell { x: 2, y: 1, v: 3 },
        SetCell { x: 3, y: 2, v: 4 },
    ];

    let artifacts = record_state_then_replay_and_compare_render_hashes(
        name,
        &out_dir,
        game,
        inputs,
        width,
        height,
        |state, gfx| draw_board(gfx, state),
    )
    .expect("hash regression run should succeed");

    let golden_path = engine::regression_golden_path!(name);
    assert_or_update_golden_hashes(
        &golden_path,
        name,
        width,
        height,
        artifacts.replay_hashes,
        update_goldens_enabled(),
    )
    .unwrap_or_else(|e| {
        panic!(
            "golden check failed: {e}\n(hint: set ROLLOUT_UPDATE_GOLDENS=1 to generate/update {})",
            golden_path.display()
        )
    });

    // Clean up on success; keep the temp dir if the test fails.
    let _ = fs::remove_dir_all(artifacts.timeline);
    let _ = fs::remove_dir_all(out_dir);
}
