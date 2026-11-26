use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use engine::embedded::{EditorEvent, TimelineStatus};
#[cfg(target_os = "macos")]
use engine::shared_surface;
use engine::surface::{DisplayMetrics, SurfaceSize};
use engine::tiles::GridSize;
use engine::tilesets::AssetWatch;
use engine::timeline::{BookmarkEdit, ForkState, TimelineCommand};
use pixels::{PixelsBuilder, SurfaceTexture, wgpu};
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, ModifiersState, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::platform::run_return::EventLoopExtRunReturn;
use winit::window::{CursorIcon, Fullscreen, Window, WindowBuilder};

#[path = "art.rs"]
mod art;
#[path = "board.rs"]
mod board;
#[path = "bookmarks.rs"]
mod bookmarks;
#[path = "canvas.rs"]
mod canvas;
#[path = "connection.rs"]
mod connection;
#[path = "explorer.rs"]
mod explorer;
#[path = "level.rs"]
mod level;
#[path = "profiler.rs"]
mod profiler;
#[path = "render.rs"]
mod render;
#[path = "target.rs"]
mod target;
#[path = "text.rs"]
mod text;

use board::{
    Board, Bounds, Content, Edges, GameHeader, Panel, PanelMenu, Point, TITLE_HEIGHT,
    TimelineLayout, TimelineResetLayout, Transport, WindowId, fullscreen_exit_button, viewport,
};
use canvas::{Chrome, FullscreenChrome, GameChrome};
use connection::Snapshot;
pub use connection::{Bridge, Lifecycle, Phase};
use explorer::{
    Action as ExplorerAction, Directory, Explorer, Field, Layout as ExplorerLayout,
    Mode as ExplorerMode,
};
use profiler::ProfileLayout;
use render::{FrameRequest, Preview, ProfileClick, RenderWorker};

use crate::{
    Supervisor, engine_root,
    project::{self, Project},
    tetree,
};

const BOARD_PATH: &str = ".sycho-board.json";
const SEEK_INTERVAL: Duration = Duration::from_millis(55);
const SAVE_DELAY: Duration = Duration::from_millis(350);
const PANEL_DRAG_THRESHOLD: f64 = 6.0;
const BLUR_DURATION: Duration = Duration::from_millis(240);
const ANIMATION_INTERVAL: Duration = Duration::from_millis(16);
const FULLSCREEN_EXIT_HINT: Duration = Duration::from_secs(3);

enum UiEvent {
    AssetsChanged,
    DuplicateReady(board::PanelWindow, Result<String, String>),
    PreviewChanged(WindowId),
    ProjectRead(WindowId, Result<Vec<Project>, String>),
    ExplorerListed(WindowId, u64, Result<Directory, String>),
    RenderStopped,
}

struct Game {
    project: Project,
    timeline_directory: PathBuf,
    bridge: Bridge,
    supervisor: Supervisor,
    generation: Option<u64>,
    pending_volume: Option<(u64, u8)>,
    volume_request: u64,
}

impl Game {
    fn volume(&self, snapshot: &Snapshot) -> Option<u8> {
        let volume = snapshot
            .frame
            .as_ref()?
            .master_volume
            .filter(|_| snapshot.connected)?;
        Some(
            self.pending_volume
                .filter(|&(request, _)| volume.request.is_none_or(|applied| applied < request))
                .map_or(volume.percent, |(_, percent)| percent),
        )
    }

    fn set_volume(&mut self, percent: u8, commit: bool) {
        self.volume_request += 1;
        let request = self.volume_request;
        self.pending_volume = Some((request, percent));
        self.bridge.send(EditorEvent::MasterVolume {
            percent,
            request,
            commit,
        });
    }
}

struct Workspace {
    board: Board,
    games: BTreeMap<WindowId, Game>,
    retired: Vec<(Project, Supervisor)>,
    proxy: EventLoopProxy<UiEvent>,
    presented: render::Presented,
    release: bool,
    #[cfg(target_os = "macos")]
    gpu_device: Option<u64>,
}

impl Workspace {
    fn mount(
        &mut self,
        id: WindowId,
        project: Project,
        enabled: bool,
        dpi: f64,
        timeline: Option<PathBuf>,
    ) {
        let bridge = Bridge::new();
        if !enabled {
            bridge.finish_disable(None, None);
        }
        bridge.attach_window(
            id,
            self.proxy.clone(),
            #[cfg(target_os = "macos")]
            self.gpu_device,
        );
        bridge.resize_game(self.board.window(id).unwrap().game_display(dpi));
        let runtime = self
            .board
            .window(id)
            .unwrap()
            .instance()
            .map(|instance| project.instance_root(instance));
        let timeline = runtime
            .as_ref()
            .map(|root| root.join("timeline"))
            .or(timeline);
        let timeline_directory = timeline
            .clone()
            .unwrap_or_else(|| engine::launch::project_timeline(project.root(), &project.binary));
        let supervisor = Supervisor::start(
            project.clone(),
            bridge.clone(),
            self.release,
            timeline,
            runtime.map(|root| root.join("runtime")),
        );
        self.games.insert(
            id,
            Game {
                project,
                timeline_directory,
                bridge,
                supervisor,
                generation: None,
                pending_volume: None,
                volume_request: 0,
            },
        );
    }

    fn attach(&mut self, id: WindowId, project: Project, dpi: f64) -> std::io::Result<()> {
        if self
            .retired
            .iter()
            .any(|(other, worker)| other == &project && !worker.is_finished())
        {
            return Err(std::io::Error::other(
                "The game is still closing. Try again in a moment.",
            ));
        }
        self.board.attach_project(id, project.clone())?;
        self.mount(id, project, true, dpi, None);
        Ok(())
    }

    fn close(&mut self, id: WindowId) {
        if let Some(game) = self.games.remove(&id) {
            game.supervisor.request_stop();
            self.retired.push((game.project, game.supervisor));
        }
        self.board.close(id);
    }

    fn game_display(
        &self,
        id: WindowId,
        display: Display,
        fullscreen: Option<WindowId>,
    ) -> DisplayMetrics {
        let panel = self.board.window(id).unwrap();
        if fullscreen == Some(id) {
            panel.fullscreen_display(display.surface, display.dpi)
        } else {
            panel.game_display(display.dpi)
        }
    }

    fn sync_sizes(&self, display: Display, fullscreen: Option<WindowId>) {
        for (&id, game) in &self.games {
            game.bridge
                .resize_game(self.game_display(id, display, fullscreen));
        }
    }

    fn cleanup(&mut self) {
        let mut retired = std::mem::take(&mut self.retired);
        while let Some((project, worker)) = retired.pop() {
            if worker.is_finished() {
                worker.finish();
            } else {
                self.retired.push((project, worker));
            }
        }
    }

    fn finish(self) {
        for game in self.games.values() {
            game.supervisor.request_stop();
        }
        for (_, game) in self.games {
            game.supervisor.finish();
        }
        for (_, game) in self.retired {
            game.finish();
        }
    }
}

#[derive(Clone, Copy)]
enum Gesture {
    Pan {
        button: MouseButton,
        origin: Point,
        pan: Point,
    },
    Move {
        id: WindowId,
        origin: Point,
        bounds: Bounds,
        undock: bool,
        allow_dock: bool,
    },
    Resize {
        id: WindowId,
        origin: Point,
        bounds: Bounds,
        edges: Edges,
    },
    Zoom(WindowId),
    Volume(WindowId),
    PaintLevel(WindowId),
    PanAsset {
        id: WindowId,
        button: MouseButton,
        origin: Point,
        pan: Point,
    },
    ResizeLevel {
        id: WindowId,
        origin: Point,
        size: GridSize,
        edges: Edges,
    },
    Target {
        id: WindowId,
        start: Point,
        since: Option<Instant>,
    },
    Scrub {
        timeline: WindowId,
        game: WindowId,
    },
}

impl Gesture {
    fn level(self) -> Option<WindowId> {
        match self {
            Self::PaintLevel(id) | Self::ResizeLevel { id, .. } => Some(id),
            _ => None,
        }
    }

    fn button(self) -> MouseButton {
        match self {
            Self::Pan { button, .. } | Self::PanAsset { button, .. } => button,
            _ => MouseButton::Left,
        }
    }
}

enum GameKeyRoute {
    Gameplay,
    Timeline,
}

struct TargetDropdown {
    id: WindowId,
    offset: usize,
}

#[derive(Clone, Copy)]
struct ResetPrompt {
    timeline: WindowId,
    game: WindowId,
}

struct FullscreenGame {
    id: WindowId,
    previous_focus: Option<WindowId>,
    hint_until: Instant,
}

#[derive(Default)]
struct Interaction {
    cursor: Option<Point>,
    focused: Option<WindowId>,
    profile_clicks: BTreeMap<WindowId, ProfileClick>,
    preview_buttons: Vec<MouseButton>,
    preview_keys: HashMap<VirtualKeyCode, GameKeyRoute>,
    gesture: Option<Gesture>,
    drag_ms: Option<(WindowId, u64)>,
    pending_seek: Option<(WindowId, u64)>,
    last_seek: Option<Instant>,
    modifiers: ModifiersState,
    menu_open: bool,
    target_menu: Option<TargetDropdown>,
    header_menu: Option<target::HeaderMenu>,
    duplicating: BTreeSet<WindowId>,
    notice: Option<(String, Instant)>,
    bookmark_popup: Option<bookmarks::Popup>,
    reset_prompt: Option<ResetPrompt>,
    fullscreen: Option<FullscreenGame>,
    explorers: BTreeMap<WindowId, Explorer>,
    active_explorer: Option<WindowId>,
    levels: BTreeMap<WindowId, level::Editor>,
    selected_level: Option<WindowId>,
    last_level: Option<WindowId>,
    picked_tile: Option<art::Pick>,
    assets: art::Assets,
    last_asset_directory: Option<PathBuf>,
}

impl Interaction {
    fn fullscreen_game(&self) -> Option<WindowId> {
        self.fullscreen.as_ref().map(|fullscreen| fullscreen.id)
    }

    fn fullscreen_exit_visible(&self, display: Display, now: Instant) -> bool {
        self.fullscreen.as_ref().is_some_and(|fullscreen| {
            let size = display.logical();
            let button = fullscreen_exit_button(size);
            let left = (button.x - 16.0).max(0.0);
            let top_corner = Bounds::new(
                left,
                0.0,
                f64::from(size.width) - left,
                button.bottom() + 10.0,
            );
            now < fullscreen.hint_until
                || self.cursor.is_some_and(|point| top_corner.contains(point))
        })
    }

    fn enter_fullscreen(&mut self, workspace: &Workspace, id: WindowId, window_focused: bool) {
        self.fullscreen = Some(FullscreenGame {
            id,
            previous_focus: self.focused,
            hint_until: Instant::now() + FULLSCREEN_EXIT_HINT,
        });
        self.switch_playtest_focus(workspace, Some(id), window_focused);
    }

    fn leave_fullscreen(&mut self, workspace: &Workspace, window_focused: bool) {
        if let Some(fullscreen) = self.fullscreen.take() {
            let focus = fullscreen
                .previous_focus
                .filter(|id| workspace.games.contains_key(id));
            self.switch_playtest_focus(workspace, focus, window_focused);
        }
    }

    fn switch_playtest_focus(
        &mut self,
        workspace: &Workspace,
        focus: Option<WindowId>,
        window_focused: bool,
    ) {
        if self.focused == focus {
            if let Some(game) = focus.and_then(|id| workspace.games.get(&id)) {
                game.bridge.send(EditorEvent::ReleaseInput);
            }
            self.preview_buttons.clear();
            self.preview_keys.clear();
        } else {
            self.focus(workspace, focus, window_focused);
        }
    }

    fn reset_layout(&self, board: &Board, display: Display) -> Option<TimelineResetLayout> {
        let prompt = self.reset_prompt?;
        let panel = board.window(prompt.timeline)?;
        let button = TimelineLayout::new(panel.bounds).reset_button();
        Some(TimelineResetLayout::new(
            board.project(button),
            display.logical(),
        ))
    }

