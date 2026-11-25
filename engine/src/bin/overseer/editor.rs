use std::error::Error;
use std::io::{self, BufReader};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use engine::embedded::{
    DEFAULT_PREVIEW_SIZE, EditorEvent, PreviewFrame, TimelineStatus, write_message,
};
#[cfg(target_os = "macos")]
use engine::shared_surface::{self, SurfaceInbox};
use engine::surface::SurfaceSize;
use engine::timeline::TimelineCommand;
use pixels::{Pixels, PixelsBuilder, SurfaceTexture};
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, ModifiersState, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use winit::platform::run_return::EventLoopExtRunReturn;
use winit::window::{CursorIcon, WindowBuilder};

#[path = "board.rs"]
mod board;
#[path = "canvas.rs"]
mod canvas;

use board::{Board, Bounds, Edges, Panel, Point, TITLE_HEIGHT, TimelineLayout, Transport};
use canvas::{Canvas, Chrome, viewport};

const BOARD_PATH: &str = ".sycho-board.json";
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(16);
const SEEK_INTERVAL: Duration = Duration::from_millis(55);
const SAVE_DELAY: Duration = Duration::from_millis(350);
const PANEL_DRAG_THRESHOLD: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Building,
    Reconnecting,
    Running,
    Failed,
    GpuFailed,
    ReloadFailed,
    Exited,
}

#[derive(Clone)]
pub struct Bridge(Arc<Mutex<Shared>>);

struct Shared {
    generation: Option<u64>,
    phase: Phase,
    input: Option<Sender<EditorEvent>>,
    game_size: SurfaceSize,
    frame: Option<Arc<PreviewFrame>>,
    redraw: Option<EventLoopProxy<()>>,
    redraw_pending: bool,
    #[cfg(target_os = "macos")]
    gpu_device: Option<u64>,
}

impl Shared {
    fn request_redraw(&mut self) {
        if !self.redraw_pending
            && let Some(redraw) = &self.redraw
        {
            self.redraw_pending = redraw.send_event(()).is_ok();
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    generation: Option<u64>,
    phase: Phase,
    connected: bool,
    frame: Option<Arc<PreviewFrame>>,
}

impl Snapshot {
    fn timeline(&self) -> TimelineStatus {
        self.frame
            .as_ref()
            .map_or(TimelineStatus::default(), |frame| frame.status)
    }
}

impl Bridge {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Shared {
            generation: None,
            phase: Phase::Building,
            input: None,
            game_size: DEFAULT_PREVIEW_SIZE,
            frame: None,
            redraw: None,
            redraw_pending: false,
            #[cfg(target_os = "macos")]
            gpu_device: None,
        })))
    }

    pub fn set_phase(&self, phase: Phase) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.phase = phase;
        shared.request_redraw();
    }

    pub fn disconnect(&self, phase: Phase) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.generation = None;
        shared.phase = phase;
        shared.input = None;
        shared.request_redraw();
    }

    pub fn endpoint(&self, generation: u64) -> io::Result<(SocketAddr, Option<String>)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let transport = None;
        #[cfg(target_os = "macos")]
        let (inbox, transport) = {
            let device = self.0.lock().expect("editor bridge").gpu_device;
            match device.map(SurfaceInbox::open) {
                Some(Ok((inbox, endpoint))) => (Some(inbox), Some(endpoint)),
                Some(Err(err)) => {
                    eprintln!("could not register shared preview surfaces, using RGBA: {err}");
                    (None, None)
                }
                None => (None, transport),
            }
        };
        {
            let mut shared = self.0.lock().expect("editor bridge");
            shared.generation = Some(generation);
            shared.phase = Phase::Reconnecting;
            shared.input = None;
            shared.request_redraw();
        }
        let bridge = self.clone();
        thread::spawn(move || {
            loop {
                if bridge.0.lock().expect("editor bridge").generation != Some(generation) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        bridge.read_game(
                            stream,
                            generation,
                            #[cfg(target_os = "macos")]
                            inbox.as_ref(),
                        );
                        return;
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(ACCEPT_POLL_INTERVAL);
                    }
                    Err(err) => {
                        eprintln!("editor preview connection failed: {err}");
                        bridge.disconnect_generation(generation);
                        return;
                    }
                }
            }
        });
        Ok((address, transport))
    }

    fn disconnect_generation(&self, generation: u64) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.generation == Some(generation) {
            shared.input = None;
            if matches!(shared.phase, Phase::Running | Phase::Reconnecting) {
                shared.phase = Phase::Reconnecting;
            }
            shared.request_redraw();
        }
    }

    fn read_game(
        &self,
        stream: TcpStream,
        generation: u64,
        #[cfg(target_os = "macos")] inbox: Option<&SurfaceInbox>,
    ) {
        // Accepted sockets can inherit the listener's nonblocking mode on macOS.
        if let Err(err) = stream
            .set_nonblocking(false)
            .and_then(|()| stream.set_nodelay(true))
        {
            eprintln!("editor preview setup failed: {err}");
            return;
        }
        let Ok(mut writer) = stream.try_clone() else {
            return;
        };
        if writer
            .set_write_timeout(Some(Duration::from_secs(3)))
            .is_err()
        {
            return;
        }
        let (input, events) = mpsc::channel();
        thread::spawn(move || {
            for event in events {
                if write_message(&mut writer, &event).is_err() {
                    let _ = writer.shutdown(Shutdown::Both);
                    break;
                }
            }
        });
        let mut reader = BufReader::new(stream);
        #[cfg(target_os = "macos")]
        let release = input.clone();
        let mut input = Some(input);
        loop {
            #[cfg(target_os = "macos")]
            let incoming = PreviewFrame::read_with_surfaces(&mut reader, inbox, &release);
            #[cfg(not(target_os = "macos"))]
            let incoming = PreviewFrame::read(&mut reader).map(Some);
            let frame = match incoming {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(err) => {
                    if !matches!(
                        err.kind(),
                        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset
                    ) {
                        eprintln!("editor could not read the game preview: {err}");
                    }
                    break;
                }
            };
            let mut shared = self.0.lock().expect("editor bridge");
            if shared.generation != Some(generation) {
                break;
            }
            if let Some(input) = input.take() {
                if frame.size != shared.game_size {
                    let _ = input.send(EditorEvent::Resize(shared.game_size));
                }
                shared.input = Some(input);
                if shared.phase == Phase::Reconnecting {
                    shared.phase = Phase::Running;
                }
            }
            shared.frame = Some(Arc::new(frame));
            shared.request_redraw();
        }
        self.disconnect_generation(generation);
    }

    fn snapshot(&self) -> Snapshot {
        let shared = self.0.lock().expect("editor bridge");
        Snapshot {
            generation: shared.generation,
            phase: shared.phase,
            connected: shared.input.is_some(),
            frame: shared.frame.clone(),
        }
    }

    fn send(&self, event: EditorEvent) {
        if let Some(input) = &self.0.lock().expect("editor bridge").input {
            let _ = input.send(event);
        }
    }

    fn resize_game(&self, size: SurfaceSize) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.game_size != size {
            shared.game_size = size;
            if let Some(input) = &shared.input {
                let _ = input.send(EditorEvent::Resize(size));
            }
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
        panel: Panel,
        origin: Point,
        bounds: Bounds,
        allow_dock: bool,
    },
    UndockTimeline {
        origin: Point,
        bounds: Bounds,
    },
    Resize {
        panel: Panel,
        origin: Point,
        bounds: Bounds,
        edges: Edges,
    },
    ZoomGame,
    Scrub,
}

