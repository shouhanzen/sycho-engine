# Testing Workflows

These presets are optimized for local iteration speed and reduced machine lag.

## Develop with live reload

Run `./go.sh --start --watch` to open Tetree and rebuild it when Rust sources or Cargo manifests change. The running game keeps playing during a build. After a successful build it commits a checkpoint and the replacement process resumes from it.

- Run `./go.sh --editor` for the embedded workspace with live reload. Click the preview to give the game keyboard and mouse input. Press **Tab** to switch focus between the preview and the editor.
- The editor's timeline dock shows the current position and the live tip. Drag the playhead to inspect saved checkpoints. **Back**, **Play/Pause**, and **Next** browse the recording. **Resume Live** continues the game from the selected checkpoint.
- The editor stays open through successful rebuilds. Build errors appear in the terminal, and the previous preview stays playable.
- `./go.sh --start` opens the same timeline without watching files.
- The default timeline is `target/timelines/headful`. Use `--timeline DIR` to select another one. Use `--fresh` for a separate timeline, optionally with a new directory of your choice.
- **Option+P** (F5) pauses live play or resumes live from the cursor. **Option+Left** (F6) and **Option+Right** (F7) step backward and forward. **Option+Space** (F8) plays or pauses the recorded sequence, stopping at the end. Use Alt in place of Option on other keyboards. Resuming live from an older checkpoint replaces the history after it.
- Use `./go.sh --start --playback --timeline DIR` to open a saved timeline paused at the beginning.
- The game queues a sample every 200 ms by default, with a full snapshot about once per second and up to four deltas in between. Set `SYCHO_CHECKPOINT_MS` to change the sample interval. A successful reload commits an exact full snapshot at handoff.
- The timeline stores timestamped, versioned JSON. Earlier live timelines stay readable and use nominal 250 ms timing for entries without timestamps. Change Tetree's timeline schema version in `game/src/state.rs` when older snapshots can no longer be read.

## Export video and use timeline fixtures

Close the game, then run `./go.sh --export-video video.mp4 --timeline DIR`. Export uses Tetree's renderer, preserves recorded pacing, and requires FFmpeg. You can specify its location with `--ffmpeg PATH`.

Tests can write a timeline with `Timeline::capture_at` and reopen it with `TimelineReader`. Use `load` to reconstruct a single checkpoint and `snapshots` to walk the committed sequence. `./go.sh --e2e --video` saves playtest timelines and exports MP4s from them.

## Recommended Local Loop

- Fast default loop (engine + game):
  - `./go.sh --test-fast`
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
