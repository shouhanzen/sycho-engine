# Editor

The editor is a **game-agnostic** UI that connects to the **engine-provided editor API**
(HTTP endpoints returning `engine::editor::*` types).

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