impl Gesture {
    fn button(self) -> MouseButton {
        match self {
            Self::Pan { button, .. } => button,
            _ => MouseButton::Left,
        }
    }
}

#[derive(Default)]
struct Interaction {
    cursor: Option<Point>,
    preview_focus: bool,
    preview_buttons: Vec<MouseButton>,
    gesture: Option<Gesture>,
    drag_ms: Option<u64>,
    pending_seek: Option<u64>,
    last_seek: Option<Instant>,
    modifiers: ModifiersState,
    generation: Option<u64>,
}

impl Interaction {
    fn update_connection(&mut self, bridge: &Bridge, snapshot: &Snapshot, window_focused: bool) {
        let generation = snapshot.generation.filter(|_| snapshot.connected);
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        self.pending_seek = None;
        if matches!(self.gesture, Some(Gesture::Scrub)) {
            self.gesture = None;
            self.drag_ms = None;
        }
        self.preview_buttons.clear();
        if generation.is_some() {
            bridge.send(EditorEvent::Focus(window_focused));
        }
    }

    fn game_point(&self, board: Board, snapshot: &Snapshot, dpi: f64) -> Option<(u32, u32)> {
        let cursor = self.cursor?;
        let world = board.world_at(cursor);
        let size = snapshot.frame.as_ref()?.size;
        if board.panel_at(world) != Some(Panel::Game)
            || (!board.timeline_docked && board.game.grip().contains(world))
            || size != board.game_size(dpi)
        {
            return None;
        }
        board.game_body().pixel(world, size)
    }