    fn confirm_reset(&mut self, workspace: &Workspace, window_focused: bool) {
        if let Some(prompt) = self.reset_prompt.take()
            && workspace
                .board
                .window(prompt.timeline)
                .and_then(|panel| panel.attached_game())
                == Some(prompt.game)
            && let Some(game) = workspace.games.get(&prompt.game)
        {
            let snapshot = game.bridge.snapshot();
            if snapshot.connected && snapshot.timeline().tip.is_some() {
                self.pending_seek = None;
                self.drag_ms = None;
                game.bridge.send(EditorEvent::ResetTimeline);
                self.focus(workspace, Some(prompt.game), window_focused);
            }
        }
    }

    fn target_menu(&self, workspace: &Workspace, display: Display) -> Option<target::Menu> {
        let menu = self.target_menu.as_ref()?;
        target::Menu::new(&workspace.board, menu.id, display.logical(), menu.offset)
    }

    fn bookmark_layout(
        &self,
        workspace: &Workspace,
        display: Display,
    ) -> Option<bookmarks::Layout> {
        let popup = self.bookmark_popup.as_ref()?;
        let panel = workspace.board.window(popup.window)?;
        let button = TimelineLayout::new(panel.bounds).bookmark_button();
        Some(bookmarks::Layout::new(
            workspace.board.project(button),
            display.logical(),
            popup.entries.len().max(usize::from(popup.error.is_some())),
        ))
    }

    fn sync_bookmarks(&mut self, workspace: &Workspace) {
        if let Some(popup) = &mut self.bookmark_popup {
            if workspace
                .board
                .window(popup.window)
                .and_then(|panel| panel.attached_game())
                == Some(popup.game)
                && let Some(game) = workspace.games.get(&popup.game)
            {
                let snapshot = game.bridge.snapshot();
                popup.sync(
                    &game.timeline_directory,
                    snapshot.timeline(),
                    snapshot.connected,
                );
            } else {
                self.bookmark_popup = None;
            }
        }
    }

    fn save_bookmark(&mut self, workspace: &Workspace) {
        if let Some(popup) = self.bookmark_popup.as_mut().filter(|popup| popup.can_save)
            && let Some(game) = workspace.games.get(&popup.game)
        {
            let name = std::mem::take(&mut popup.name);
            game.bridge
                .send(EditorEvent::Bookmark(BookmarkEdit::Save(name)));
            popup.offset = 0;
        }
    }

    fn bookmark_click(&mut self, workspace: &Workspace, display: Display, point: Point) {
        let Some(layout) = self.bookmark_layout(workspace, display) else {
            self.bookmark_popup = None;
            return;
        };
        if !layout.bounds.contains(point) {
            self.bookmark_popup = None;
            return;
        }
        if layout.save.contains(point) {
            self.save_bookmark(workspace);
            return;
        }
        let Some(popup) = self.bookmark_popup.as_ref().filter(|popup| popup.connected) else {
            return;
        };
        let Some(game) = workspace.games.get(&popup.game) else {
            return;
        };
        let offset = layout.offset(popup);
        for (index, bookmark) in popup
            .entries
            .iter()
            .skip(offset)
            .take(layout.visible)
            .enumerate()
        {
            if layout.remove(index).contains(point) {
                if popup.selected != Some(bookmark.id) {
                    game.bridge
                        .send(EditorEvent::Bookmark(BookmarkEdit::Remove(bookmark.id)));
                }
                break;
            }
            if layout.row(index).contains(point) {
                game.bridge
                    .send(EditorEvent::Timeline(TimelineCommand::OpenBookmark(
                        bookmark.id,
                    )));
                self.bookmark_popup = None;
                break;
            }
        }
    }

    fn scroll_bookmarks(&mut self, workspace: &Workspace, display: Display, step: isize) {
        if let Some(layout) = self.bookmark_layout(workspace, display)
            && let Some(popup) = &mut self.bookmark_popup
        {
            popup.offset = layout
                .offset(popup)
                .saturating_add_signed(step)
                .min(popup.entries.len().saturating_sub(layout.visible));
        }
    }

    fn target_at(&self, board: &Board, display: Display) -> Option<WindowId> {
        let point = self.cursor?;
        if !viewport(display.logical()).contains(point)
            || PanelMenu::new(display.logical()).button.contains(point)
        {
            return None;
        }
        board.game_at(board.world_at(point))
    }

    fn focus(&mut self, workspace: &Workspace, id: Option<WindowId>, window_focused: bool) {
        if self.focused == id {
            return;
        }
        if let Some(old) = self.focused.and_then(|old| workspace.games.get(&old)) {
            old.bridge.send(EditorEvent::ReleaseInput);
            old.bridge.send(EditorEvent::Focus(false));
        }
        self.focused = id;
        self.preview_buttons.clear();
        self.preview_keys.clear();
        if let Some(game) = id.and_then(|id| workspace.games.get(&id)) {
            game.bridge.send(EditorEvent::Focus(window_focused));
        }
    }

    fn sync_profiling(&mut self, workspace: &Workspace) {
        let targets: BTreeSet<_> = workspace
            .board
            .windows()
            .filter(|panel| panel.kind() == Panel::Profiler)
            .filter_map(|panel| panel.attached_game())
            .collect();
        for (&id, game) in &workspace.games {
            game.bridge.set_profiling(targets.contains(&id));
        }
    }

    fn sync_level_editor(&mut self, workspace: &Workspace) {
        let mut changed = false;
        self.levels.retain(|id, _| {
            let keep = workspace.board.window(*id).is_some();
            changed |= !keep;
            keep
        });
        for panel in workspace
            .board
            .windows()
            .filter(|panel| panel.kind() == Panel::LevelEditor)
        {
            let target = panel.attached_game().and_then(|id| {
                let game = workspace.games.get(&id)?;
                let stamp = game.bridge.snapshot().frame?.tile_level?;
                Some((id, &game.project, stamp))
            });
            let editor = self.levels.entry(panel.id).or_default();
            if let Some(replaced) = editor.sync(target) {
                if replaced
                    && self
                        .gesture
                        .is_some_and(|gesture| gesture.level() == Some(panel.id))
                {
                    self.gesture = None;
                }
                changed = true;
            }
            editor.set_selected(self.selected_level == Some(panel.id));
        }
        if changed {
            self.sync_assets(workspace);
        }
    }

    fn sync_assets(&mut self, workspace: &Workspace) {
        self.assets
            .sync(&workspace.board, &self.levels, self.picked_tile.as_ref());
    }

    fn select_level(&mut self, selected: Option<WindowId>) {
        if let Some(previous) = self.selected_level.and_then(|id| self.levels.get_mut(&id)) {
            previous.set_selected(false);
        }
        self.selected_level = selected;
        if let Some(id) = selected {
            self.last_level = Some(id);
            self.levels.entry(id).or_default().set_selected(true);
        }
    }

    fn panel_at(&self, board: &Board, point: Point) -> Option<WindowId> {
        board.panel_at(point).or_else(|| {
            board
                .windows()
                .rev()
                .find(|panel| {
                    self.levels
                        .get(&panel.id)
                        .is_some_and(|editor| editor.hits_grid(panel, point, board.zoom))
                })
                .map(|panel| panel.id)
        })
    }

    fn game_point(
        &self,
        workspace: &Workspace,
        id: WindowId,
        snapshot: &Snapshot,
        display: Display,
    ) -> Option<(u32, u32)> {
        let cursor = self.cursor?;
        let frame = snapshot.frame.as_ref()?;
        let size = frame.size;
        let fullscreen = self.fullscreen_game();
        let requested = workspace.game_display(id, display, fullscreen);
        if size != requested.render_size
            || frame.display.is_some_and(|display| display != requested)
        {
            return None;
        }
        if let Some(active) = fullscreen {
            if id != active
                || (self.fullscreen_exit_visible(display, Instant::now())
                    && fullscreen_exit_button(display.logical()).contains(cursor))
            {
                return None;
            }
            let physical = Point::new(cursor.x * display.dpi, cursor.y * display.dpi);
            return viewport(display.surface).pixel(physical, size);
        }
        if !viewport(display.logical()).contains(cursor)
            || self.target_menu.is_some()
            || self.header_menu.is_some()
            || self.menu_open
            || PanelMenu::new(display.logical()).button.contains(cursor)
        {
            return None;
        }
        let world = workspace.board.world_at(cursor);
        let panel = workspace.board.window(id)?;
        if workspace.board.panel_at(world) != Some(id)
            || (workspace.board.dock_group(id).is_none() && panel.bounds.grip().contains(world))
        {
            return None;
        }
        panel.game_body().pixel(world, size)
    }

    fn dock_target(&self, board: &Board) -> Option<WindowId> {
        let Some(Gesture::Move {
            id,
            bounds,
            allow_dock: true,
            ..
        }) = self.gesture
        else {
            return None;
        };
        let window = board.window(id)?;
        if window.kind() != Panel::Timeline
            || (window.bounds.x - bounds.x).hypot(window.bounds.y - bounds.y) * board.zoom
                < PANEL_DRAG_THRESHOLD
        {
            return None;
        }
        board.dock_candidate(id)
    }

