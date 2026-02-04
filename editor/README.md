# Editor

The editor is a **game-agnostic** UI that connects to the **engine-provided editor API**
(HTTP endpoints returning `engine::editor::*` types). In this repo, the editor API is a proxy to a
running headful game instance, so the timeline reflects the external game you launch.

## Structure
- `frontend`: React + Vite UI wrapped as a **Tauri** desktop app (`frontend/src-tauri`)
- `agents`: agent profiles, prompts, and tooling (scaffolded)

## Run (local)
- Via `go.sh` (recommended):
  - Foreground (best for dev): `./go.sh --start --editor`
  - Background: `./go.sh --start --editor --detach` (stop with `./go.sh --stop --editor`)

- Engine editor API (example impl for this repo's game): `cargo run -p game --bin editor_api`
- Editor app: `cd editor/frontend && npm install && npm run tauri dev`

Set `VITE_EDITOR_API` to point the UI at a different backend base URL.

## Timeline behavior
- Click **Launch Headful Game** in the editor to start the game process.
- The editor API will attach to the running headful game and expose its timeline at `/api/agent/*`.
- Play in the game window; the timeline in the editor will update automatically as frames record.

## Build speed (multi-worktree)
- **Rust compile caching**: if you have [`sccache`](https://github.com/mozilla/sccache) installed, `go.sh` will auto-enable it.
  - Disable with: `ROLLOUT_DISABLE_SCCACHE=1`
- **Shared Cargo artifacts across worktrees**: set `ROLLOUT_SHARED_TARGET_DIR=1` (or set `CARGO_TARGET_DIR` yourself) to reuse build outputs between worktrees.
  - Note: concurrent Cargo builds will block on a file lock in the shared target directory.