    fn pointer(
        &mut self,
        bridge: &Bridge,
        board: &mut Board,
        cursor: Option<Point>,
        snapshot: &Snapshot,
        dpi: f64,
    ) {
        let was_in_game = self.game_point(*board, snapshot, dpi).is_some();
        self.cursor = cursor;
        match (self.gesture, cursor) {
            (Some(Gesture::Pan { origin, pan, .. }), Some(point)) => {
                board.pan = Point::new(pan.x + point.x - origin.x, pan.y + point.y - origin.y);
            }
            (
                Some(Gesture::Move {
                    panel,
                    origin,
                    bounds,
                    ..
                }),
                Some(point),
            ) => {
                let at = board.world_at(point);
                board.set_bounds(
                    panel,
                    bounds.translated(Point::new(at.x - origin.x, at.y - origin.y)),
                );
            }
            (Some(Gesture::UndockTimeline { origin, bounds }), Some(point)) => {
                let at = board.world_at(point);
                let delta = Point::new(at.x - origin.x, at.y - origin.y);
                if delta.x.hypot(delta.y) * board.zoom >= PANEL_DRAG_THRESHOLD {
                    board.timeline_docked = false;
                    board.set_bounds(Panel::Timeline, bounds.translated(delta));
                    self.gesture = Some(Gesture::Move {
                        panel: Panel::Timeline,
                        origin,
                        bounds,
                        allow_dock: false,
                    });
                }
            }
            (
                Some(Gesture::Resize {
                    panel,
                    origin,
                    bounds,
                    edges,
                }),
                Some(point),
            ) => {
                let at = board.world_at(point);
                board.set_bounds(
                    panel,
                    edges.resize(
                        panel,
                        bounds,
                        Point::new(at.x - origin.x, at.y - origin.y),
                        dpi,
                    ),
                );
            }
            (Some(Gesture::ZoomGame), Some(point)) => {
                board.game_zoom = board.game_header().zoom_at(board.world_at(point).x);
            }
            (Some(Gesture::Scrub), _) => self.seek_cursor(*board, snapshot.timeline()),
            _ if self.preview_focus => {
                let position = self.game_point(*board, snapshot, dpi);
                if position.is_some() || was_in_game || !self.preview_buttons.is_empty() {
                    let size = board.game_size(dpi);
                    bridge.send(EditorEvent::Pointer(
                        position.or(Some((size.width, size.height))),
                    ));
                }
            }
            _ => {}
        }
    }

    fn dock_candidate(&self, board: Board) -> bool {
        let Some(Gesture::Move {
            panel: Panel::Timeline,
            bounds,
            allow_dock: true,
            ..
        }) = self.gesture
        else {
            return false;
        };
        let moved = (board.timeline.x - bounds.x).hypot(board.timeline.y - bounds.y);
        moved * board.zoom >= PANEL_DRAG_THRESHOLD && board.can_dock_timeline()
    }

    fn seek_cursor(&mut self, board: Board, timeline: TimelineStatus) {
        if let (Some(point), Some(tip)) = (self.cursor, timeline.tip) {
            let track = TimelineLayout::new(board.timeline).track;
            let x = board.world_at(point).x;
            let fraction = ((x - track.x) / track.w).clamp(0.0, 1.0);
            let at = (tip.at_ms as f64 * fraction).round() as u64;
            self.drag_ms = Some(at);
            // Pausing live play can publish a newer tip after the drag starts.
            self.pending_seek = Some(if x >= track.right() { u64::MAX } else { at });
        }
    }

    fn flush_seek(&mut self, bridge: &Bridge, force: bool) {
        if (force
            || self
                .last_seek
                .is_none_or(|last| last.elapsed() >= SEEK_INTERVAL))
            && let Some(at) = self.pending_seek.take()
        {
            bridge.send(EditorEvent::Timeline(TimelineCommand::Seek(at)));
            self.last_seek = Some(Instant::now());
        }
    }

    fn wait(&self, save_at: Option<Instant>) -> ControlFlow {
        let seek = self
            .pending_seek
            .and(self.last_seek)
            .map(|last| last + SEEK_INTERVAL);
        seek.into_iter()
            .chain(save_at)
            .min()
            .map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
    }

    fn hovered_button(&self, board: Board) -> Option<Transport> {
        let cursor = self.cursor?;
        let world = board.world_at(cursor);
        if board.panel_at(world) == Some(Panel::Timeline) {
            TimelineLayout::new(board.timeline).hit_button(world)
        } else {
            None
        }
    }

    fn set_preview_focus(&mut self, bridge: &Bridge, focused: bool) {
        if self.preview_focus != focused {
            self.preview_focus = focused;
            self.preview_buttons.clear();
            if !focused {
                bridge.send(EditorEvent::ReleaseInput);
            }
        }
    }