    fn pointer(&mut self, workspace: &mut Workspace, cursor: Option<Point>, display: Display) {
        let focus = self.focused.and_then(|id| {
            let game = workspace.games.get(&id)?;
            let snapshot = game.bridge.snapshot();
            let previous = self.game_point(workspace, id, &snapshot, display);
            Some((id, snapshot, previous))
        });
        self.cursor = cursor;
        let board = &mut workspace.board;
        match (self.gesture, cursor) {
            (Some(Gesture::Pan { origin, pan, .. }), Some(point)) => {
                board.pan = Point::new(pan.x + point.x - origin.x, pan.y + point.y - origin.y);
            }
            (
                Some(Gesture::PanAsset {
                    id, origin, pan, ..
                }),
                Some(point),
            ) => {
                if let Some(view) = self.assets.panels.get_mut(&id) {
                    let at = board.world_at(point);
                    view.camera.pan = Point::new(pan.x + at.x - origin.x, pan.y + at.y - origin.y);
                }
            }
            (
                Some(Gesture::Move {
                    id,
                    origin,
                    bounds,
                    undock,
                    ..
                }),
                Some(point),
            ) => {
                let at = board.world_at(point);
                let delta = Point::new(at.x - origin.x, at.y - origin.y);
                if undock {
                    if delta.x.hypot(delta.y) * board.zoom < PANEL_DRAG_THRESHOLD {
                        return;
                    }
                    board.undock(id);
                    board.raise(id);
                    self.gesture = Some(Gesture::Move {
                        id,
                        origin,
                        bounds,
                        undock: false,
                        allow_dock: false,
                    });
                }
                board.set_bounds(id, bounds.translated(delta));
            }
            (
                Some(Gesture::Resize {
                    id,
                    origin,
                    bounds,
                    edges,
                }),
                Some(point),
            ) => {
                if let Some(window) = board.window(id) {
                    let at = board.world_at(point);
                    board.set_bounds(
                        id,
                        edges.resize(
                            window.kind(),
                            bounds,
                            Point::new(at.x - origin.x, at.y - origin.y),
                            display.dpi,
                        ),
                    );
                }
            }
            (Some(Gesture::Zoom(id)), Some(point)) => {
                if let Some(window) = board.window(id) {
                    board.set_game_zoom(
                        id,
                        GameHeader::new(window.bounds).zoom_at(board.world_at(point).x),
                    );
                }
            }
            (Some(Gesture::Volume(id)), Some(point)) => {
                if let (Some(window), Some(game)) = (board.window(id), workspace.games.get_mut(&id))
                {
                    let percent = GameHeader::new(window.bounds).volume_at(board.world_at(point).x);
                    if game.pending_volume.is_none_or(|(_, old)| old != percent) {
                        game.set_volume(percent, false);
                    }
                }
            }
            (
                Some(Gesture::Target {
                    id,
                    start,
                    since: None,
                }),
                Some(point),
            ) if (point.x - start.x).hypot(point.y - start.y) >= PANEL_DRAG_THRESHOLD => {
                self.gesture = Some(Gesture::Target {
                    id,
                    start,
                    since: Some(Instant::now()),
                });
            }
            (Some(Gesture::PaintLevel(id)), point) => {
                if let Some(panel) = board.window(id)
                    && let Some(editor) = self.levels.get_mut(&id)
                {
                    let layout = editor.layout(panel);
                    let cell = point
                        .map(|point| board.world_at(point))
                        .filter(|point| board.panel_at(*point).is_none())
                        .and_then(|point| layout.cell_at(point));
                    editor.paint_to(cell, self.picked_tile.as_ref(), |candidate| {
                        let tile = layout.row(candidate.x, candidate.y, candidate.x + 1);
                        let center = Point::new(tile.x + tile.w / 2.0, tile.y + tile.h / 2.0);
                        Some(candidate) == cell || board.panel_at(center).is_none()
                    });
                }
            }
            (
                Some(Gesture::ResizeLevel {
                    id,
                    origin,
                    size,
                    edges,
                }),
                Some(point),
            ) => {
                let at = board.world_at(point);
                let delta = Point::new(at.x - origin.x, at.y - origin.y);
                if let Some(editor) = self.levels.get_mut(&id) {
                    editor.resize_to(level::snapped_size(size, edges, delta));
                    self.sync_assets(workspace);
                }
            }
            (Some(Gesture::Scrub { timeline, game }), _) => {
                self.seek_cursor(workspace, timeline, game)
            }
            (None, _) => {
                if let Some((id, snapshot, previous)) = focus {
                    let position = self.game_point(workspace, id, &snapshot, display);
                    if position.is_some() || previous.is_some() || !self.preview_buttons.is_empty()
                    {
                        let size = workspace
                            .game_display(id, display, self.fullscreen_game())
                            .render_size;
                        workspace.games[&id].bridge.send(EditorEvent::Pointer(
                            position.or(Some((size.width, size.height))),
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    fn seek_cursor(&mut self, workspace: &Workspace, timeline: WindowId, game: WindowId) {
        if let (Some(point), Some(window), Some(snapshot)) = (
            self.cursor,
            workspace.board.window(timeline),
            workspace
                .games
                .get(&game)
                .map(|game| game.bridge.snapshot()),
        ) && let Some(tip) = snapshot.timeline().tip
        {
            let track = TimelineLayout::new(window.bounds).track;
            let x = workspace.board.world_at(point).x;
            let fraction = ((x - track.x) / track.w).clamp(0.0, 1.0);
            let at = (tip.at_ms as f64 * fraction).round() as u64;
            self.drag_ms = Some((timeline, at));
            self.pending_seek = Some((game, if x >= track.right() { u64::MAX } else { at }));
        }
    }

    fn flush_seek(&mut self, workspace: &Workspace, force: bool) {
        if (force
            || self
                .last_seek
                .is_none_or(|last| last.elapsed() >= SEEK_INTERVAL))
            && let Some((id, at)) = self.pending_seek.take()
            && let Some(game) = workspace.games.get(&id)
        {
            game.bridge
                .send(EditorEvent::Timeline(TimelineCommand::Seek(at)));
            self.last_seek = Some(Instant::now());
        }
    }

    fn wait(&self, save_at: Option<Instant>, redraw_at: Option<Instant>) -> ControlFlow {
        let seek = self
            .pending_seek
            .and(self.last_seek)
            .map(|last| last + SEEK_INTERVAL);
        seek.into_iter()
            .chain(save_at)
            .chain(redraw_at)
            .min()
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
    }

    fn end_gesture(&mut self, workspace: &mut Workspace, display: Display, commit: bool) {
        let target = if commit {
            self.dock_target(&workspace.board)
        } else {
            None
        };
        match self.gesture.take() {
            Some(Gesture::Target { id, since, .. }) => {
                if commit {
                    if since.is_some() {
                        if let Some(game) = self.target_at(&workspace.board, display) {
                            workspace.board.attach_target(id, Some(game));
                        }
                    } else if let Some(point) = self.cursor
                        && viewport(display.logical()).contains(point)
                        && workspace
                            .board
                            .window(id)
                            .and_then(|window| window.target_button())
                            .is_some_and(|bounds| bounds.contains(workspace.board.world_at(point)))
                    {
                        self.target_menu = Some(TargetDropdown { id, offset: 0 });
                    }
                }
            }
            Some(Gesture::PaintLevel(id) | Gesture::ResizeLevel { id, .. }) => {
                if let Some(editor) = self.levels.get_mut(&id) {
                    if commit
                        && let Some(target) = workspace
                            .board
                            .window(id)
                            .and_then(|panel| panel.attached_game())
                        && let Some(game) = workspace.games.get(&target)
                    {
                        if editor.save(target, &game.project, self.picked_tile.as_ref()) {
                            let level = editor.level_id().map(str::to_owned);
                            for runtime in workspace
                                .games
                                .values()
                                .filter(|runtime| runtime.project == game.project)
                            {
                                runtime.bridge.send(EditorEvent::ReloadTiles);
                            }
                            if let Some(level) = level {
                                for (&other, editor) in &mut self.levels {
                                    if other != id {
                                        editor.reload_shared(&game.project, &level);
                                    }
                                }
                            }
                        }
                    } else {
                        editor.cancel();
                    }
                }
                self.sync_assets(workspace);
            }
            Some(Gesture::Scrub { .. }) => {
                self.flush_seek(workspace, true);
                self.drag_ms = None;
            }
            Some(Gesture::Volume(id)) => {
                if let Some(game) = workspace.games.get_mut(&id)
                    && let Some((_, percent)) = game.pending_volume
                {
                    game.set_volume(percent, true);
                }
            }
            Some(Gesture::Move { id, .. }) => {
                if let Some(target) = target {
                    workspace.board.dock(id, target);
                }
            }
            _ => {}
        }
    }

    fn mouse(
        &mut self,
        workspace: &mut Workspace,
        display: Display,
        button: MouseButton,
        state: ElementState,
        window_focused: bool,
    ) {
        if state == ElementState::Released {
            if self
                .gesture
                .is_some_and(|gesture| gesture.button() == button)
            {
                self.end_gesture(workspace, display, true);
            }
            if self.preview_buttons.contains(&button) {
                self.preview_buttons.retain(|held| *held != button);
                if let Some(game) = self.focused.and_then(|id| workspace.games.get(&id)) {
                    game.bridge.send(EditorEvent::Button(button, state));
                }
            }
            return;
        }
        if self.gesture.is_some() {
            return;
        }
        let Some(point) = self.cursor else { return };
        if let Some(id) = self.fullscreen_game() {
            if button == MouseButton::Left
                && self.fullscreen_exit_visible(display, Instant::now())
                && fullscreen_exit_button(display.logical()).contains(point)
            {
                self.leave_fullscreen(workspace, window_focused);
            } else if let Some(game) = workspace.games.get(&id) {
                let snapshot = game.bridge.snapshot();
                if snapshot.connected {
                    self.send_game_press(workspace, id, &snapshot, display, button);
                }
            }
            return;
        }
        if self.reset_prompt.is_some() {
            if button == MouseButton::Left {
                if self
                    .reset_layout(&workspace.board, display)
                    .is_some_and(|layout| layout.confirm.contains(point))
                {
                    self.confirm_reset(workspace, window_focused);
                } else {
                    self.reset_prompt = None;
                }
            }
            return;
        }
        if self.bookmark_popup.is_some() {
            if button == MouseButton::Left {
                self.bookmark_click(workspace, display, point);
            }
            return;
        }
        if self.target_menu.is_some() {
            if button == MouseButton::Left {
                self.select_target_from_menu(workspace, display, point);
            }
            return;
        }
        if let Some(menu) = self.header_menu.take() {
            if button == MouseButton::Left && !menu.pending && menu.bounds.contains(point) {
                self.duplicate_panel(workspace, menu.source, display);
            }
            return;
        }
        self.notice = None;
        let menu = PanelMenu::new(display.logical());
        if button == MouseButton::Left && menu.button.contains(point) {
            self.focus(workspace, None, window_focused);
            self.active_explorer = None;
            self.menu_open = !self.menu_open;
            return;
        }
        if self.menu_open {
            if button == MouseButton::Left
                && let Some(kind) = menu.panel_at(point)
            {
                let id = workspace.board.add(kind, viewport(display.logical()));
                if kind == Panel::Explorer {
                    self.start_explorer(workspace, id, ExplorerMode::Browse, explorer::games());
                    self.active_explorer = Some(id);
                }
                self.select_level((kind == Panel::LevelEditor).then_some(id));
            }
            self.menu_open = false;
            return;
        }

        let world = workspace.board.world_at(point);
        let panel = self.panel_at(&workspace.board, world);
        if (button == MouseButton::Right || button == MouseButton::Left && self.modifiers.ctrl())
            && let Some(id) = panel
            && let Some(window) = workspace.board.window(id)
            && window.chrome_bounds().contains(world)
            && world.y < window.bounds.y + TITLE_HEIGHT
            && workspace.board.picker(id).is_none()
        {
            self.focus(workspace, None, window_focused);
            let mut menu = target::HeaderMenu::new(id, point, display.logical());
            menu.pending = self.duplicating.contains(&id);
            self.header_menu = Some(menu);
            return;
        }

        if button == MouseButton::Left
            && let Some(id) = panel
            && let Some(game) = workspace.games.get(&id)
            && let Some(window) = workspace.board.window(id)
            && GameHeader::new(window.bounds)
                .fullscreen_button
                .contains(world)
        {
            if game.bridge.snapshot().connected {
                self.enter_fullscreen(workspace, id, window_focused);
            }
            return;
        }
        let was_level_selected = panel.is_some() && panel == self.selected_level;
        if button == MouseButton::Left {
            let selected = panel.filter(|&id| {
                workspace
                    .board
                    .window(id)
                    .is_some_and(|window| window.kind() == Panel::LevelEditor)
            });
            self.select_level(selected);
        }
        if button == MouseButton::Middle
            && let Some(id) = panel
            && let Some(window) = workspace.board.window(id)
            && workspace.board.picker(id).is_none()
            && let Some(view) = self.assets.panels.get(&id)
            && view.image.is_some()
            && art::Layout::new(window.bounds).image.contains(world)
        {
            self.gesture = Some(Gesture::PanAsset {
                id,
                button,
                origin: world,
                pan: view.camera.pan,
            });
            return;
        }
        if button == MouseButton::Middle || (panel.is_none() && button == MouseButton::Left) {
            self.focus(workspace, None, window_focused);
            if button == MouseButton::Left {
                self.active_explorer = None;
            }
            self.gesture = Some(Gesture::Pan {
                button,
                origin: point,
                pan: workspace.board.pan,
            });
            return;
        }
        let Some(id) = panel else { return };
        let panel = workspace.board.window(id).unwrap().clone();
        let picker = workspace.board.picker(id);
        if let Some(owner) = panel.owner.or(picker.map(|_| id)) {
            if button == MouseButton::Left {
                workspace.board.raise(owner);
                self.focus(workspace, None, window_focused);
                self.active_explorer = Some(picker.unwrap_or(id));
                if panel.stack_drag_at(world) {
                    self.gesture = Some(Gesture::Move {
                        id: owner,
                        origin: world,
                        bounds: workspace.board.window(owner).unwrap().bounds,
                        undock: false,
                        allow_dock: false,
                    });
                } else if panel.owner.is_some() {
                    if panel.close_button().contains(world) {
                        self.close_explorer(workspace, id);
                    } else {
                        self.explorer_click(workspace, id, panel.bounds, display, world);
                    }
                }
            }
            return;
        }
        if panel.kind() == Panel::Explorer {
            if button == MouseButton::Left {
                workspace.board.raise(id);
                self.focus(workspace, None, window_focused);
                self.active_explorer = Some(id);
                if panel.close_button().contains(world) {
                    self.close_explorer(workspace, id);
                    return;
                }
                if world.y >= panel.bounds.y + TITLE_HEIGHT
                    && Edges::within(panel.bounds, workspace.board.zoom, world).is_none()
                {
                    self.explorer_click(workspace, id, panel.bounds, display, world);
                    return;
                }
            } else {
                return;
            }
        } else {
            self.active_explorer = None;
        }
        if button == MouseButton::Left {
            workspace.board.raise(id);
            if panel.close_button().contains(world) {
                self.focus(workspace, None, window_focused);
                if self.selected_level == Some(id) {
                    self.select_level(None);
                }
                workspace.close(id);
                return;
            }
            if panel
                .target_button()
                .is_some_and(|button| button.contains(world))
            {
                self.focus(workspace, None, window_focused);
                self.gesture = Some(Gesture::Target {
                    id,
                    start: point,
                    since: None,
                });
                return;
            }
            if panel.kind() == Panel::Timeline
                && TimelineLayout::new(panel.bounds)
                    .reset_button()
                    .contains(world)
                && let Some(game) = panel.attached_game()
                && let Some(runtime) = workspace.games.get(&game)
            {
                let snapshot = runtime.bridge.snapshot();
                if snapshot.connected && snapshot.timeline().tip.is_some() {
                    self.focus(workspace, None, window_focused);
                    self.reset_prompt = Some(ResetPrompt { timeline: id, game });
                }
                return;
            }
            if panel.kind() == Panel::Game {
                if let Some(game) = workspace.games.get(&id) {
                    let header = GameHeader::new(panel.bounds);
                    if header.lifecycle_button.contains(world) {
                        let bridge = game.bridge.clone();
                        self.focus(workspace, None, window_focused);
                        bridge.toggle_lifecycle();
                        return;
                    }
                    if header.hit_zoom(world) {
                        workspace.board.set_game_zoom(id, header.zoom_at(world.x));
                        self.gesture = Some(Gesture::Zoom(id));
                        return;
                    }
                    if header.hit_volume(world) && game.volume(&game.bridge.snapshot()).is_some() {
                        workspace
                            .games
                            .get_mut(&id)
                            .unwrap()
                            .set_volume(header.volume_at(world.x), false);
                        self.gesture = Some(Gesture::Volume(id));
                        return;
                    }
                } else {
                    let (new, open) = panel.project_buttons();
                    if new.contains(world) || open.contains(world) {
                        self.focus(workspace, None, window_focused);
                        let mode = if new.contains(world) {
                            ExplorerMode::Create
                        } else {
                            ExplorerMode::Open
                        };
                        if let Some(picker) = workspace.board.open_picker(id) {
                            let directory = workspace
                                .board
                                .recent_projects()
                                .first()
                                .cloned()
                                .unwrap_or_else(explorer::games);
                            self.start_explorer(workspace, picker, mode, directory);
                            self.active_explorer = Some(picker);
                        }
                        return;
                    }
                }
            }
            if panel.kind() == Panel::LevelEditor {
                self.focus(workspace, None, window_focused);
                let editor = self.levels.entry(id).or_default();
                let layout = editor.layout(&panel);
                if editor.size().is_some() && layout.paint.contains(world) {
                    editor.select_brush(level::Brush::Paint);
                    self.picked_tile = None;
                    self.sync_assets(workspace);
                } else if editor.size().is_some() && layout.erase.contains(world) {
                    editor.select_brush(level::Brush::Erase);
                } else if layout.toolbar.contains(world) {
                    self.gesture = Some(Gesture::Move {
                        id,
                        origin: world,
                        bounds: panel.bounds,
                        undock: false,
                        allow_dock: false,
                    });
                } else if was_level_selected {
                    if let Some(edges) = layout.resize_edges(world, workspace.board.zoom)
                        && let Some(size) = editor.size()
                    {
                        self.gesture = Some(Gesture::ResizeLevel {
                            id,
                            origin: world,
                            size,
                            edges,
                        });
                    } else if let Some(cell) = layout.cell_at(world) {
                        editor.paint_to(Some(cell), self.picked_tile.as_ref(), |_| true);
                        self.gesture = Some(Gesture::PaintLevel(id));
                    }
                }
                return;
            }
            let dock = workspace.board.dock_group(id);
            let outer = workspace.board.dock_bounds(id).unwrap_or(panel.bounds);
            let resize_id = dock.map_or(id, |(game, _)| game);
            if let Some(edges) = Edges::within(outer, workspace.board.zoom, world)
                && workspace.board.picker(resize_id).is_none()
            {
                self.focus(workspace, None, window_focused);
                self.gesture = Some(Gesture::Resize {
                    id: resize_id,
                    origin: world,
                    bounds: workspace.board.window(resize_id).unwrap().bounds,
                    edges,
                });
                return;
            }
            if world.y < panel.bounds.y + TITLE_HEIGHT {
                self.focus(workspace, None, window_focused);
                if panel.kind() == Panel::Profiler
                    && ProfileLayout::new(panel.bounds).live.contains(world)
                {
                    self.profile_clicks.insert(
                        id,
                        ProfileClick {
                            game: panel.attached_game(),
                            bounds: panel.bounds,
                            point: world,
                        },
                    );
                    return;
                }
                self.gesture = Some(Gesture::Move {
                    id,
                    origin: world,
                    bounds: panel.bounds,
                    undock: dock.is_some_and(|(_, timeline)| timeline == id),
                    allow_dock: dock.is_none(),
                });
                return;
            }
        }

        if self.assets.panels.contains_key(&id) {
            if button == MouseButton::Left {
                self.focus(workspace, None, window_focused);
                self.asset_click(workspace, display, &panel, world);
            }
            return;
        }

        if panel.kind() == Panel::LevelEditor {
            return;
        }

        if let Some(snapshot) = workspace.games.get(&id).map(|game| game.bridge.snapshot())
            && snapshot.connected
        {
            self.focus(workspace, Some(id), window_focused);
            self.send_game_press(workspace, id, &snapshot, display, button);
        } else if button == MouseButton::Left {
            let mut focus = None;
            if panel.kind() == Panel::Profiler {
                self.profile_clicks.insert(
                    id,
                    ProfileClick {
                        game: panel.attached_game(),
                        bounds: panel.bounds,
                        point: world,
                    },
                );
            } else if let Some(game) = panel.attached_game()
                && let Some(runtime) = workspace.games.get(&game)
            {
                let snapshot = runtime.bridge.snapshot();
                let layout = TimelineLayout::new(panel.bounds);
                let timeline = snapshot.timeline();
                if layout.bookmark_button().contains(world) {
                    self.bookmark_popup = Some(bookmarks::Popup::new(id, game));
                    self.sync_bookmarks(workspace);
                } else if snapshot.connected
                    && let Some(command) = layout
                        .hit_button(world)
                        .and_then(|control| dock_command(control, timeline))
                {
                    if command == TimelineCommand::ResumeLive
                        || command == TimelineCommand::TogglePlayPause
                            && timeline.play_resumes_live()
                    {
                        focus = Some(game);
                    }
                    runtime.bridge.send(EditorEvent::Timeline(command));
                } else if snapshot.connected && layout.hit_track(world) && timeline.tip.is_some() {
                    self.gesture = Some(Gesture::Scrub { timeline: id, game });
                    self.seek_cursor(workspace, id, game);
                    self.flush_seek(workspace, true);
                }
            }
            self.focus(workspace, focus, window_focused);
        }
    }

    fn send_game_press(
        &mut self,
        workspace: &Workspace,
        id: WindowId,
        snapshot: &Snapshot,
        display: Display,
        button: MouseButton,
    ) {
        if let Some(position) = self.game_point(workspace, id, snapshot, display) {
            let bridge = &workspace.games[&id].bridge;
            bridge.send(EditorEvent::Pointer(Some(position)));
            bridge.send(EditorEvent::Button(button, ElementState::Pressed));
            self.preview_buttons.push(button);
        }
    }

    fn start_explorer(
        &mut self,
        workspace: &Workspace,
        id: WindowId,
        mode: ExplorerMode,
        path: PathBuf,
    ) {
        self.explorers.insert(id, Explorer::new(mode));
        self.list_directory(workspace, id, path);
    }

    fn list_directory(&mut self, workspace: &Workspace, id: WindowId, path: PathBuf) {
        let Some(explorer) = self.explorers.get_mut(&id) else {
            return;
        };
        let request = explorer.start(&path);
        let mode = explorer.mode;
        let proxy = workspace.proxy.clone();
        thread::spawn(move || {
            let result = Directory::read(&path, mode).map_err(|err| err.to_string());
            let _ = proxy.send_event(UiEvent::ExplorerListed(id, request, result));
        });
    }

    fn directory_listed(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        request: u64,
        result: Result<Directory, String>,
    ) {
        if let Some(explorer) = self.explorers.get_mut(&id) {
            let path = result.as_ref().ok().map(|directory| directory.path.clone());
            if explorer.complete(request, result)
                && let Some(path) = path
            {
                workspace.board.set_explorer_directory(id, path);
            }
        }
    }

    fn close_explorer(&mut self, workspace: &mut Workspace, id: WindowId) {
        if self
            .explorers
            .get(&id)
            .is_some_and(|explorer| explorer.submitting)
        {
            return;
        }
        workspace.close(id);
        self.explorers.remove(&id);
        if self.active_explorer == Some(id) {
            self.active_explorer = None;
        }
    }

    fn explorer_action(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        display: Display,
        action: Option<ExplorerAction>,
    ) {
        match action {
            Some(ExplorerAction::List(path)) => self.list_directory(workspace, id, path),
            Some(ExplorerAction::Submit(path)) => {
                let Some(explorer) = self.explorers.get_mut(&id) else {
                    return;
                };
                let mode = explorer.mode;
                if mode.picks_asset() {
                    self.open_asset_file(workspace, id, mode, path);
                    return;
                }
                let proxy = workspace.proxy.clone();
                explorer.error = None;
                explorer.submitting = true;
                thread::spawn(move || {
                    let projects = match mode {
                        ExplorerMode::Create => {
                            project::scaffold(&path, engine_root()).map(|game| vec![game])
                        }
                        ExplorerMode::Open => project::discover(&path),
                        ExplorerMode::Browse | ExplorerMode::Atlas | ExplorerMode::Texture => {
                            return;
                        }
                    };
                    let _ = proxy.send_event(UiEvent::ProjectRead(
                        id,
                        projects.map_err(|err| err.to_string()),
                    ));
                });
            }
            Some(ExplorerAction::Choose(project)) => {
                self.attach_from_explorer(workspace, id, project, display)
            }
            None => {}
        }
    }

    fn open_asset_file(
        &mut self,
        workspace: &mut Workspace,
        picker: WindowId,
        mode: ExplorerMode,
        path: PathBuf,
    ) {
        let Some(owner) = workspace.board.window(picker).and_then(|panel| panel.owner) else {
            return;
        };
        let result = path.canonicalize().and_then(|path| {
            if mode.accepts_asset(&path) && path.is_file() {
                Ok(path)
            } else {
                Err(std::io::Error::other(if mode == ExplorerMode::Atlas {
                    "Choose an .atlas.json file"
                } else {
                    "Choose a PNG file"
                }))
            }
        });
        match result {
            Ok(path) => {
                self.last_asset_directory = path.parent().map(Path::to_path_buf);
                workspace.board.set_asset_source(owner, path);
                self.close_explorer(workspace, picker);
                self.sync_assets(workspace);
            }
            Err(err) => {
                if let Some(explorer) = self.explorers.get_mut(&picker) {
                    explorer.error = Some(err.to_string());
                }
            }
        }
    }

    fn attach_from_explorer(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        project: Project,
        display: Display,
    ) {
        let Some(owner) = workspace.board.window(id).and_then(|panel| panel.owner) else {
            return;
        };
        let directory = project.root().to_path_buf();
        match workspace.attach(owner, project, display.dpi) {
            Ok(()) => {
                workspace.board.note_project(directory);
                self.close_explorer(workspace, id);
            }
            Err(err) => {
                if let Some(explorer) = self.explorers.get_mut(&id) {
                    explorer.error = Some(err.to_string());
                }
            }
        }
    }

    fn project_read(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        projects: Result<Vec<Project>, String>,
        display: Display,
    ) {
        let Some(explorer) = self.explorers.get_mut(&id) else {
            return;
        };
        explorer.submitting = false;
        match projects {
            Ok(mut projects) if projects.len() == 1 => {
                self.attach_from_explorer(workspace, id, projects.pop().unwrap(), display);
            }
            Ok(projects) => explorer.targets(projects),
            Err(message) => explorer.error = Some(message),
        }
    }

    fn explorer_click(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        bounds: Bounds,
        display: Display,
        point: Point,
    ) {
        let Some(explorer) = self.explorers.get_mut(&id) else {
            return;
        };
        if explorer.busy() {
            return;
        }
        let layout = ExplorerLayout::new(bounds);
        let action = if !explorer.targets.is_empty() {
            if layout.up.contains(point) {
                explorer.back();
                None
            } else if let Some(row) = layout.row_at(point) {
                explorer.activate(explorer.current_offset(layout.visible()) + row)
            } else {
                None
            }
        } else if layout.up.contains(point) {
            explorer.parent()
        } else if layout.path.contains(point) {
            explorer.select_field(Field::Path);
            None
        } else if explorer.mode == ExplorerMode::Create && layout.name.contains(point) {
            explorer.select_field(Field::Name);
            None
        } else if layout.confirm.contains(point) {
            explorer.confirm()
        } else if let Some(row) = layout.row_at(point) {
            explorer.activate(explorer.current_offset(layout.visible()) + row)
        } else {
            explorer::shortcuts(workspace.board.recent_projects())
                .into_iter()
                .enumerate()
                .find(|(index, _)| layout.shortcut(*index).contains(point))
                .map(|(_, shortcut)| ExplorerAction::List(shortcut.path))
        };
        self.explorer_action(workspace, id, display, action);
    }

    fn asset_click(
        &mut self,
        workspace: &mut Workspace,
        display: Display,
        panel: &board::PanelWindow,
        point: Point,
    ) {
        let id = panel.id;
        let kind = panel.kind();
        let layout = art::Layout::new(panel.bounds);
        if layout.open.contains(point) {
            let directory = panel
                .asset_source()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .or_else(|| self.last_asset_directory.clone())
                .unwrap_or_else(|| {
                    engine_root()
                        .parent()
                        .unwrap_or(engine_root())
                        .to_path_buf()
                });
            let mode = if kind == Panel::TileAtlas {
                ExplorerMode::Atlas
            } else {
                ExplorerMode::Texture
            };
            if let Some(picker) = workspace.board.open_picker(id) {
                self.start_explorer(workspace, picker, mode, directory);
                self.active_explorer = Some(picker);
            }
            return;
        }
        let view = self.assets.panels.get_mut(&id).unwrap();
        if layout.action.contains(point) {
            if kind == Panel::TileAtlas {
                if let Some(atlas) = &view.atlas {
                    let texture = atlas.texture.clone();
                    let existing = workspace
                        .board
                        .windows()
                        .find(|window| {
                            window.kind() == Panel::TextureInspector
                                && window.asset_source() == Some(texture.as_path())
                        })
                        .map(|window| window.id);
                    let inspector = existing.unwrap_or_else(|| {
                        workspace
                            .board
                            .add(Panel::TextureInspector, viewport(display.logical()))
                    });
                    workspace.board.set_asset_source(inspector, texture);
                    workspace.board.raise(inspector);
                    self.sync_assets(workspace);
                }
            } else {
                view.camera = art::Camera::default();
            }
        } else if layout.image.contains(point) {
            if kind == Panel::TileAtlas {
                if let Some(sprite) = view.tile_at(layout.image, point) {
                    self.picked_tile = Some(sprite);
                    for editor in self.levels.values_mut() {
                        editor.select_brush(level::Brush::Paint);
                    }
                    let selected = self
                        .last_level
                        .filter(|id| self.levels.contains_key(id))
                        .or_else(|| {
                            (self.levels.len() == 1).then(|| *self.levels.keys().next().unwrap())
                        });
                    self.select_level(selected);
                    self.sync_assets(workspace);
                }
            } else if view.image.is_some() {
                self.gesture = Some(Gesture::PanAsset {
                    id,
                    button: MouseButton::Left,
                    origin: point,
                    pan: view.camera.pan,
                });
            }
        }
    }

    fn duplicate_panel(&mut self, workspace: &mut Workspace, id: WindowId, display: Display) {
        let Some(mut panel) = workspace.board.window(id).cloned() else {
            return;
        };
        if let Content::Explorer { directory } = &mut panel.content
            && let Some(open) = self
                .explorers
                .get(&id)
                .and_then(|explorer| explorer.directory.as_ref())
        {
            *directory = Some(open.path.clone());
        }
        let Some(game) = workspace.games.get(&id) else {
            self.finish_duplicate(workspace, panel, None, display);
            return;
        };
        if !self.duplicating.insert(id) {
            return;
        }
        let snapshot = game.bridge.snapshot();
        let stamp = workspace
            .presented
            .lock()
            .expect("presented game frames")
            .get(&id)
            .copied()
            .or(snapshot.frame.as_ref().and_then(|frame| frame.stamp));
        let connected = snapshot.connected;
        let has_preview = snapshot.frame.is_some();
        let frozen_state = snapshot.frozen_state;
        let project = game.project.clone();
        let bridge = game.bridge.clone();
        let source = game.timeline_directory.clone();
        let proxy = workspace.proxy.clone();
        thread::spawn(move || {
            let result = (|| -> std::io::Result<String> {
                let state = if let Some(seed) = frozen_state {
                    Some(seed)
                } else if connected && let Some(stamp) = stamp {
                    Some(std::sync::Arc::new(bridge.capture_state(stamp)?))
                } else if has_preview {
                    return Err(std::io::Error::other(
                        "the displayed game state is unavailable",
                    ));
                } else {
                    ForkState::from_closed(&source)?.map(std::sync::Arc::new)
                };
                let instance = project.reserve_instance()?;
                let directory = project.instance_root(&instance);
                let prepared = std::fs::create_dir(directory.join("runtime")).and_then(|()| {
                    state.map_or(Ok(()), |state| state.store(&directory.join("timeline")))
                });
                if let Err(err) = prepared {
                    let _ = std::fs::remove_dir_all(directory);
                    return Err(err);
                }
                Ok(instance)
            })();
            let _ = proxy.send_event(UiEvent::DuplicateReady(
                panel,
                result.map_err(|err| err.to_string()),
            ));
        });
    }

    fn finish_duplicate(
        &mut self,
        workspace: &mut Workspace,
        panel: board::PanelWindow,
        instance: Option<String>,
        display: Display,
    ) {
        self.duplicating.remove(&panel.id);
        let content = panel.content.clone();
        let id = workspace.board.duplicate(panel, instance);
        match content {
            Content::Game {
                project: Some(project),
                enabled,
                ..
            } => workspace.mount(id, project, enabled, display.dpi, None),
            Content::Explorer { directory } => self.start_explorer(
                workspace,
                id,
                ExplorerMode::Browse,
                directory.unwrap_or_else(explorer::games),
            ),
            _ => {}
        }
        self.sync_level_editor(workspace);
        self.sync_assets(workspace);
    }

    fn select_target_from_menu(
        &mut self,
        workspace: &mut Workspace,
        display: Display,
        point: Point,
    ) {
        let menu = self.target_menu(workspace, display);
        if let Some(dropdown) = self.target_menu.take()
            && let Some(target) = menu.and_then(|menu| menu.choice_at(point))
        {
            workspace.board.attach_target(dropdown.id, target);
        }
    }

    fn key(
        &mut self,
        workspace: &mut Workspace,
        display: Display,
        key: VirtualKeyCode,
        state: ElementState,
        window_focused: bool,
    ) {
        use VirtualKeyCode::{Escape, Key0, Tab};
        if let Some(id) = self.fullscreen_game() {
            if key == Escape && self.modifiers.shift() {
                if state == ElementState::Pressed {
                    self.leave_fullscreen(workspace, window_focused);
                }
            } else if let Some(game) = workspace.games.get(&id) {
                self.game_key(&game.bridge, key, state);
            }
            return;
        }
        if self.reset_prompt.is_some() {
            if key == Escape && state == ElementState::Pressed {
                self.reset_prompt = None;
            }
            return;
        }
        if self.bookmark_popup.is_some() {
            if state == ElementState::Pressed {
                match key {
                    Escape => self.bookmark_popup = None,
                    VirtualKeyCode::Return => self.save_bookmark(workspace),
                    VirtualKeyCode::Back => {
                        self.bookmark_popup.as_mut().unwrap().name.pop();
                    }
                    VirtualKeyCode::Up => self.scroll_bookmarks(workspace, display, -1),
                    VirtualKeyCode::Down => self.scroll_bookmarks(workspace, display, 1),
                    _ => {}
                }
            }
            return;
        }
        if self.header_menu.is_some() {
            if key == Escape && state == ElementState::Pressed {
                self.header_menu = None;
            }
            return;
        }
        if self.target_menu.is_some() {
            if state == ElementState::Pressed {
                match key {
                    Escape => self.target_menu = None,
                    VirtualKeyCode::Up => self.scroll_targets(workspace, display, 1.0),
                    VirtualKeyCode::Down => self.scroll_targets(workspace, display, -1.0),
                    _ => {}
                }
            }
            return;
        }
        if self.gesture.is_some_and(|gesture| {
            matches!(gesture, Gesture::Target { .. }) || gesture.level().is_some()
        }) {
            if key == Escape && state == ElementState::Pressed {
                self.end_gesture(workspace, display, false);
            }
            return;
        }
        if self.menu_open && key == Escape {
            if state == ElementState::Pressed {
                self.menu_open = false;
            }
            return;
        }
        if key == Key0 && (self.modifiers.logo() || self.modifiers.ctrl()) {
            if state == ElementState::Pressed {
                self.focus(workspace, None, window_focused);
                let levels = workspace
                    .board
                    .windows()
                    .filter_map(|panel| {
                        let editor = self.levels.get(&panel.id)?;
                        editor.size()?;
                        Some(editor.layout(panel).grid)
                    })
                    .collect::<Vec<_>>();
                workspace.board.fit(viewport(display.logical()), levels);
            }
            return;
        }
        if let Some(id) = self.active_explorer {
            if state == ElementState::Pressed {
                self.explorer_key(workspace, id, display, key);
            }
            return;
        }
        if key == Tab {
            if state == ElementState::Pressed {
                let mut ids = workspace
                    .games
                    .iter()
                    .filter(|(_, game)| game.bridge.snapshot().connected)
                    .map(|(&id, _)| id);
                let next = match self.focused {
                    Some(current) => ids.find(|&id| id > current),
                    None => ids.next(),
                };
                self.focus(workspace, next, window_focused);
                if let Some(next) = next {
                    workspace.board.raise(next);
                } else {
                    let timeline = workspace
                        .board
                        .windows()
                        .rev()
                        .find(|window| window.kind() == Panel::Timeline)
                        .map(|window| window.id);
                    if let Some(timeline) = timeline {
                        workspace.board.raise(timeline);
                    }
                }
            }
            return;
        }
        if let Some(game) = self.focused.and_then(|id| workspace.games.get(&id)) {
            self.game_key(&game.bridge, key, state);
        }
    }

    fn explorer_key(
        &mut self,
        workspace: &mut Workspace,
        id: WindowId,
        display: Display,
        key: VirtualKeyCode,
    ) {
        use VirtualKeyCode::{A, Back, Down, Escape, Return, Tab, Up};
        let Some(panel) = workspace.board.window(id) else {
            return;
        };
        let Some(explorer) = self.explorers.get_mut(&id) else {
            return;
        };
        if key == Escape {
            if panel.owner.is_some() {
                self.close_explorer(workspace, id);
            } else {
                self.active_explorer = None;
            }
            return;
        }
        if explorer.busy() {
            return;
        }
        let layout = ExplorerLayout::new(panel.bounds);
        let command = self.modifiers.logo() || self.modifiers.ctrl();
        let action = match key {
            Return => explorer.enter(),
            Back if explorer.field.is_some() => {
                explorer.backspace();
                None
            }
            Back if !explorer.targets.is_empty() => {
                explorer.back();
                None
            }
            Back => explorer.parent(),
            Tab if explorer.targets.is_empty() => {
                let next = match (explorer.mode, explorer.field) {
                    (ExplorerMode::Create, Some(Field::Path)) => Field::Name,
                    _ => Field::Path,
                };
                explorer.select_field(next);
                None
            }
            Up | Down => {
                explorer.field = None;
                explorer.select_step(if key == Up { -1 } else { 1 }, layout.visible());
                None
            }
            A if command => {
                explorer.mark_all();
                None
            }
            #[cfg(target_os = "macos")]
            VirtualKeyCode::V if command => {
                if let Some(text) = mac_clipboard::read() {
                    explorer.input(text.trim());
                }
                None
            }
            _ => None,
        };
        self.explorer_action(workspace, id, display, action);
    }

    fn game_key(&mut self, bridge: &Bridge, key: VirtualKeyCode, state: ElementState) {
        use VirtualKeyCode::{F5, F6, F7, F8, Left, P, Right, Space};
        match state {
            ElementState::Pressed => {
                let Entry::Vacant(held) = self.preview_keys.entry(key) else {
                    return;
                };
                let command = match (key, self.modifiers.alt()) {
                    (F5, _) | (P, true) => Some(TimelineCommand::TogglePause),
                    (F6, _) | (Left, true) => Some(TimelineCommand::Rewind),
                    (F7, _) | (Right, true) => Some(TimelineCommand::Forward),
                    (F8, _) | (Space, true) => Some(TimelineCommand::TogglePlayback),
                    _ => None,
                };
                if let Some(command) = command {
                    held.insert(GameKeyRoute::Timeline);
                    bridge.send(EditorEvent::Timeline(command));
                } else {
                    held.insert(GameKeyRoute::Gameplay);
                    bridge.send(EditorEvent::Key(key, state));
                }
            }
            ElementState::Released => {
                if matches!(self.preview_keys.remove(&key), Some(GameKeyRoute::Gameplay)) {
                    bridge.send(EditorEvent::Key(key, state));
                }
            }
        }
    }

    fn scroll_targets(&mut self, workspace: &Workspace, display: Display, amount: f32) {
        if amount != 0.0
            && let Some(menu) = self.target_menu(workspace, display)
            && let Some(dropdown) = self.target_menu.as_mut()
        {
            let step = if amount > 0.0 { -1 } else { 1 };
            dropdown.offset = menu
                .offset
                .saturating_add_signed(step)
                .min(menu.max_offset());
        }
    }

    fn wheel(&mut self, workspace: &mut Workspace, display: Display, delta: MouseScrollDelta) {
        if self.gesture.is_some() || self.reset_prompt.is_some() {
            return;
        }
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x, y),
            MouseScrollDelta::PixelDelta(value) => {
                let logical = value.to_logical::<f32>(display.dpi);
                (logical.x / 100.0, logical.y / 100.0)
            }
        };
        if let Some(id) = self.fullscreen_game() {
            if let Some(game) = workspace.games.get(&id)
                && self
                    .game_point(workspace, id, &game.bridge.snapshot(), display)
                    .is_some()
            {
                game.bridge.send(EditorEvent::Scroll(x, y));
            }
            return;
        }
        if self.bookmark_popup.is_some() {
            if y != 0.0 {
                self.scroll_bookmarks(workspace, display, if y > 0.0 { -1 } else { 1 });
            }
            return;
        }
        if self.target_menu.is_some() {
            self.scroll_targets(workspace, display, y);
            return;
        }
        if self.menu_open || self.header_menu.is_some() {
            return;
        }
        if !self.modifiers.logo()
            && !self.modifiers.ctrl()
            && let Some(point) = self.cursor.map(|point| workspace.board.world_at(point))
            && let Some(window) = workspace
                .board
                .panel_at(point)
                .and_then(|id| workspace.board.window(id))
            && workspace.board.picker(window.id).is_none()
            && let Some(view) = self.assets.panels.get_mut(&window.id)
        {
            let layout = art::Layout::new(window.bounds);
            if layout.image.contains(point) && view.image.is_some() {
                view.camera.zoom_at(layout.image, point, y);
            }
            return;
        }
        if !self.modifiers.logo()
            && !self.modifiers.ctrl()
            && let Some(point) = self.cursor
            && let Some(panel) = workspace
                .board
                .panel_at(workspace.board.world_at(point))
                .and_then(|id| workspace.board.window(id))
            && let Some(explorer) = self.explorers.get_mut(&panel.id)
        {
            let amount = match delta {
                MouseScrollDelta::LineDelta(_, y) => y,
                MouseScrollDelta::PixelDelta(value) => {
                    value.to_logical::<f32>(display.dpi).y / 25.0
                }
            };
            explorer.scroll(amount, ExplorerLayout::new(panel.bounds).visible());
            return;
        }
        if !self.modifiers.logo()
            && !self.modifiers.ctrl()
            && let Some(id) = self.focused
            && let Some(game) = workspace.games.get(&id)
            && self
                .game_point(workspace, id, &game.bridge.snapshot(), display)
                .is_some()
        {
            game.bridge.send(EditorEvent::Scroll(x, y));
        } else if let Some(point) = self.cursor {
            workspace
                .board
                .zoom_at(point, (f64::from(y) * 0.16).clamp(-0.9, 0.9).exp());
        }
    }

    fn preview_changed(&mut self, workspace: &mut Workspace, id: WindowId, window_focused: bool) {
        let Some(game) = workspace.games.get_mut(&id) else {
            return;
        };
        game.bridge.acknowledge_redraw();
        let snapshot = game.bridge.snapshot();
        match snapshot.phase {
            Phase::Disabled => workspace.board.set_enabled(id, false),
            Phase::Enabling => workspace.board.set_enabled(id, true),
            _ if game.bridge.lifecycle() == Lifecycle::Enabled => {
                workspace.board.set_enabled(id, true)
            }
            _ => {}
        }
        if self.fullscreen_game() == Some(id)
            && matches!(
                snapshot.phase,
                Phase::Exited | Phase::Disabled | Phase::Failed | Phase::GpuFailed
            )
        {
            self.leave_fullscreen(workspace, window_focused);
            return;
        }
        let generation = snapshot.generation.filter(|_| snapshot.connected);
        if game.generation == generation {
            return;
        }
        game.generation = generation;
        game.pending_volume = None;
        if self.focused == Some(id) {
            self.preview_buttons.clear();
            self.preview_keys.clear();
        }
        if self.pending_seek.is_some_and(|(target, _)| target == id) {
            self.pending_seek = None;
        }
        if matches!(self.gesture, Some(Gesture::Volume(target) | Gesture::Scrub { game: target, .. }) if target == id)
        {
            self.gesture = None;
            self.drag_ms = None;
        }
        if generation.is_some() {
            game.bridge.send(EditorEvent::Focus(
                window_focused && self.focused == Some(id),
            ));
        }
    }

    fn cursor_icon(&self, workspace: &Workspace, display: Display) -> CursorIcon {
        if self.fullscreen.is_some() {
            return if self.fullscreen_exit_visible(display, Instant::now())
                && self
                    .cursor
                    .is_some_and(|point| fullscreen_exit_button(display.logical()).contains(point))
            {
                CursorIcon::Hand
            } else {
                CursorIcon::Default
            };
        }
        let edges = match self.gesture {
            Some(Gesture::Resize { edges, .. } | Gesture::ResizeLevel { edges, .. }) => Some(edges),
            Some(Gesture::PaintLevel(_)) => return CursorIcon::Crosshair,
            Some(Gesture::PanAsset { .. }) => return CursorIcon::Grabbing,
            Some(_) => return CursorIcon::Grabbing,
            None => {
                let Some(point) = self.cursor else {
                    return CursorIcon::Default;
                };
                if let Some(layout) = self.reset_layout(&workspace.board, display) {
                    return if layout.confirm.contains(point) || layout.cancel.contains(point) {
                        CursorIcon::Hand
                    } else {
                        CursorIcon::Default
                    };
                }
                if let Some(menu) = self.header_menu {
                    return if !menu.pending && menu.bounds.contains(point) {
                        CursorIcon::Hand
                    } else {
                        CursorIcon::Default
                    };
                }
                if self.target_menu.is_some() {
                    return CursorIcon::Default;
                }
                if PanelMenu::new(display.logical())
                    .covers(self.menu_open)
                    .any(|bounds| bounds.contains(point))
                {
                    return CursorIcon::Hand;
                }
                let world = workspace.board.world_at(point);
                let Some(panel) = self
                    .panel_at(&workspace.board, world)
                    .and_then(|id| workspace.board.window(id))
                else {
                    return CursorIcon::Grab;
                };
                let has_picker = workspace.board.picker(panel.id).is_some();
                if (panel.owner.is_some() || has_picker) && panel.stack_drag_at(world) {
                    return CursorIcon::Grab;
                }
                if has_picker {
                    return CursorIcon::Hand;
                }
                if panel.kind() == Panel::Explorer {
                    let layout = ExplorerLayout::new(panel.bounds);
                    if layout.path.contains(world)
                        || layout.name.contains(world)
                            && self
                                .explorers
                                .get(&panel.id)
                                .is_some_and(|explorer| explorer.mode == ExplorerMode::Create)
                    {
                        return CursorIcon::Text;
                    }
                    if panel.close_button().contains(world)
                        || layout.up.contains(world)
                        || layout.list.contains(world)
                        || layout.sidebar.contains(world)
                        || layout.confirm.contains(world)
                    {
                        return CursorIcon::Hand;
                    }
                    if panel.owner.is_some() {
                        return CursorIcon::Default;
                    }
                }
                if panel.close_button().contains(world)
                    || panel
                        .target_button()
                        .is_some_and(|button| button.contains(world))
                    || panel.kind() == Panel::Timeline
                        && TimelineLayout::new(panel.bounds)
                            .reset_button()
                            .contains(world)
                        && panel.attached_game().is_some()
                {
                    return CursorIcon::Hand;
                }
                if panel.kind() == Panel::Game {
                    if let Some(game) = workspace.games.get(&panel.id) {
                        let header = GameHeader::new(panel.bounds);
                        if header.lifecycle_button.contains(world)
                            || header.hit_zoom(world)
                            || (header.fullscreen_button.contains(world)
                                && game.bridge.snapshot().connected)
                            || (header.hit_volume(world)
                                && game.volume(&game.bridge.snapshot()).is_some())
                        {
                            return CursorIcon::Hand;
                        }
                    } else {
                        let (new, open) = panel.project_buttons();
                        if new.contains(world) || open.contains(world) {
                            return CursorIcon::Hand;
                        }
                    }
                }
                if let Some(editor) = self.levels.get(&panel.id) {
                    let layout = editor.layout(panel);
                    if editor.size().is_some()
                        && (layout.paint.contains(world) || layout.erase.contains(world))
                    {
                        return CursorIcon::Hand;
                    }
                    if layout.toolbar.contains(world) {
                        return CursorIcon::Grab;
                    }
                    if editor.selected() {
                        if let Some(edges) = layout.resize_edges(world, workspace.board.zoom) {
                            return resize_cursor(edges);
                        }
                        return CursorIcon::Crosshair;
                    }
                    return CursorIcon::Hand;
                }
                if panel.kind() == Panel::Profiler {
                    let layout = ProfileLayout::new(panel.bounds);
                    if layout.live.contains(world) || layout.graph.contains(world) {
                        return CursorIcon::Hand;
                    }
                }
                let outer = workspace
                    .board
                    .dock_bounds(panel.id)
                    .unwrap_or(panel.bounds);
                let docked_to_blocked_game = workspace
                    .board
                    .dock_group(panel.id)
                    .is_some_and(|(game, _)| workspace.board.picker(game).is_some());
                let edges = (!docked_to_blocked_game)
                    .then(|| Edges::within(outer, workspace.board.zoom, world))
                    .flatten();
                if edges.is_none() && world.y < panel.bounds.y + TITLE_HEIGHT {
                    return CursorIcon::Grab;
                }
                edges
            }
        };
        edges.map_or(CursorIcon::Default, resize_cursor)
    }

    fn hovered_transport(&self, board: &Board) -> Option<(WindowId, Transport)> {
        if self.target_menu.is_some()
            || self.bookmark_popup.is_some()
            || self.reset_prompt.is_some()
            || self.menu_open
        {
            return None;
        }
        let world = board.world_at(self.cursor?);
        let panel = board.window(board.panel_at(world)?)?;
        if panel.kind() != Panel::Timeline {
            return None;
        }
        TimelineLayout::new(panel.bounds)
            .hit_button(world)
            .map(|control| (panel.id, control))
    }

    fn frame(
        &mut self,
        workspace: &Workspace,
        display: Display,
    ) -> (FrameRequest, Option<Instant>) {
        let mut games = BTreeMap::new();
        let mut previews = BTreeMap::new();
        let mut redraw_at = None;
        let now = Instant::now();
        let fullscreen = self.fullscreen.as_ref().map(|mode| {
            if mode.hint_until > now {
                redraw_at = Some(mode.hint_until);
            }
            FullscreenChrome {
                game: mode.id,
                show_exit: self.fullscreen_exit_visible(display, now),
            }
        });
        for (&id, game) in &workspace.games {
            if fullscreen.is_some_and(|mode| mode.game != id) {
                continue;
            }
            let snapshot = game.bridge.snapshot();
            let progress = snapshot.frozen_since.map_or(0.0, |start| {
                (now.saturating_duration_since(start).as_secs_f32() / BLUR_DURATION.as_secs_f32())
                    .min(1.0)
            });
            if snapshot.frozen_since.is_some() && progress < 1.0 {
                redraw_at = redraw_at
                    .into_iter()
                    .chain(Some(now + ANIMATION_INTERVAL))
                    .min();
            }
            games.insert(
                id,
                GameChrome {
                    phase: snapshot.phase,
                    connected: snapshot.connected,
                    frame_size: snapshot.frame.as_ref().map(|frame| frame.size),
                    master_volume: game.volume(&snapshot),
                    timeline: snapshot.timeline(),
                    error: snapshot.error,
                },
            );
            if let Some(frame) = snapshot.frame {
                previews.insert(
                    id,
                    Preview {
                        frame,
                        blur: 1.0 - (1.0 - progress).powi(3),
                        arrival: snapshot.arrival,
                    },
                );
            }
        }
        let hovered = self.hovered_transport(&workspace.board);
        let target_drag = match (self.gesture, self.cursor) {
            (
                Some(Gesture::Target {
                    id,
                    since: Some(start),
                    ..
                }),
                Some(pointer),
            ) => {
                let hovered = self.target_at(&workspace.board, display);
                if hovered.is_some() {
                    redraw_at = redraw_at
                        .into_iter()
                        .chain(Some(now + ANIMATION_INTERVAL))
                        .min();
                }
                Some(target::Drag {
                    source: id,
                    pointer,
                    hovered,
                    phase: ((now.saturating_duration_since(start).as_millis() / 24) % 36) as u8,
                })
            }
            _ => None,
        };
        let chrome = Chrome {
            board: workspace.board.clone(),
            size: display.logical(),
            games,
            focus: self.focused,
            hovered,
            drag_ms: self.drag_ms,
            dock_target: self.dock_target(&workspace.board),
            menu_open: self.menu_open,
            target_menu: self.target_menu(workspace, display),
            header_menu: self.header_menu,
            notice: self
                .notice
                .as_ref()
                .filter(|(_, expires)| *expires > now)
                .map(|(message, expires)| {
                    redraw_at = Some(redraw_at.map_or(*expires, |at| at.min(*expires)));
                    message.clone()
                }),
            bookmark_popup: self
                .bookmark_layout(workspace, display)
                .zip(self.bookmark_popup.clone()),
            reset_prompt: self.reset_layout(&workspace.board, display),
            levels: self
                .levels
                .iter()
                .map(|(&id, editor)| (id, editor.view(self.picked_tile.as_ref())))
                .collect(),
            picked_tile: self.picked_tile.clone(),
            level_tilesets: self.assets.level_tilesets.clone(),
            assets: self.assets.panels.clone(),
            explorers: self.explorers.clone(),
            active_explorer: self.active_explorer,
            fullscreen,
        };
        (
            FrameRequest {
                chrome,
                target_drag,
                surface: display.surface,
                previews,
                profile_clicks: std::mem::take(&mut self.profile_clicks),
            },
            redraw_at,
        )
    }
}

#[cfg(target_os = "macos")]
mod mac_clipboard {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::{CStr, c_char};

    unsafe fn string(value: *mut Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let bytes: *const c_char = unsafe { msg_send![value, UTF8String] };
        if bytes.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(bytes) }
            .to_str()
            .ok()
            .map(str::to_owned)
    }

    pub fn read() -> Option<String> {
        unsafe {
            let pasteboard: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            let kind: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
            let text: *mut Object = msg_send![pasteboard, stringForType: kind];
            string(text)
        }
    }
}

fn resize_cursor(edges: Edges) -> CursorIcon {
    match edges {
        Edges { horizontal: 0, .. } => CursorIcon::NsResize,
        Edges { vertical: 0, .. } => CursorIcon::EwResize,
        Edges {
            horizontal,
            vertical,
        } if horizontal == vertical => CursorIcon::NwseResize,
        _ => CursorIcon::NeswResize,
    }
}

fn dock_command(control: Transport, timeline: TimelineStatus) -> Option<TimelineCommand> {
    if timeline.detached_bookmark.is_some() {
        return match control {
            Transport::Back => Some(TimelineCommand::ReturnFromBookmark),
            Transport::TakeOver => Some(TimelineCommand::ResumeLive),
            Transport::Play | Transport::Next => None,
        };
    }
    match control {
        Transport::Back if timeline.cursor.is_some_and(|frame| frame.id > 0) => {
            Some(TimelineCommand::Rewind)
        }
        Transport::Play if timeline.tip.is_some() => Some(TimelineCommand::TogglePlayPause),
        Transport::Next if timeline.cursor != timeline.tip => Some(TimelineCommand::Forward),
        Transport::TakeOver
            if timeline.paused
                && !timeline.playing
                && timeline
                    .cursor
                    .zip(timeline.tip)
                    .is_some_and(|(cursor, tip)| cursor.id < tip.id) =>
        {
            Some(TimelineCommand::ResumeLive)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Display {
    surface: SurfaceSize,
    dpi: f64,
}

#[derive(Default)]
struct FullscreenWindow {
    restore_on_exit: bool,
    observed: bool,
}

impl FullscreenWindow {
    fn set_playtest(&mut self, window: &Window, active: bool) {
        if active {
            self.restore_on_exit = window.fullscreen().is_none();
            if self.restore_on_exit {
                window.set_fullscreen(Some(Fullscreen::Borderless(None)));
            }
            self.observed = window.fullscreen().is_some();
        } else {
            if self.restore_on_exit {
                window.set_fullscreen(None);
            }
            *self = Self::default();
        }
    }

    fn exited(&mut self, window: &Window) -> bool {
        let fullscreen = window.fullscreen().is_some();
        self.observed |= fullscreen;
        self.observed && !fullscreen
    }
}

impl Display {
    fn logical(self) -> SurfaceSize {
        self.surface.to_logical(self.dpi)
    }

    fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if size.width != 0 && size.height != 0 {
            self.surface = SurfaceSize::new(size.width, size.height);
        }
    }
}

fn save_board(path: &Path, board: &Board, saved: &mut Option<Board>) {
    let board = board.persisted();
    if saved.as_ref() != Some(&board) {
        match board.save(path) {
            Ok(()) => *saved = Some(board),
            Err(err) => eprintln!("could not save the editor board: {err}"),
        }
    }
}

pub fn run(release: bool, timeline: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let mut events = EventLoopBuilder::<UiEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("Sycho Editor")
        .with_inner_size(LogicalSize::new(1120.0, 850.0))
        .with_maximized(true)
        .with_min_inner_size(LogicalSize::new(535.0, 390.0))
        .build(&events)?;
    let size = window.inner_size();
    let mut display = Display {
        surface: SurfaceSize::new(size.width, size.height),
        dpi: window.scale_factor(),
    };
    #[cfg(target_os = "macos")]
    let use_shared_surfaces = std::env::var("SYCHO_PREVIEW_TRANSPORT").as_deref() != Ok("rgba");
    let build_pixels = |timestamps| {
        let builder =
            PixelsBuilder::new(1, 1, SurfaceTexture::new(size.width, size.height, &window));
        #[cfg(target_os = "macos")]
        let builder = if use_shared_surfaces {
            builder.request_adapter_options(wgpu::RequestAdapterOptions {
                power_preference: wgpu::util::power_preference_from_env()
                    .unwrap_or(wgpu::PowerPreference::HighPerformance),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
        } else {
            builder
        };
        let builder = if timestamps {
            let mut limits = wgpu::Limits::default();
            limits.max_texture_dimension_2d = limits
                .max_texture_dimension_2d
                .max(size.width)
                .max(size.height);
            builder.device_descriptor(wgpu::DeviceDescriptor {
                label: Some("editor profiler device"),
                features: wgpu::Features::TIMESTAMP_QUERY,
                limits,
            })
        } else {
            builder
        };
        builder.build()
    };
    // Winit creates the native window and its surface on the window thread.
    let pixels = build_pixels(true).or_else(|_| build_pixels(false))?;
    #[cfg(target_os = "macos")]
    let gpu_device = use_shared_surfaces
        .then(|| shared_surface::device_id(&pixels.context().device))
        .flatten();
    let render = RenderWorker::start(pixels, display.surface, events.create_proxy())?;
    let path = Path::new(BOARD_PATH);
    let mut example = tetree();
    if let Ok(manifest) = example.manifest.canonicalize() {
        example.manifest = manifest;
    }
    let legacy_enabled = std::fs::read(".sycho-games.json")
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|state| {
            state
                .get("disabled")
                .and_then(|ids| ids.as_array())
                .map(|ids| !ids.iter().any(|id| id.as_str() == Some("game/headful")))
        })
        .unwrap_or(true);
    let loaded = Board::load(path, &example, legacy_enabled)?;
    let mut saved = loaded
        .as_ref()
        .filter(|(_, legacy)| !legacy)
        .map(|(board, _)| board.clone());
    let board = loaded.map(|(board, _)| board).unwrap_or_else(|| {
        if example.manifest.is_file() {
            Board::with_example(example.clone(), legacy_enabled)
        } else {
            Board::default()
        }
    });
    let initial: Vec<_> = board
        .windows()
        .filter_map(|panel| match &panel.content {
            Content::Game {
                project: Some(project),
                enabled,
                ..
            } => Some((panel.id, project.clone(), *enabled)),
            _ => None,
        })
        .collect();
    let mut workspace = Workspace {
        board,
        games: BTreeMap::new(),
        retired: Vec::new(),
        proxy: events.create_proxy(),
        presented: render.presented.clone(),
        release,
        #[cfg(target_os = "macos")]
        gpu_device,
    };
    for (id, project, enabled) in initial {
        let timeline = (project == example).then(|| timeline.clone()).flatten();
        workspace.mount(id, project, enabled, display.dpi, timeline);
    }
    let mut save_at = None;
    let mut redraw_at = None;
    let mut input = Interaction::default();
    for panel in workspace.board.windows() {
        if let Content::Explorer { directory } = &panel.content {
            input.start_explorer(
                &workspace,
                panel.id,
                ExplorerMode::Browse,
                directory.clone().unwrap_or_else(explorer::games),
            );
        }
    }
    let mut window_focused = true;
    let mut occluded = false;
    let mut closing = false;
    let mut fullscreen_window = FullscreenWindow::default();
    let proxy = workspace.proxy.clone();
    input.assets.watch = AssetWatch::with_notify(move || {
        let _ = proxy.send_event(UiEvent::AssetsChanged);
    });
    input.sync_profiling(&workspace);
    input.sync_assets(&workspace);
    window.request_redraw();
    events.run_return(|event, _, control| {
        if closing {
            *control = if matches!(event, Event::UserEvent(UiEvent::RenderStopped)) {
                ControlFlow::Exit
            } else {
                ControlFlow::Wait
            };
            return;
        }
        *control = input.wait(save_at, redraw_at);
        let before_fullscreen = input.fullscreen_game();
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                closing = true;
                render.close();
            }
            Event::UserEvent(UiEvent::RenderStopped) => *control = ControlFlow::Exit,
            Event::WindowEvent { event, .. } => {
                let previous = workspace.board.clone();
                let previous_display = display;
                let exit_visible = input.fullscreen_exit_visible(display, Instant::now());
                let hovered = input.hovered_transport(&workspace.board);
                let pointer_only = matches!(
                    event,
                    WindowEvent::CursorMoved { .. } | WindowEvent::CursorLeft { .. }
                );
                match event {
                    WindowEvent::Resized(size) => display.resize(size),
                    WindowEvent::ScaleFactorChanged {
                        scale_factor,
                        new_inner_size,
                    } => {
                        display.dpi = scale_factor;
                        display.resize(*new_inner_size);
                    }
                    WindowEvent::Occluded(hidden) => occluded = hidden,
                    WindowEvent::CursorMoved { position, .. } => {
                        let position = position.to_logical::<f64>(display.dpi);
                        input.pointer(
                            &mut workspace,
                            Some(Point::new(position.x, position.y)),
                            display,
                        );
                    }
                    WindowEvent::CursorLeft { .. }
                        if input.gesture.is_none()
                            || matches!(
                                input.gesture,
                                Some(
                                    Gesture::Target { .. }
                                        | Gesture::PanAsset { .. }
                                        | Gesture::PaintLevel(_)
                                        | Gesture::ResizeLevel { .. }
                                )
                            ) =>
                    {
                        input.pointer(&mut workspace, None, display)
                    }
                    WindowEvent::MouseInput { button, state, .. } => {
                        input.mouse(&mut workspace, display, button, state, window_focused)
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        input.wheel(&mut workspace, display, delta)
                    }
                    WindowEvent::TouchpadMagnify { delta, .. }
                        if input.fullscreen.is_none()
                            && input.target_menu.is_none()
                            && input.bookmark_popup.is_none()
                            && input.reset_prompt.is_none()
                            && !input.menu_open =>
                    {
                        if input.gesture.is_none()
                            && let Some(point) = input.cursor
                        {
                            workspace.board.zoom_at(point, delta.clamp(-0.9, 0.9).exp());
                        }
                    }
                    WindowEvent::ModifiersChanged(modifiers) => input.modifiers = modifiers,
                    WindowEvent::ReceivedCharacter(ch) => {
                        if input.fullscreen.is_none()
                            && !input.modifiers.logo()
                            && !input.modifiers.ctrl()
                            && !ch.is_control()
                        {
                            if let Some(popup) = input.bookmark_popup.as_mut() {
                                if popup.can_save && popup.name.chars().count() < 80 {
                                    popup.name.push(ch);
                                }
                            } else if let Some(explorer) = input
                                .active_explorer
                                .and_then(|id| input.explorers.get_mut(&id))
                            {
                                explorer.input(ch.encode_utf8(&mut [0; 4]));
                            }
                        }
                    }
                    WindowEvent::KeyboardInput { input: key, .. } => {
                        if let Some(code) = key.virtual_keycode {
                            input.key(&mut workspace, display, code, key.state, window_focused);
                        }
                    }
                    WindowEvent::Focused(focused) => {
                        window_focused = focused;
                        if let Some(game) = input.focused.and_then(|id| workspace.games.get(&id)) {
                            game.bridge.send(EditorEvent::Focus(focused));
                        }
                        if !focused {
                            input.reset_prompt = None;
                            input.end_gesture(&mut workspace, display, false);
                            input.preview_buttons.clear();
                            input.preview_keys.clear();
                            input.modifiers = ModifiersState::empty();
                        }
                    }
                    _ => return,
                }
                input.sync_profiling(&workspace);
                let board_changed = previous != workspace.board;
                if board_changed {
                    input.sync_assets(&workspace);
                    input.sync_level_editor(&workspace);
                    save_at = Some(Instant::now() + SAVE_DELAY);
                }
                if board_changed || previous_display != display {
                    workspace.sync_sizes(display, input.fullscreen_game());
                }
                window.set_cursor_icon(input.cursor_icon(&workspace, display));
                if !occluded
                    && (!pointer_only
                        || board_changed
                        || exit_visible != input.fullscreen_exit_visible(display, Instant::now())
                        || hovered != input.hovered_transport(&workspace.board)
                        || matches!(
                            input.gesture,
                            Some(
                                Gesture::Scrub { .. }
                                    | Gesture::Volume(_)
                                    | Gesture::Target { .. }
                                    | Gesture::PanAsset { .. }
                                    | Gesture::PaintLevel(_)
                                    | Gesture::ResizeLevel { .. }
                            )
                        ))
                {
                    window.request_redraw();
                }
            }
            Event::UserEvent(UiEvent::PreviewChanged(id)) => {
                let previous = workspace
                    .board
                    .window(id)
                    .map(|panel| panel.content.clone());
                input.preview_changed(&mut workspace, id, window_focused);
                input.sync_level_editor(&workspace);
                input.sync_bookmarks(&workspace);
                if previous.as_ref() != workspace.board.window(id).map(|panel| &panel.content) {
                    save_at = Some(Instant::now() + SAVE_DELAY);
                }
                window.request_redraw();
            }
            Event::UserEvent(UiEvent::DuplicateReady(panel, result)) => {
                input.duplicating.remove(&panel.id);
                if input
                    .header_menu
                    .is_some_and(|menu| menu.source == panel.id)
                {
                    input.header_menu = None;
                }
                match result {
                    Ok(instance) => {
                        input.finish_duplicate(&mut workspace, panel, Some(instance), display);
                        save_at = Some(Instant::now());
                    }
                    Err(err) => {
                        eprintln!("could not duplicate game: {err}");
                        input.notice = Some((
                            format!("Could not duplicate game. {err}"),
                            Instant::now() + Duration::from_secs(5),
                        ));
                    }
                }
                input.sync_profiling(&workspace);
                window.request_redraw();
            }
            Event::UserEvent(UiEvent::AssetsChanged) => {
                if input.assets.poll() {
                    window.request_redraw();
                }
            }
            Event::UserEvent(UiEvent::ExplorerListed(id, request, result)) => {
                input.directory_listed(&mut workspace, id, request, result);
                save_at = Some(Instant::now() + SAVE_DELAY);
                window.request_redraw();
            }
            Event::UserEvent(UiEvent::ProjectRead(id, projects)) => {
                input.project_read(&mut workspace, id, projects, display);
                input.sync_assets(&workspace);
                input.sync_level_editor(&workspace);
                input.sync_profiling(&workspace);
                save_at = Some(Instant::now() + SAVE_DELAY);
                window.request_redraw();
            }
            Event::MainEventsCleared => {
                if input.fullscreen.is_some() && fullscreen_window.exited(&window) {
                    input.leave_fullscreen(&workspace, window_focused);
                }
                input.flush_seek(&workspace, false);
                workspace.cleanup();
                if save_at.is_some_and(|at| Instant::now() >= at) {
                    save_board(path, &workspace.board, &mut saved);
                    save_at = None;
                }
                if redraw_at.is_some_and(|at| Instant::now() >= at) {
                    redraw_at = None;
                    window.request_redraw();
                }
                *control = input.wait(save_at, redraw_at);
            }
            Event::RedrawRequested(_) if !occluded => {
                let (frame, next) = input.frame(&workspace, display);
                redraw_at = next;
                render.submit(frame);
            }
            _ => {}
        }
        if before_fullscreen != input.fullscreen_game() {
            fullscreen_window.set_playtest(&window, input.fullscreen.is_some());
            workspace.sync_sizes(display, input.fullscreen_game());
            window.request_redraw();
        }
    });
    save_board(path, &workspace.board, &mut saved);
    workspace.finish();
    render.finish().map_err(|err| -> Box<dyn Error> { err })
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::timeline::Frame;

    #[test]
    fn game_shortcuts_route_to_one_bridge_without_leaking_into_gameplay() {
        let (first, first_events) = Bridge::with_test_input();
        let (second, second_events) = Bridge::with_test_input();
        let mut input = Interaction {
            modifiers: ModifiersState::ALT,
            ..Interaction::default()
        };
        input.game_key(&first, VirtualKeyCode::P, ElementState::Pressed);
        input.game_key(&first, VirtualKeyCode::P, ElementState::Pressed);
        input.game_key(&first, VirtualKeyCode::P, ElementState::Released);
        assert!(matches!(
            first_events.try_recv(),
            Ok(EditorEvent::Timeline(TimelineCommand::TogglePause))
        ));
        assert!(first_events.try_recv().is_err());
        assert!(second_events.try_recv().is_err());
        input.modifiers = ModifiersState::empty();
        input.game_key(&second, VirtualKeyCode::A, ElementState::Pressed);
        input.game_key(&second, VirtualKeyCode::A, ElementState::Released);
        assert!(matches!(
            second_events.try_recv(),
            Ok(EditorEvent::Key(VirtualKeyCode::A, ElementState::Pressed))
        ));
        assert!(matches!(
            second_events.try_recv(),
            Ok(EditorEvent::Key(VirtualKeyCode::A, ElementState::Released))
        ));
        assert!(first_events.try_recv().is_err());
    }

    #[test]
    fn the_play_button_resumes_live_only_at_the_paused_tip() {
        let frame = Some(Frame { id: 2, at_ms: 100 });
        let paused_at_tip = TimelineStatus {
            cursor: frame,
            tip: frame,
            paused: true,
            ..TimelineStatus::default()
        };
        assert!(paused_at_tip.play_resumes_live());
        for status in [
            TimelineStatus {
                paused: false,
                ..paused_at_tip
            },
            TimelineStatus {
                playing: true,
                ..paused_at_tip
            },
            TimelineStatus {
                cursor: Some(Frame { id: 1, at_ms: 50 }),
                ..paused_at_tip
            },
            TimelineStatus {
                detached_bookmark: Some(1),
                ..paused_at_tip
            },
            TimelineStatus {
                cursor: None,
                tip: None,
                ..paused_at_tip
            },
        ] {
            assert!(!status.play_resumes_live());
        }
    }

    #[test]
    fn take_over_enables_only_for_a_paused_checkpoint_behind_the_tip() {
        let first = Frame { id: 1, at_ms: 0 };
        let last = Frame { id: 2, at_ms: 100 };
        let history = TimelineStatus {
            cursor: Some(first),
            tip: Some(last),
            paused: true,
            ..TimelineStatus::default()
        };
        assert_eq!(
            dock_command(Transport::TakeOver, history),
            Some(TimelineCommand::ResumeLive)
        );
        assert_eq!(
            dock_command(
                Transport::TakeOver,
                TimelineStatus {
                    cursor: Some(last),
                    ..history
                }
            ),
            None
        );
        assert_eq!(
            dock_command(
                Transport::TakeOver,
                TimelineStatus {
                    paused: false,
                    ..history
                }
            ),
            None
        );
    }

    #[test]
    fn disabling_one_bridge_leaves_another_bridge_receiving_controls() {
        let (first, first_events) = Bridge::with_test_input();
        let (second, second_events) = Bridge::with_test_input();
        first.toggle_lifecycle();
        first.send(EditorEvent::Timeline(TimelineCommand::TogglePause));
        second.send(EditorEvent::Timeline(TimelineCommand::TogglePause));
        assert_eq!(first.lifecycle(), Lifecycle::Disabling);
        assert_eq!(second.lifecycle(), Lifecycle::Enabled);
        assert!(first_events.try_recv().is_err());
        assert!(matches!(
            second_events.try_recv(),
            Ok(EditorEvent::Timeline(TimelineCommand::TogglePause))
        ));
        first.finish_disable(None, None);
        assert_eq!(first.snapshot().phase, Phase::Disabled);
        assert!(second.snapshot().connected);
    }
}
