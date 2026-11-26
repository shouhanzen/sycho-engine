# Testing Workflows

These presets are optimized for local iteration speed and reduced machine lag.

## Develop with live reload

Run `./go.sh --start --watch` to open Tetree and rebuild it when Rust sources or Cargo manifests change. The running game keeps playing during a build. After a successful build it commits a checkpoint and the replacement process resumes from it.

- Run `./go.sh --editor` for the maximized board window with live reload. A fresh board opens Tetree from `games/tetree/`. Drag a panel title bar to move it and drag an edge or corner to resize it. Drag the background to pan and scroll or pinch to zoom. **Command+0** on macOS or **Ctrl+0** elsewhere brings all visible panels into view. The editor saves the workspace to `.sycho-board.json` in the project.
- Fresh boards start at 100% zoom. The editor and both example games use logical points for layout and the display’s pixels for rendering. The Game zoom slider still changes the size of the rendered pixels. Recorded game frames keep their logical viewport when played back in a different window or exported.
- Add **Tile Atlas** and click **Open…** to pick any `.atlas.json` file through the board's Explorer. Click a tile to set the board's active brush, then paint any Level Editor. **Erase** removes the sprite and collision. The **Plain** brush paints solid cells with the game's default color.
- An atlas definition contains `texture` (a PNG path relative to the definition), `tile_width`, and `tile_height`. The PNG must divide evenly into tiles. Platformer includes two definitions in `assets/tiles`. Each painted cell saves the path relative to its level file and the tile coordinates. Older maps can still resolve IDs through their project's `assets/tilesets.json`.
- Open **Texture Inspector** from the board's add menu or the atlas's **Inspect** button. **Open…** browses PNGs directly. Scroll to zoom, drag to pan, and click **Fit** to reset the view. In-use definitions and PNGs reload automatically. A missing tile uses a magenta-and-black checkerboard and retains its collision.
- Control-click a window header and choose **Duplicate**. The copy keeps the window's size and configured file, folder, or target. A docked panel copies as a free window. Game copies start their own recording at the state displayed by the original. Copies keep their own controls and survive reopening the board.
- Game copies share the project's code and static assets. Each copy has a separate timeline and runtime directory under the project's `.sycho/instances`. Game code can use `engine::launch::runtime_directory()` for instance-local saves. Tetree uses it for its preferences.
- To dock, drag the **Timeline** title bar toward the bottom of **Game** until the edge highlights, then release. Drag the **Game** title bar to move the pair, or resize it from an outer edge. Both panels take the group width and the Game takes the remaining height after the Timeline. Drag the **Timeline** title bar again to undock it.
- The **Game** title bar shows the render resolution and includes a master-volume slider. The speaker slider shares the in-game audio setting and works while the timeline is paused. The game fills the panel at every zoom. The zoom slider sets the pixel size from 0.5× to 2×. At 2× the game renders at half the width and height and scales the pixels up sharply. Resizing the panel also changes the render resolution and reflows the game UI. Click a game image to play. **Tab** cycles through connected games and then returns focus to the board. Use **Command+scroll** on macOS or **Ctrl+scroll** elsewhere to zoom the board over a focused preview.
- Click the fullscreen icon in a **Game** header to fill the display with that game. Press **Shift+Esc**, or move to the top-right corner and click **Exit fullscreen**, to return to the board. The Game zoom setting also applies in fullscreen.
- The **Timeline** panel shows the current position and the live tip. Drag the playhead to inspect saved checkpoints, or use the arrows to step through them. **Play/Pause** plays recorded history, pauses live play, or resumes live play from the latest checkpoint. Pause at an earlier checkpoint and click **Take Over** to resume live and replace the later recording from that point.
- Use the bottom-left **+** menu to add a **Game**, **Timeline**, **Profiler**, or **Explorer** window. An empty Game offers **New Game** and **Open Game**. New Game creates a standalone Rust project with a local Sycho dependency. Open Game takes a project folder or `Cargo.toml`. Both actions open the same Explorer offset below and to the right of the Game. Drag either title bar to move both windows together. The picker blocks the Game controls while the rest of the board stays usable. Click a folder to enter it, use the path field to jump to a folder, or select a location under **Recent projects**. Use **Command+scroll** on macOS or **Ctrl+scroll** elsewhere to zoom the board over Explorer. Use **Attach to Game** beside the Timeline or Profiler title. Click for a dropdown or drag the button to a Game window. Use **Detach** in the dropdown to clear the link. Closing a Timeline leaves that game running and recording. The board saves project links, empty windows, and per-game Enable/Disable state. Open **Profiler** and attach it to a Game to collect timings from that game. The board remembers the attachment. Click the graph to inspect a frame or click **Live** to follow the latest. The graph shows game draw CPU time, game GPU time when available, and the interval between editor presents. The breakdown shows the CPU hooks, render queue, GPU completion, estimated handoff, and editor work. Vertical marks show changes in game process, resolution, transport, or pause state. Red marks show skipped preview IDs.
- Run `./go.sh --profile` to capture the standalone game in a native window. After 200 warmup frames it records 600 frames, prints timing averages and maxima, and writes a Chrome trace under `target/perf_traces/`. Set `ROLLOUT_PROFILE_FRAMES` or `ROLLOUT_PROFILE_WARMUP` to change the capture length. The Draw metric covers the full game draw hook in both the trace and the editor. The standalone trace records the CPU-side present call and the editor reports preview submission, GPU completion, and compositing separately.
- On macOS, the editor and game share Metal preview surfaces when available. The editor prefers the high-performance GPU on dual-GPU Macs. Set `SYCHO_PREVIEW_TRANSPORT=rgba` to compare against the raw pixel transport.
- The editor stays open through successful rebuilds. Build errors appear in the terminal, and the previous preview stays playable.
- Use **Disable** in the Game header to checkpoint and stop that game. The editor blurs its last frame, and source changes wait until **Enable** builds again. Timeline controls become available when the game reconnects. The board remembers disabled games in `.sycho-board.json`. The frozen preview stays in the current editor session. If checkpointing fails, the game keeps running and the header offers **Retry**.
- `./go.sh --start` opens the same timeline without watching files.
- Use `./go.sh --start --no-recording` to play standalone with in-memory progress. With `--watch`, a successful rebuild starts a new session from defaults. The editor always records.
- New sessions start with default skill-tree and broken-wall progress. New rounds carry that progress forward from the current game state. Reopening a timeline restores the progress saved in it.
- Fresh sessions load player preferences from the settings file. Once a session starts, the timeline records preferences in the game state. Rewinding or taking over restores the preferences saved at that point. Edits made while paused enter the timeline when live play resumes.
- Each game keeps its default recording under `.sycho/timelines/<binary>` in its project. Tetree uses `games/tetree/.sycho/timelines/headful`. Its legacy default recording moves automatically the first time Tetree opens the new default path. Use `--timeline DIR` to select another one. Use `--fresh` for a separate timeline, optionally with a new directory of your choice.
- Click the editor’s **Game** panel to use timeline shortcuts. **Option+P** (F5) pauses live play or resumes live from the cursor. **Option+Left** (F6) and **Option+Right** (F7) step backward and forward. **Option+Space** (F8) plays or pauses the recorded sequence, stopping at the end. Use Alt in place of Option on other keyboards. Resuming live from an older checkpoint replaces the history after it.
- Use `./go.sh --start --playback --timeline DIR` to play a saved timeline from the beginning. A normal standalone launch resumes at the latest checkpoint.
- The game records state and input each tick. The writer saves a full snapshot about once per second and batches deltas about every 200 ms. Pause and reload commit the pending history. After a crash the timeline opens at the last committed batch.
- The newest two seconds stay at full resolution. In older history, the sample rate halves across progressively longer bands. Scrubbing, playback, and export use the original times of the retained samples. Existing samples from before retention was enabled keep their original density.
- Click the bookmark icon on the Timeline to save the current checkpoint, optionally with a name. Click a ledger entry to open it. A bookmark preserves the nearby samples still available on the active timeline and keeps its exact state even when **Take Over** replaces that history. When viewing a bookmark from a replaced branch, use the left arrow to return to the timeline or **Take Over** to resume from the bookmark.
- Click **Reset** in the Timeline header and confirm to clear the recording and restart the attached game at time zero. Saved bookmarks remain available in the ledger.
- The timeline stores timestamped, versioned JSON. Format 4 supports sparse history and bookmarks and can open format 3 recordings. Tetree's snapshot schema 2 records the viewport, dig camera, and round-timer display. Use `--fresh` for recordings made with schema 1.