    fn mouse(
        &mut self,
        bridge: &Bridge,
        board: &mut Board,
        snapshot: &Snapshot,
        display: Display,
        button: MouseButton,
        state: ElementState,
    ) {
        if state == ElementState::Released {
            if self
                .gesture
                .is_some_and(|gesture| gesture.button() == button)
            {
                if matches!(self.gesture, Some(Gesture::Scrub)) {
                    self.flush_seek(bridge, true);
                    self.drag_ms = None;
                }
                if self.dock_candidate(*board) {
                    board.dock_timeline();
                }
                self.gesture = None;
            }
            if self.preview_buttons.contains(&button) {
                self.preview_buttons.retain(|held| *held != button);
                bridge.send(EditorEvent::Button(button, state));
            }
            return;
        }
        if self.gesture.is_some() {
            return;
        }
        let Some(point) = self.cursor else { return };
        let world = board.world_at(point);
        let panel = board.panel_at(world);
        if button == MouseButton::Middle || (panel.is_none() && button == MouseButton::Left) {
            self.set_preview_focus(bridge, false);
            self.gesture = Some(Gesture::Pan {
                button,
                origin: point,
                pan: board.pan,
            });
            return;
        }

        let Some(panel) = panel else { return };
        let bounds = board.bounds(panel);
        if button == MouseButton::Left {
            board.front = panel;
            if panel == Panel::Game && board.game_header().hit_zoom(world) {
                board.game_zoom = board.game_header().zoom_at(world.x);
                self.gesture = Some(Gesture::ZoomGame);
                return;
            }
            if let Some(edges) = Edges::within(*board, panel, world) {
                self.set_preview_focus(bridge, false);
                // The Timeline keeps its height when the dock's outer edge is dragged.
                let panel = if board.timeline_docked {
                    Panel::Game
                } else {
                    panel
                };
                self.gesture = Some(Gesture::Resize {
                    panel,
                    origin: world,
                    bounds: board.bounds(panel),
                    edges,
                });
                return;
            }
            if world.y < bounds.y + TITLE_HEIGHT {
                self.set_preview_focus(bridge, false);
                self.gesture = Some(if board.timeline_docked && panel == Panel::Timeline {
                    Gesture::UndockTimeline {
                        origin: world,
                        bounds,
                    }
                } else {
                    Gesture::Move {
                        panel,
                        origin: world,
                        bounds,
                        allow_dock: true,
                    }
                });
                return;
            }
        }

        if panel == Panel::Game {
            self.set_preview_focus(bridge, true);
            if let Some(position) = self.game_point(*board, snapshot, display.dpi)
                && snapshot.connected
            {
                bridge.send(EditorEvent::Pointer(Some(position)));
                bridge.send(EditorEvent::Button(button, state));
                self.preview_buttons.push(button);
            }
        } else if button == MouseButton::Left {
            self.set_preview_focus(bridge, false);
            if !snapshot.connected {
                return;
            }
            let layout = TimelineLayout::new(bounds);
            let timeline = snapshot.timeline();
            if let Some(command) = layout
                .hit_button(world)
                .and_then(|control| dock_command(control, timeline))
            {
                bridge.send(EditorEvent::Timeline(command));
            } else if layout.hit_track(world) && timeline.tip.is_some() {
                self.gesture = Some(Gesture::Scrub);
                self.seek_cursor(*board, timeline);
                self.flush_seek(bridge, true);
            }
        }
    }

    fn key(
        &mut self,
        bridge: &Bridge,
        board: &mut Board,
        display: Display,
        key: VirtualKeyCode,
        state: ElementState,
    ) {
        if key == VirtualKeyCode::Key0 && (self.modifiers.logo() || self.modifiers.ctrl()) {
            if state == ElementState::Pressed {
                self.set_preview_focus(bridge, false);
                board.fit(viewport(display.logical()));
            }
        } else if key == VirtualKeyCode::Tab {
            if state == ElementState::Pressed {
                self.set_preview_focus(bridge, !self.preview_focus);
                board.front = if self.preview_focus {
                    Panel::Game
                } else {
                    Panel::Timeline
                };
            }
        } else if self.preview_focus {
            bridge.send(EditorEvent::Key(key, state));
        } else if state == ElementState::Pressed && self.modifiers.alt() {
            let command = match key {
                VirtualKeyCode::P => TimelineCommand::TogglePause,
                VirtualKeyCode::Left => TimelineCommand::Rewind,
                VirtualKeyCode::Right => TimelineCommand::Forward,
                VirtualKeyCode::Space => TimelineCommand::TogglePlayback,
                _ => return,
            };
            bridge.send(EditorEvent::Timeline(command));
        }
    }

    fn wheel(
        &self,
        bridge: &Bridge,
        board: &mut Board,
        snapshot: &Snapshot,
        dpi: f64,
        delta: MouseScrollDelta,
    ) {
        if self.gesture.is_some() {
            return;
        }
        let (x, y) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x, y),
            MouseScrollDelta::PixelDelta(value) => (value.x as f32 / 100.0, value.y as f32 / 100.0),
        };
        if !self.modifiers.logo()
            && !self.modifiers.ctrl()
            && self.preview_focus
            && self.game_point(*board, snapshot, dpi).is_some()
        {
            bridge.send(EditorEvent::Scroll(x, y));
        } else if let Some(point) = self.cursor {
            board.zoom_at(point, (f64::from(y) * 0.16).clamp(-0.9, 0.9).exp());
        }
    }

    fn cursor_icon(&self, board: Board) -> CursorIcon {
        let edges = match self.gesture {
            Some(Gesture::Resize { edges, .. }) => Some(edges),
            Some(_) => return CursorIcon::Grabbing,
            None => {
                let Some(point) = self.cursor else {
                    return CursorIcon::Default;
                };
                let world = board.world_at(point);
                let Some(panel) = board.panel_at(world) else {
                    return CursorIcon::Grab;
                };
                if panel == Panel::Game && board.game_header().hit_zoom(world) {
                    return CursorIcon::Hand;
                }
                let edges = Edges::within(board, panel, world);
                if edges.is_none() && world.y < board.bounds(panel).y + TITLE_HEIGHT {
                    return CursorIcon::Grab;
                }
                edges
            }
        };
        match edges {
            Some(Edges { horizontal: 0, .. }) => CursorIcon::NsResize,
            Some(Edges { vertical: 0, .. }) => CursorIcon::EwResize,
            Some(Edges {
                horizontal,
                vertical,
            }) if horizontal == vertical => CursorIcon::NwseResize,
            Some(_) => CursorIcon::NeswResize,
            None => CursorIcon::Default,
        }
    }
}

