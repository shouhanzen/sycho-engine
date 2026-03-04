## Cursor Cloud specific instructions

### Overview

Rollout is a Tetris-variant game engine (Rust workspace: `engine`, `game`, `editor/frontend/src-tauri`, `orca/plantool`). See `.cursorrules` for architecture and workflow guidance, `TESTING.md` for test commands, and `editor/README.md` for editor setup.

### Running tests

Use `./go.sh` as the control surface (see `./go.sh --help`):
- Fast loop: `./go.sh --test-fast` (engine + game)
- E2E: `./go.sh --e2e`
- Full workspace: `./go.sh --test`
- Profiler: `./go.sh --profile`

### Known pre-existing test failures (cloud VM)

- `golden_gridgame_render_hashes_are_stable` — golden render hashes are platform-dependent; set `ROLLOUT_UPDATE_GOLDENS=1` to regenerate if needed.
- `paused_state_holds_dig_camera_motion_and_depth_impulses` — intermittently fails due to missing audio device in headless environments (ALSA warnings).
- `render_tests` (`color_mapping_is_stable`, `draw_board_renders_bottom_row_at_buffer_bottom`, `draw_board_centers_board_in_larger_buffer`) — color mapping assertions are platform-dependent.

### Headful game and editor

The headful game (`./go.sh --start --game`) and Tauri editor require a display (X11/Wayland). In headless cloud VMs, use the editor API server standalone for HTTP-based testing:

```
ROLLOUT_EDITOR_API_ADDR="127.0.0.1:4000" cargo run -p game --bin editor_api
```

### Tauri editor on Ubuntu 24.04

Ubuntu 24.04 ships `webkit2gtk-4.1` but Tauri 1.x needs `webkit2gtk-4.0`. The VM snapshot includes pkg-config and .so symlinks from 4.1→4.0. If the Tauri crate fails to build with "cannot find -lwebkit2gtk-4.0", recreate the symlinks:

```bash
PKG=/usr/lib/x86_64-linux-gnu/pkgconfig
LIB=/usr/lib/x86_64-linux-gnu
sudo ln -sf $PKG/webkit2gtk-4.1.pc $PKG/webkit2gtk-4.0.pc
sudo ln -sf $PKG/javascriptcoregtk-4.1.pc $PKG/javascriptcoregtk-4.0.pc
sudo ln -sf $PKG/webkit2gtk-web-extension-4.1.pc $PKG/webkit2gtk-web-extension-4.0.pc
sudo ln -sf $LIB/libwebkit2gtk-4.1.so $LIB/libwebkit2gtk-4.0.so
sudo ln -sf $LIB/libjavascriptcoregtk-4.1.so $LIB/libjavascriptcoregtk-4.0.so
```

The Tauri build also requires `editor/frontend/dist/` and `editor/frontend/src-tauri/icons/icon.png` to exist (normally created by `npm run build` / `tauri init`). Create placeholder files if building the Tauri crate outside `tauri dev`.

### Rust toolchain

The workspace uses `edition = "2024"` which requires Rust >= 1.85.0. The VM snapshot has 1.85.0 installed as the default toolchain.