## Edit Platformer levels

Open `games/Platformer/Cargo.toml` in a Game window. Move with the arrow keys or A/D and jump with Space, Up, or W. Add **Level Editor** from **+** and attach it to the Platformer Game window.

The level grid sits behind the other board panels. Click the toolbar or an exposed solid tile to select it. Use **Paint** or **Erase** on exposed cells, or drag the right or bottom edge to add or remove whole tiles. The board previews your changes as you drag. When you release, Sycho saves them in `games/Platformer/levels/courtyard.tiles.json` and refreshes the running game. Expanding the bounds again restores any tiles hidden by shrinking. Platformer keeps a fixed tile size and follows the player with a camera clamped to the level. Pause or scrub the timeline to see the current grid from a recorded player pose.

## Export video and use timeline fixtures

Close the game, then run `./go.sh --export-video video.mp4 --timeline DIR`. Export uses Tetree's renderer at 60 FPS, preserves recorded pacing and the recorded cursor, and requires FFmpeg. You can specify its location with `--ffmpeg PATH`.

Tests can write a timeline with `Timeline::capture_at` and reopen it with `TimelineReader`. Use `load` to reconstruct a single checkpoint and `snapshots` to walk the committed sequence. `./go.sh --e2e --video` saves playtest timelines and exports MP4s from them.

## Recommended Local Loop

- Default loop (engine + game):
  - `./go.sh --test`
- Lowest-lag loop (reduced parallelism):
  - `./go.sh --test-light`
- Run one crate only:
  - `cargo test -p tetree`
  - `cargo test -p engine`
- Run one integration test binary:
  - `cargo test -p tetree --test tetris_core_tests`
- Run one specific test:
  - `cargo test -p tetree --test tetris_core_tests piece_locks_after_cumulative_grounded_delay`

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
