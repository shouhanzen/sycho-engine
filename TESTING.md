# Testing Workflows

These presets are optimized for local iteration speed and reduced machine lag.

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

`./go.sh --test` runs `cargo test --workspace` and includes all workspace crates (including editor tooling). Use it for full validation, not the default inner loop.

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
