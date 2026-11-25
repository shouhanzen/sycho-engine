# Testing Workflows

These presets are optimized for local iteration speed and reduced machine lag.

## Develop with live reload

Run `./go.sh --start --watch` to open Tetree and rebuild it when Rust sources or Cargo manifests change. The running game keeps playing during a build. After a successful build it commits a checkpoint and the replacement process resumes from it.

- Run `./go.sh --editor` for the board with live reload. Drag the **Game** or **Timeline** title bar to move its panel and drag an edge or corner to resize it. Drag the background to pan and scroll or pinch to zoom. **Command+0** on macOS or **Ctrl+0** elsewhere brings both panels into view. The editor saves the workspace to `.sycho-board.json` in the project.
- To dock, drag the **Timeline** title bar toward the bottom of **Game** until the edge highlights, then release. Drag the **Game** title bar to move the pair, or resize it from an outer edge. Both panels take the group width and the Game takes the remaining height after the Timeline. Drag the **Timeline** title bar again to undock it.
- The **Game** title bar shows the render resolution. The game fills the panel at every zoom. Its slider sets the pixel size from 0.5× to 2×. At 2× the game renders at half the width and height and scales the pixels up sharply. Resizing the panel also changes the render resolution and reflows the game UI. Click the image to play and use **Tab** to switch between Game and Timeline. Use **Command+scroll** on macOS or **Ctrl+scroll** elsewhere to zoom the board over a focused preview.
- The **Timeline** panel shows the current position and the live tip. Drag the playhead to inspect saved checkpoints, or use the arrows to step through them. **Play/Pause** plays recorded history, pauses live play, or resumes live play from the latest checkpoint. Pause at an earlier checkpoint and click **Take Over** to resume live and replace the later recording from that point.
- On macOS, the editor and game share Metal preview surfaces when available. The editor prefers the high-performance GPU on dual-GPU Macs. Set `SYCHO_PREVIEW_TRANSPORT=rgba` to compare against the raw pixel transport.
- The editor stays open through successful rebuilds. Build errors appear in the terminal, and the previous preview stays playable.
- `./go.sh --start` opens the same timeline without watching files.
- The default timeline is `target/timelines/headful`. Use `--timeline DIR` to select another one. Use `--fresh` for a separate timeline, optionally with a new directory of your choice.
- **Option+P** (F5) pauses live play or resumes live from the cursor. **Option+Left** (F6) and **Option+Right** (F7) step backward and forward. **Option+Space** (F8) plays or pauses the recorded sequence, stopping at the end. Use Alt in place of Option on other keyboards. Resuming live from an older checkpoint replaces the history after it.
- Use `./go.sh --start --playback --timeline DIR` to open a saved timeline paused at the beginning.
- The game records state and game input after each simulation tick. The writer saves a full snapshot about once per second and batches intermediate deltas to disk about every 200 ms. Pause and reload commit the pending history and save an exact state. After a crash the timeline opens at the last committed batch.
- The timeline stores timestamped, versioned JSON. Playback requires format 3 with recorded timestamps. Start a new timeline with `--fresh` if a saved recording uses an earlier format. Change Tetree's timeline schema version in `game/src/state.rs` when older snapshots can no longer be read.

## Export video and use timeline fixtures

Close the game, then run `./go.sh --export-video video.mp4 --timeline DIR`. Export uses Tetree's renderer at 60 FPS, preserves recorded pacing and the recorded cursor, and requires FFmpeg. You can specify its location with `--ffmpeg PATH`.

Tests can write a timeline with `Timeline::capture_at` and reopen it with `TimelineReader`. Use `load` to reconstruct a single checkpoint and `snapshots` to walk the committed sequence. `./go.sh --e2e --video` saves playtest timelines and exports MP4s from them.

## Recommended Local Loop

- Default loop (engine + game):
  - `./go.sh --test`
- Lowest-lag loop (reduced parallelism):
  - `./go.sh --test-light`
- Run one crate only:
  - `cargo test -p game`
  - `cargo test -p engine`
- Run one integration test binary:
  - `cargo test -p game --test tetris_core_tests`
- Run one specific test:
  - `cargo test -p game --test tetris_core_tests piece_locks_after_cumulative_grounded_delay`

## When To Use `--test`

`./go.sh --test` runs `cargo test --workspace` for both the engine and game.

## Lower-CPU Knobs

For `./go.sh --test-light`, you can tune:

- `ROLLOUT_TEST_LIGHT_BUILD_JOBS` (default: `4`)
- `ROLLOUT_TEST_LIGHT_THREADS` (default: `1`)

Example:

`ROLLOUT_TEST_LIGHT_BUILD_JOBS=2 ROLLOUT_TEST_LIGHT_THREADS=1 ./go.sh --test-light`

## Build Cache Knobs

`go.sh` supports cache acceleration:

- Shared target dir across worktrees:
  - `ROLLOUT_SHARED_TARGET_DIR=1`
- `sccache` wrapper (if installed):
  - enabled automatically by `go.sh`