fn dock_command(control: Transport, timeline: TimelineStatus) -> Option<TimelineCommand> {
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

#[derive(Clone, Copy)]
struct Display {
    surface: SurfaceSize,
    dpi: f64,
}

impl Display {
    fn logical(self) -> SurfaceSize {
        SurfaceSize::new(
            (f64::from(self.surface.width) / self.dpi).round().max(1.0) as u32,
            (f64::from(self.surface.height) / self.dpi).round().max(1.0) as u32,
        )
    }

    fn resize(
        &mut self,
        pixels: &mut Pixels,
        size: winit::dpi::PhysicalSize<u32>,
    ) -> Result<(), pixels::TextureError> {
        if size.width != 0 && size.height != 0 {
            pixels.resize_surface(size.width, size.height)?;
            self.surface = SurfaceSize::new(size.width, size.height);
        }
        Ok(())
    }
}

fn save_board(path: &Path, board: Board, saved: &mut Option<Board>) {
    if *saved != Some(board) {
        match board.save(path) {
            Ok(()) => *saved = Some(board),
            Err(err) => eprintln!("could not save the editor board: {err}"),
        }
    }
}

pub fn run(bridge: Bridge, game_name: &str, ready: impl FnOnce()) -> Result<(), Box<dyn Error>> {
    let mut events = EventLoopBuilder::<()>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(format!("Sycho Editor · {game_name}"))
        .with_inner_size(LogicalSize::new(1120.0, 850.0))
        .with_min_inner_size(LogicalSize::new(535.0, 390.0))
        .build(&events)?;
    let size = window.inner_size();
    let mut display = Display {
        surface: SurfaceSize::new(size.width, size.height),
        dpi: window.scale_factor(),
    };
    let builder = PixelsBuilder::new(1, 1, SurfaceTexture::new(size.width, size.height, &window));
    #[cfg(target_os = "macos")]
    let use_shared_surfaces = std::env::var("SYCHO_PREVIEW_TRANSPORT").as_deref() != Ok("rgba");
    #[cfg(target_os = "macos")]
    let builder = if use_shared_surfaces {
        // The game uses this same device. On dual-GPU Macs, defaulting to the
        // integrated editor device can put the game on a slower GPU than RGBA.
        builder.request_adapter_options(pixels::wgpu::RequestAdapterOptions {
            power_preference: pixels::wgpu::util::power_preference_from_env()
                .unwrap_or(pixels::wgpu::PowerPreference::HighPerformance),
            compatible_surface: None,
            force_fallback_adapter: false,
        })
    } else {
        builder
    };
    let mut pixels = builder.build()?;
    let mut canvas = Canvas::new(&pixels);
    let path = Path::new(BOARD_PATH);
    let mut saved = match Board::load(path) {
        Ok(board) => board,
        Err(err) => {
            eprintln!("could not load the editor board: {err}");
            None
        }
    };
    let mut board = saved.unwrap_or_else(|| {
        let mut board = Board::default();
        board.fit(viewport(display.logical()));
        board
    });
    let mut save_at = None;
    let mut input = Interaction::default();
    let mut window_focused = true;
    let mut failure: Option<Box<dyn Error>> = None;
    bridge.resize_game(board.game_size(display.dpi));
    {
        let mut shared = bridge.0.lock().expect("editor bridge");
        shared.redraw = Some(events.create_proxy());
        #[cfg(target_os = "macos")]
        if use_shared_surfaces {
            shared.gpu_device = shared_surface::device_id(&pixels.context().device);
        }
    }
    ready();
    window.request_redraw();
    events.run_return(|event, _, control| {
        *control = input.wait(save_at);
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control = ControlFlow::Exit,
            Event::WindowEvent { event, .. } => {
                let snapshot = bridge.snapshot();
                input.update_connection(&bridge, &snapshot, window_focused);
                let previous_board = board;
                let previous_focus = input.preview_focus;
                let previous_drag = input.drag_ms;
                let previous_hover = input.hovered_button(board);
                let previous_dock_target = input.dock_candidate(board);
                let resized = match event {
                    WindowEvent::Resized(size) => Some(display.resize(&mut pixels, size)),
                    WindowEvent::ScaleFactorChanged {
                        scale_factor,
                        new_inner_size,
                    } => {
                        display.dpi = scale_factor;
                        bridge.resize_game(board.game_size(display.dpi));
                        Some(display.resize(&mut pixels, *new_inner_size))
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let position = position.to_logical::<f64>(display.dpi);
                        input.pointer(
                            &bridge,
                            &mut board,
                            Some(Point::new(position.x, position.y)),
                            &snapshot,
                            display.dpi,
                        );
                        None
                    }
                    WindowEvent::CursorLeft { .. } if input.gesture.is_none() => {
                        input.pointer(&bridge, &mut board, None, &snapshot, display.dpi);
                        None
                    }
                    WindowEvent::MouseInput { button, state, .. } => {
                        input.mouse(&bridge, &mut board, &snapshot, display, button, state);
                        None
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        input.wheel(&bridge, &mut board, &snapshot, display.dpi, delta);
                        None
                    }
                    WindowEvent::TouchpadMagnify { delta, .. } => {
                        if input.gesture.is_none()
                            && let Some(point) = input.cursor
                        {
                            board.zoom_at(point, delta.clamp(-0.9, 0.9).exp());
                        }
                        None
                    }
                    WindowEvent::ModifiersChanged(modifiers) => {
                        input.modifiers = modifiers;
                        None
                    }
                    WindowEvent::KeyboardInput { input: key, .. } => {
                        if let Some(code) = key.virtual_keycode {
                            input.key(&bridge, &mut board, display, code, key.state);
                        }
                        None
                    }
                    WindowEvent::Focused(focused) => {
                        window_focused = focused;
                        bridge.send(EditorEvent::Focus(focused));
                        if !focused {
                            input.preview_buttons.clear();
                            input.modifiers = ModifiersState::empty();
                            input.gesture = None;
                            input.drag_ms = None;
                            input.pending_seek = None;
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(result) = resized {
                    if let Err(err) = result {
                        failure = Some(err.into());
                        *control = ControlFlow::Exit;
                    }
                    window.request_redraw();
                }
                if previous_board != board {
                    bridge.resize_game(board.game_size(display.dpi));
                    save_at = Some(Instant::now() + SAVE_DELAY);
                }
                if previous_board != board
                    || previous_focus != input.preview_focus
                    || previous_drag != input.drag_ms
                    || previous_hover != input.hovered_button(board)
                    || previous_dock_target != input.dock_candidate(board)
                {
                    window.request_redraw();
                }
                window.set_cursor_icon(input.cursor_icon(board));
            }
            Event::UserEvent(()) => {
                bridge.0.lock().expect("editor bridge").redraw_pending = false;
                window.request_redraw();
            }
            Event::MainEventsCleared => {
                input.flush_seek(&bridge, false);
                if save_at.is_some_and(|at| Instant::now() >= at) {
                    save_board(path, board, &mut saved);
                    save_at = None;
                }
                *control = input.wait(save_at);
                #[cfg(target_os = "macos")]
                canvas.poll_gpu(&pixels, control);
            }
            Event::RedrawRequested(_) => {
                let snapshot = bridge.snapshot();
                input.update_connection(&bridge, &snapshot, window_focused);
                let chrome = Chrome {
                    board,
                    size: display.logical(),
                    phase: snapshot.phase,
                    connected: snapshot.connected,
                    frame_size: snapshot.frame.as_ref().map(|frame| frame.size),
                    timeline: snapshot.timeline(),
                    preview_focus: input.preview_focus,
                    hovered: input.hovered_button(board),
                    drag_ms: input.drag_ms,
                    dock_target: input.dock_candidate(board),
                };
                if let Err(err) =
                    canvas.draw(&pixels, chrome, display.surface, snapshot.frame.as_ref())
                {
                    failure = Some(err.into());
                    *control = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
    save_board(path, board, &mut saved);
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::timeline::Frame;

    fn fixture() -> (Bridge, mpsc::Receiver<EditorEvent>, Snapshot) {
        let bridge = Bridge::new();
        let (sender, events) = mpsc::channel();
        bridge.0.lock().unwrap().input = Some(sender);
        let tip = Some(Frame {
            id: 5,
            at_ms: 1_000,
        });
        let snapshot = Snapshot {
            generation: Some(1),
            phase: Phase::Running,
            connected: true,
            frame: Some(Arc::new(PreviewFrame {
                size: DEFAULT_PREVIEW_SIZE,
                status: TimelineStatus {
                    cursor: tip,
                    tip,
                    paused: false,
                    playing: false,
                },
                pixels: Vec::new(),
                #[cfg(target_os = "macos")]
                surface: None,
            })),
        };
        (bridge, events, snapshot)
    }

    #[test]
    fn preview_and_scrubber_use_their_moved_and_zoomed_panel_bounds() {
        let (bridge, events, snapshot) = fixture();
        let mut board = Board {
            zoom: 0.75,
            pan: Point::new(120.0, 90.0),
            ..Board::default()
        };
        let display = Display {
            surface: SurfaceSize::new(1400, 1100),
            dpi: 1.0,
        };
        let game = board.game_body();
        let mut input = Interaction {
            cursor: Some(board.screen_at(Point::new(game.x + 12.25, game.y + 24.25))),
            ..Interaction::default()
        };
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Pressed,
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Pointer(Some((12, 24)))
        ));
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Button(MouseButton::Left, ElementState::Pressed)
        ));
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Released,
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Button(MouseButton::Left, ElementState::Released)
        ));

        let track = TimelineLayout::new(board.timeline).track;
        input.cursor = Some(board.screen_at(Point::new(track.x + track.w / 2.0, track.y)));
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Pressed,
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::ReleaseInput
        ));
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(500))
        ));
        let end = board.screen_at(Point::new(track.right() + 1.0, track.y));
        input.pointer(&bridge, &mut board, Some(end), &snapshot, display.dpi);
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Released,
        );
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(u64::MAX))
        ));
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn resizing_a_panel_notifies_only_the_game_viewport_changes() {
        let (bridge, events, snapshot) = fixture();
        let mut board = Board::default();
        let display = Display {
            surface: SurfaceSize::new(2000, 1800),
            dpi: 2.0,
        };
        let mut input = Interaction::default();
        for panel in [Panel::Timeline, Panel::Game] {
            let bounds = board.bounds(panel);
            let point = Point::new(bounds.right() - 2.0, bounds.bottom() - 2.0);
            let size = board.game_size(display.dpi);
            input.cursor = Some(board.screen_at(point));
            input.mouse(
                &bridge,
                &mut board,
                &snapshot,
                display,
                MouseButton::Left,
                ElementState::Pressed,
            );
            let end = board.screen_at(Point::new(point.x + 45.0, point.y - 4.0));
            input.pointer(&bridge, &mut board, Some(end), &snapshot, display.dpi);
            input.mouse(
                &bridge,
                &mut board,
                &snapshot,
                display,
                MouseButton::Left,
                ElementState::Released,
            );
            assert!((board.bounds(panel).w - bounds.w - 45.0).abs() < 0.01);
            if panel == Panel::Game {
                let resized = board.game_size(display.dpi);
                assert_eq!(resized, SurfaceSize::new(size.width + 90, size.height - 8));
                bridge.resize_game(resized);
                assert!(
                    matches!(events.try_recv().unwrap(), EditorEvent::Resize(received) if received == resized)
                );
                board.zoom_at(Point::new(400.0, 400.0), 1.45);
                bridge.resize_game(board.game_size(display.dpi));
                assert!(events.try_recv().is_err());
            } else {
                assert_eq!(board.game_size(display.dpi), size);
            }
        }
    }

    #[test]
    fn game_header_zoom_resizes_the_framebuffer_and_maps_the_full_panel() {
        let (bridge, events, mut snapshot) = fixture();
        let mut board = Board {
            zoom: 0.8,
            pan: Point::new(75.0, 60.0),
            ..Board::default()
        };
        let display = Display {
            surface: SurfaceSize::new(1400, 1100),
            dpi: 1.0,
        };
        let body = board.game_body();
        let track = board.game_header().zoom_track;
        let mut input = Interaction {
            cursor: Some(board.screen_at(Point::new(track.x + track.w / 2.0, track.y))),
            ..Interaction::default()
        };
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Pressed,
        );
        assert_eq!(board.game_zoom, 1.0);
        let right = board.screen_at(Point::new(track.right() + 4.0, track.y));
        input.pointer(&bridge, &mut board, Some(right), &snapshot, display.dpi);
        input.mouse(
            &bridge,
            &mut board,
            &snapshot,
            display,
            MouseButton::Left,
            ElementState::Released,
        );
        let size = board.game_size(display.dpi);
        bridge.resize_game(size);
        assert_eq!(board.game_zoom, 2.0);
        assert_eq!(size, SurfaceSize::new(480, 320));
        assert_eq!(board.game_body(), body);
        assert!(matches!(
            events.try_recv().unwrap(),
            EditorEvent::Resize(requested) if requested == size
        ));

        input.cursor = Some(board.screen_at(Point::new(body.x + 21.0, body.y + 21.0)));
        assert_eq!(input.game_point(board, &snapshot, display.dpi), None);
        Arc::get_mut(snapshot.frame.as_mut().unwrap()).unwrap().size = size;
        assert_eq!(
            input.game_point(board, &snapshot, display.dpi),
            Some((10, 10))
        );

        board.game_zoom = 0.5;
        let size = board.game_size(display.dpi);
        assert_eq!(size, SurfaceSize::new(1920, 1280));
        Arc::get_mut(snapshot.frame.as_mut().unwrap()).unwrap().size = size;
        input.cursor =
            Some(board.screen_at(Point::new(body.right() - 20.25, body.bottom() - 20.25)));
        assert_eq!(
            input.game_point(board, &snapshot, display.dpi),
            Some((1879, 1239))
        );
    }

    #[test]
    fn saved_boards_default_to_unzoomed_floating_panels() {
        let saved = Board::default();
        let mut data = serde_json::to_value(saved).unwrap();
        let fields = data.as_object_mut().unwrap();
        fields.remove("game_zoom");
        fields.remove("timeline_docked");
        assert_eq!(serde_json::from_value::<Board>(data).unwrap(), saved);
    }

    #[test]
    fn preview_zoom_uses_rounded_native_pixel_dimensions() {
        let mut board = Board {
            game: Bounds::new(40.0, 36.0, 922.5, 644.5),
            ..Board::default()
        };
        assert_eq!(board.game_size(2.0), SurfaceSize::new(1841, 1227));
        board.game_zoom = 2.0;
        assert_eq!(board.game_size(2.0), SurfaceSize::new(921, 614));
        board.game_zoom = 0.5;
        assert_eq!(board.game_size(2.0), SurfaceSize::new(3682, 2454));
    }

    #[test]
    fn timeline_docks_moves_resizes_and_detaches_through_the_panel_gestures() {
        let (bridge, _, snapshot) = fixture();
        let mut board = Board::default();
        board.timeline.w = 430.0;
        board.timeline.y = board.game.bottom() + 10.0;
        let timeline_height = board.timeline.h;
        let display = Display {
            surface: SurfaceSize::new(1600, 1250),
            dpi: 1.0,
        };
        let mut input = Interaction::default();
        let mut drag = |board: &mut Board, start: Point, offset: Point| {
            let finish = board.screen_at(Point::new(start.x + offset.x, start.y + offset.y));
            input.cursor = Some(board.screen_at(start));
            input.mouse(
                &bridge,
                board,
                &snapshot,
                display,
                MouseButton::Left,
                ElementState::Pressed,
            );
            input.pointer(&bridge, board, Some(finish), &snapshot, display.dpi);
            let candidate = input.dock_candidate(*board);
            input.mouse(
                &bridge,
                board,
                &snapshot,
                display,
                MouseButton::Left,
                ElementState::Released,
            );
            candidate
        };

        let title = Point::new(board.timeline.x + 45.0, board.timeline.y + 15.0);
        assert!(!drag(&mut board, title, Point::new(0.0, 0.0)));
        assert!(!board.timeline_docked);
        assert!(drag(&mut board, title, Point::new(55.0, -10.0)));
        assert!(board.timeline_docked);
        assert_eq!(
            board.timeline,
            Bounds::new(
                board.game.x,
                board.game.bottom(),
                board.game.w,
                timeline_height
            )
        );

        let game = board.game;
        let title = Point::new(game.x + 45.0, game.y + 15.0);
        drag(&mut board, title, Point::new(30.0, 20.0));
        assert_eq!(board.game, game.translated(Point::new(30.0, 20.0)));
        assert_eq!(board.timeline.x, board.game.x);
        assert_eq!(board.timeline.y, board.game.bottom());

        let dock = board.dock_bounds().unwrap();
        let corner = Point::new(dock.right() - 2.0, dock.bottom() - 2.0);
        drag(&mut board, corner, Point::new(52.0, 30.0));
        let resized = board.dock_bounds().unwrap();
        assert_eq!(resized.w, dock.w + 52.0);
        assert_eq!(resized.h, dock.h + 30.0);
        assert_eq!(board.timeline.h, timeline_height);
        assert_eq!(board.timeline.w, board.game.w);
        assert_eq!(board.game_size(display.dpi), SurfaceSize::new(1012, 670));

        let game = board.game;
        let title = Point::new(board.timeline.x + 45.0, board.timeline.y + 15.0);
        assert!(!drag(&mut board, title, Point::new(50.0, 50.0)));
        assert!(!board.timeline_docked);
        assert_eq!(board.game, game);
        assert_eq!(board.timeline.y, game.bottom() + 50.0);
    }

    #[test]
    fn take_over_resumes_from_a_paused_checkpoint_behind_the_tip() {
        let (bridge, events, mut snapshot) = fixture();
        let mut board = Board::default();
        board.timeline.w = 260.0;
        let layout = TimelineLayout::new(board.timeline);
        let button = layout.button(Transport::TakeOver);
        assert!(layout.button(Transport::Next).right() < button.x);
        let display = Display {
            surface: SurfaceSize::new(1400, 1100),
            dpi: 1.0,
        };
        let mut input = Interaction {
            cursor: Some(board.screen_at(Point::new(button.x + button.w / 2.0, button.y + 12.0))),
            ..Interaction::default()
        };
        let tip = snapshot.timeline().tip;
        for (paused, playing, cursor) in [
            (false, false, tip),
            (true, false, tip),
            (true, true, Some(Frame { id: 3, at_ms: 600 })),
            (true, false, Some(Frame { id: 3, at_ms: 600 })),
        ] {
            Arc::get_mut(snapshot.frame.as_mut().unwrap())
                .unwrap()
                .status = TimelineStatus {
                paused,
                playing,
                cursor,
                tip,
            };
            input.mouse(
                &bridge,
                &mut board,
                &snapshot,
                display,
                MouseButton::Left,
                ElementState::Pressed,
            );
            if paused && !playing && cursor != tip {
                assert!(matches!(
                    events.try_recv().unwrap(),
                    EditorEvent::Timeline(TimelineCommand::ResumeLive)
                ));
            } else {
                assert!(events.try_recv().is_err());
            }
        }
    }
}
