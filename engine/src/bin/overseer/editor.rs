use std::error::Error;
use std::io::{self, BufReader};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use engine::embedded::{
    EditorEvent, PREVIEW_HEIGHT, PREVIEW_WIDTH, PreviewFrame, TimelineStatus, write_message,
};
use engine::graphics::{Color, CpuRenderer, Renderer2d};
use engine::surface::SurfaceSize;
use engine::timeline::TimelineCommand;
use engine::ui::Rect;
use pixels::{Pixels, SurfaceTexture, wgpu};
use winit::dpi::LogicalSize;
use winit::event::{
    ElementState, Event, MouseButton, MouseScrollDelta, VirtualKeyCode, WindowEvent,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::platform::run_return::EventLoopExtRunReturn;
use winit::window::WindowBuilder;

const WIDTH: u32 = 1180;
const HEIGHT: u32 = 912;
const PREVIEW: Rect = Rect::new(110, 76, PREVIEW_WIDTH, PREVIEW_HEIGHT);
const DOCK: Rect = Rect::new(110, 742, PREVIEW_WIDTH, 128);
const TRACK: Rect = Rect::new(138, 787, 904, 12);
const SCRUB_TARGET: Rect = Rect::new(130, 774, 920, 37);
const BACK: Rect = Rect::new(138, 817, 86, 32);
const PLAY: Rect = Rect::new(234, 817, 92, 32);
const NEXT: Rect = Rect::new(336, 817, 86, 32);
const RESUME: Rect = Rect::new(880, 817, 162, 32);
const TICK: Duration = Duration::from_millis(16);
const SEEK_INTERVAL: Duration = Duration::from_millis(55);

const CANVAS: Color = [232, 222, 204, 255];
const GRID: Color = [208, 195, 172, 255];
const PAPER: Color = [248, 241, 224, 255];
const INK: Color = [35, 44, 38, 255];
const MUTED: Color = [102, 100, 83, 255];
const BORDER: Color = [186, 173, 149, 255];
const GREEN: Color = [43, 93, 71, 255];
const GREEN_LIGHT: Color = [224, 234, 213, 255];
const RED: Color = [143, 59, 43, 255];
const RED_LIGHT: Color = [248, 223, 206, 255];
const BLACK: Color = [14, 19, 19, 255];
const WHITE: Color = [251, 246, 230, 255];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Building,
    Reconnecting,
    Running,
    Failed,
    ReloadFailed,
    Exited,
}

#[derive(Clone)]
pub struct Bridge(Arc<Mutex<Shared>>);

struct Shared {
    generation: Option<u64>,
    phase: Phase,
    input: Option<Sender<EditorEvent>>,
    frame: Option<Arc<PreviewFrame>>,
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
            frame: None,
        })))
    }

    pub fn set_phase(&self, phase: Phase) {
        self.0.lock().expect("editor bridge").phase = phase;
    }

    pub fn disconnect(&self) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.generation = None;
        shared.phase = Phase::Exited;
        shared.input = None;
    }

    pub fn endpoint(&self, generation: u64) -> io::Result<SocketAddr> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        {
            let mut shared = self.0.lock().expect("editor bridge");
            shared.generation = Some(generation);
            shared.phase = Phase::Reconnecting;
            shared.input = None;
        }
        let bridge = self.clone();
        thread::spawn(move || {
            loop {
                if bridge.0.lock().expect("editor bridge").generation != Some(generation) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        bridge.read_game(stream, generation);
                        return;
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => thread::sleep(TICK),
                    Err(err) => {
                        eprintln!("editor preview connection failed: {err}");
                        bridge.disconnect_generation(generation);
                        return;
                    }
                }
            }
        });
        Ok(address)
    }

    fn disconnect_generation(&self, generation: u64) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.generation == Some(generation) {
            shared.input = None;
            if matches!(shared.phase, Phase::Running | Phase::Reconnecting) {
                shared.phase = Phase::Reconnecting;
            }
        }
    }

    fn read_game(&self, stream: TcpStream, generation: u64) {
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
        let mut input = Some(input);
        loop {
            let frame = match PreviewFrame::read(&mut reader) {
                Ok(frame) => frame,
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
                shared.input = Some(input);
                if shared.phase == Phase::Reconnecting {
                    shared.phase = Phase::Running;
                }
            }
            shared.frame = Some(Arc::new(frame));
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
}

#[derive(Default)]
struct Interaction {
    cursor: Option<(u32, u32)>,
    preview_focus: bool,
    preview_buttons: Vec<MouseButton>,
    dragging: bool,
    drag_ms: Option<u64>,
    pending_seek: Option<u64>,
    last_seek: Option<Instant>,
    generation: Option<u64>,
}

impl Interaction {
    fn update_connection(&mut self, bridge: &Bridge, snapshot: &Snapshot, window_focused: bool) {
        let generation = snapshot.generation.filter(|_| snapshot.connected);
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        self.dragging = false;
        self.drag_ms = None;
        self.pending_seek = None;
        self.preview_buttons.clear();
        if generation.is_some() {
            bridge.send(EditorEvent::Focus(window_focused));
        }
    }

    fn pointer(&mut self, bridge: &Bridge, cursor: Option<(u32, u32)>, timeline: TimelineStatus) {
        let was_in_preview = self.cursor.is_some_and(|(x, y)| PREVIEW.contains(x, y));
        self.cursor = cursor;
        if self.dragging {
            self.seek_cursor(timeline);
        } else if self.preview_focus {
            let position = cursor.and_then(|(x, y)| {
                PREVIEW
                    .contains(x, y)
                    .then(|| (x - PREVIEW.x, y - PREVIEW.y))
            });
            if position.is_some() || was_in_preview || !self.preview_buttons.is_empty() {
                bridge.send(EditorEvent::Pointer(
                    position.or(Some((PREVIEW_WIDTH, PREVIEW_HEIGHT))),
                ));
            }
        }
    }

    fn seek_cursor(&mut self, timeline: TimelineStatus) {
        if let (Some((x, _)), Some(tip)) = (self.cursor, timeline.tip) {
            let offset = x.saturating_sub(TRACK.x).min(TRACK.w);
            let at = (u128::from(tip.at_ms) * u128::from(offset) / u128::from(TRACK.w)) as u64;
            self.drag_ms = Some(at);
            // Pausing live play can publish a newer tip after the drag starts.
            self.pending_seek = Some(if offset == TRACK.w { u64::MAX } else { at });
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
        snapshot: &Snapshot,
        button: MouseButton,
        state: ElementState,
    ) {
        if state == ElementState::Released {
            if button == MouseButton::Left && self.dragging {
                self.flush_seek(bridge, true);
                self.dragging = false;
                self.drag_ms = None;
            }
            if self.preview_buttons.contains(&button) {
                self.preview_buttons.retain(|held| *held != button);
                bridge.send(EditorEvent::Button(button, state));
            }
            return;
        }
        let Some((x, y)) = self.cursor else { return };
        if PREVIEW.contains(x, y) {
            self.set_preview_focus(bridge, true);
            bridge.send(EditorEvent::Pointer(Some((x - PREVIEW.x, y - PREVIEW.y))));
            bridge.send(EditorEvent::Button(button, state));
            self.preview_buttons.push(button);
        } else if button == MouseButton::Left {
            self.set_preview_focus(bridge, false);
            if !snapshot.connected {
                return;
            }
            let timeline = snapshot.timeline();
            if SCRUB_TARGET.contains(x, y) && timeline.tip.is_some() {
                self.dragging = true;
                self.seek_cursor(timeline);
                self.flush_seek(bridge, true);
            } else if let Some(command) = dock_command(x, y, timeline) {
                bridge.send(EditorEvent::Timeline(command));
            }
        }
    }

    fn key(&mut self, bridge: &Bridge, key: VirtualKeyCode, state: ElementState) {
        if key == VirtualKeyCode::Tab {
            if state == ElementState::Pressed {
                self.set_preview_focus(bridge, !self.preview_focus);
            }
            return;
        }
        if self.preview_focus {
            bridge.send(EditorEvent::Key(key, state));
        }
    }
}

fn dock_command(x: u32, y: u32, timeline: TimelineStatus) -> Option<TimelineCommand> {
    if BACK.contains(x, y) && timeline.cursor.is_some_and(|frame| frame.id > 0) {
        Some(TimelineCommand::Rewind)
    } else if PLAY.contains(x, y) && (!timeline.paused || timeline.cursor != timeline.tip) {
        Some(TimelineCommand::TogglePlayback)
    } else if NEXT.contains(x, y) && timeline.cursor != timeline.tip {
        Some(TimelineCommand::Forward)
    } else if RESUME.contains(x, y) && timeline.paused {
        Some(TimelineCommand::ResumeLive)
    } else {
        None
    }
}

pub fn run(bridge: Bridge, game_name: &str) -> Result<(), Box<dyn Error>> {
    let mut events = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Sycho Editor")
        .with_inner_size(LogicalSize::new(1000.0, 773.0))
        .with_min_inner_size(LogicalSize::new(590.0, 456.0))
        .build(&events)?;
    let size = window.inner_size();
    let mut pixels = Pixels::new(
        WIDTH,
        HEIGHT,
        SurfaceTexture::new(size.width, size.height, &window),
    )?;
    pixels.clear_color(wgpu::Color {
        r: 0.065,
        g: 0.08,
        b: 0.07,
        a: 1.0,
    });
    let background = render_background(game_name);
    let mut input = Interaction::default();
    let mut window_focused = true;
    let mut next_draw = Instant::now();
    let mut failure: Option<Box<dyn Error>> = None;
    events.run_return(|event, _, control| {
        *control = ControlFlow::WaitUntil(next_draw);
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control = ControlFlow::Exit,
            Event::WindowEvent { event, .. } => {
                let snapshot = bridge.snapshot();
                input.update_connection(&bridge, &snapshot, window_focused);
                match event {
                    WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                        if let Err(err) = pixels.resize_surface(size.width, size.height) {
                            failure = Some(err.into());
                            *control = ControlFlow::Exit;
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        let point =
                            pixels.window_pos_to_pixel((position.x as f32, position.y as f32));
                        let point = match point {
                            Ok((x, y)) => Some((x as u32, y as u32)),
                            Err((x, y)) if input.dragging => {
                                Some((x.max(0) as u32, y.max(0) as u32))
                            }
                            _ => None,
                        };
                        input.pointer(&bridge, point, snapshot.timeline());
                    }
                    WindowEvent::CursorLeft { .. } if !input.dragging => {
                        input.pointer(&bridge, None, snapshot.timeline());
                    }
                    WindowEvent::MouseInput { button, state, .. } => {
                        input.mouse(&bridge, &snapshot, button, state);
                    }
                    WindowEvent::MouseWheel { delta, .. }
                        if input.preview_focus
                            && input.cursor.is_some_and(|(x, y)| PREVIEW.contains(x, y)) =>
                    {
                        let (x, y) = match delta {
                            MouseScrollDelta::LineDelta(x, y) => (x, y),
                            MouseScrollDelta::PixelDelta(value) => {
                                (value.x as f32 / 120.0, value.y as f32 / 120.0)
                            }
                        };
                        bridge.send(EditorEvent::Scroll(x, y));
                    }
                    WindowEvent::KeyboardInput { input: key, .. } => {
                        if let Some(code) = key.virtual_keycode {
                            input.key(&bridge, code, key.state);
                        }
                    }
                    WindowEvent::Focused(focused) => {
                        window_focused = focused;
                        bridge.send(EditorEvent::Focus(focused));
                        if !focused {
                            input.preview_buttons.clear();
                            input.dragging = false;
                            input.drag_ms = None;
                            input.pending_seek = None;
                        }
                    }
                    _ => {}
                }
            }
            Event::MainEventsCleared if Instant::now() >= next_draw => {
                next_draw = Instant::now() + TICK;
                input.flush_seek(&bridge, false);
                window.request_redraw();
                *control = ControlFlow::WaitUntil(next_draw);
            }
            Event::RedrawRequested(_) => {
                let snapshot = bridge.snapshot();
                input.update_connection(&bridge, &snapshot, window_focused);
                render(pixels.frame_mut(), &background, &snapshot, &input);
                if let Err(err) = pixels.render() {
                    failure = Some(err.into());
                    *control = ControlFlow::Exit;
                }
            }
            _ => {}
        }
    });
    if let Some(err) = failure {
        return Err(err);
    }
    Ok(())
}

fn fill(gfx: &mut dyn Renderer2d, x: u32, y: u32, w: u32, h: u32, color: Color) {
    gfx.fill_rect(Rect::new(x, y, w, h), color);
}

fn text(gfx: &mut dyn Renderer2d, x: u32, y: u32, value: &str, color: Color, scale: u32) {
    gfx.draw_text_scaled(x, y, value, color, scale);
}

fn button(
    gfx: &mut dyn Renderer2d,
    rect: Rect,
    label: &str,
    enabled: bool,
    primary: bool,
    hover: bool,
) {
    let (bg, fg) = if enabled && primary {
        (if hover { [58, 113, 88, 255] } else { GREEN }, WHITE)
    } else if enabled {
        (if hover { GREEN_LIGHT } else { PAPER }, INK)
    } else {
        ([236, 227, 207, 255], MUTED)
    };
    gfx.fill_rect(rect, bg);
    if !primary {
        gfx.rect_outline(rect, BORDER);
    }
    let label_width = label.chars().count() as u32 * 8;
    text(
        gfx,
        rect.x + (rect.w - label_width) / 2,
        rect.y + 11,
        label,
        fg,
        2,
    );
}

fn timestamp(ms: u64) -> String {
    format!(
        "{}:{:02}.{}",
        ms / 60_000,
        (ms / 1_000) % 60,
        (ms / 100) % 10
    )
}

fn render_background(game_name: &str) -> Vec<u8> {
    let mut pixels = vec![0; WIDTH as usize * HEIGHT as usize * 4];
    let mut gfx = CpuRenderer::new(&mut pixels, SurfaceSize::new(WIDTH, HEIGHT));
    gfx.clear(CANVAS);
    for y in (16..HEIGHT).step_by(20) {
        for x in (16..WIDTH).step_by(20) {
            fill(&mut gfx, x, y, 2, 2, GRID);
        }
    }
    text(&mut gfx, 110, 26, "SYCHO EDITOR", INK, 3);
    text(
        &mut gfx,
        266,
        30,
        &game_name
            .chars()
            .take(26)
            .collect::<String>()
            .to_uppercase(),
        MUTED,
        2,
    );
    pixels
}

fn render(canvas: &mut [u8], background: &[u8], snapshot: &Snapshot, input: &Interaction) {
    canvas.copy_from_slice(background);
    {
        let mut gfx = CpuRenderer::new(canvas, SurfaceSize::new(WIDTH, HEIGHT));
        let (status, fg, bg) = match snapshot.phase {
            Phase::Building => ("BUILDING", INK, PAPER),
            Phase::Reconnecting => ("CONNECTING", INK, PAPER),
            Phase::Running => ("CONNECTED", GREEN, GREEN_LIGHT),
            Phase::Failed => ("BUILD FAILED", RED, RED_LIGHT),
            Phase::ReloadFailed => ("RELOAD FAILED", RED, RED_LIGHT),
            Phase::Exited => ("GAME EXITED", RED, RED_LIGHT),
        };
        fill(&mut gfx, 938, 23, 132, 24, bg);
        text(&mut gfx, 947, 30, status, fg, 2);
        let focus = if input.preview_focus {
            "GAME INPUT ACTIVE"
        } else {
            "CLICK PREVIEW TO PLAY"
        };
        text(&mut gfx, 110, 53, focus, MUTED, 2);
        if matches!(snapshot.phase, Phase::Failed | Phase::ReloadFailed) {
            text(&mut gfx, 869, 53, "DETAILS IN TERMINAL", RED, 2);
        }
        let frame_border = if input.preview_focus { GREEN } else { INK };
        fill(
            &mut gfx,
            PREVIEW.x - 5,
            PREVIEW.y - 5,
            PREVIEW.w + 10,
            PREVIEW.h + 10,
            frame_border,
        );
        if snapshot.frame.is_none() {
            gfx.fill_rect(PREVIEW, BLACK);
            let label = if snapshot.phase == Phase::Failed {
                "WAITING FOR A SOURCE FIX"
            } else {
                "WAITING FOR GAME PREVIEW"
            };
            text(&mut gfx, PREVIEW.x + 336, PREVIEW.y + 305, label, WHITE, 3);
        }

        fill(&mut gfx, DOCK.x + 3, DOCK.y + 5, DOCK.w, DOCK.h, GRID);
        gfx.fill_rect(DOCK, PAPER);
        gfx.rect_outline(DOCK, BORDER);
        let timeline = snapshot.timeline();
        text(&mut gfx, 138, 758, "TIMELINE", INK, 2);
        let cursor = input
            .drag_ms
            .or_else(|| timeline.cursor.map(|frame| frame.at_ms))
            .unwrap_or(0);
        let tip = timeline.tip.map_or(0, |frame| frame.at_ms);
        let mode = if timeline.playing {
            "PLAYBACK"
        } else if timeline.paused {
            "PAUSED"
        } else if snapshot.connected {
            "LIVE"
        } else {
            "OFFLINE"
        };
        text(
            &mut gfx,
            244,
            758,
            mode,
            if timeline.paused { RED } else { GREEN },
            2,
        );
        text(
            &mut gfx,
            778,
            758,
            &format!("{}  /  TIP {}", timestamp(cursor), timestamp(tip)),
            MUTED,
            2,
        );
        gfx.fill_rect(TRACK, [221, 210, 187, 255]);
        let marker = if tip == 0 {
            TRACK.x
        } else {
            TRACK.x + (u128::from(TRACK.w) * u128::from(cursor.min(tip)) / u128::from(tip)) as u32
        };
        fill(&mut gfx, TRACK.x, TRACK.y + 4, marker - TRACK.x, 4, GREEN);
        for i in 0..=10 {
            let x = TRACK.x + TRACK.w * i / 10;
            fill(&mut gfx, x, TRACK.y + 9, 1, 6, BORDER);
        }
        fill(
            &mut gfx,
            (TRACK.x + TRACK.w).saturating_sub(2),
            TRACK.y - 2,
            3,
            16,
            INK,
        );
        fill(
            &mut gfx,
            marker.saturating_sub(4),
            TRACK.y - 4,
            9,
            20,
            GREEN,
        );
        let hover = |rect: Rect| input.cursor.is_some_and(|(x, y)| rect.contains(x, y));
        let in_past = timeline.cursor != timeline.tip;
        button(
            &mut gfx,
            BACK,
            "BACK",
            snapshot.connected && timeline.cursor.is_some_and(|frame| frame.id > 0),
            false,
            hover(BACK),
        );
        let play_label = if timeline.playing || !timeline.paused {
            "PAUSE"
        } else {
            "PLAY"
        };
        button(
            &mut gfx,
            PLAY,
            play_label,
            snapshot.connected && (!timeline.paused || in_past),
            false,
            hover(PLAY),
        );
        button(
            &mut gfx,
            NEXT,
            "NEXT",
            snapshot.connected && in_past,
            false,
            hover(NEXT),
        );
        button(
            &mut gfx,
            RESUME,
            "RESUME LIVE",
            snapshot.connected && timeline.paused,
            true,
            hover(RESUME),
        );
        if timeline.paused && in_past {
            text(
                &mut gfx,
                449,
                828,
                "RESUMING HERE REPLACES LATER HISTORY",
                MUTED,
                2,
            );
        }
    }
    if let Some(frame) = &snapshot.frame {
        let source_stride = PREVIEW_WIDTH as usize * 4;
        for (y, row) in frame.pixels.chunks_exact(source_stride).enumerate() {
            let offset = ((PREVIEW.y as usize + y) * WIDTH as usize + PREVIEW.x as usize) * 4;
            canvas[offset..offset + source_stride].copy_from_slice(row);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::timeline::Frame;

    #[test]
    fn dock_drag_releases_game_input_and_sends_seek_without_resuming() {
        let bridge = Bridge::new();
        let (sender, events) = mpsc::channel();
        bridge.0.lock().unwrap().input = Some(sender);
        let snapshot = Snapshot {
            generation: Some(1),
            phase: Phase::Running,
            connected: true,
            frame: Some(Arc::new(PreviewFrame {
                status: TimelineStatus {
                    cursor: Some(Frame {
                        id: 5,
                        at_ms: 1_000,
                    }),
                    tip: Some(Frame {
                        id: 5,
                        at_ms: 1_000,
                    }),
                    paused: false,
                    playing: false,
                },
                pixels: Vec::new(),
            })),
        };
        let mut interaction = Interaction {
            cursor: Some((PREVIEW.x + 12, PREVIEW.y + 24)),
            ..Interaction::default()
        };
        interaction.mouse(&bridge, &snapshot, MouseButton::Left, ElementState::Pressed);
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Pointer(Some((12, 24)))
        ));
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Button(MouseButton::Left, ElementState::Pressed)
        ));
        interaction.mouse(
            &bridge,
            &snapshot,
            MouseButton::Left,
            ElementState::Released,
        );
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Button(MouseButton::Left, ElementState::Released)
        ));
        interaction.key(&bridge, VirtualKeyCode::Left, ElementState::Pressed);
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Key(VirtualKeyCode::Left, ElementState::Pressed)
        ));

        interaction.cursor = Some((TRACK.x + TRACK.w / 2, TRACK.y));
        interaction.mouse(&bridge, &snapshot, MouseButton::Left, ElementState::Pressed);
        assert!(matches!(events.recv().unwrap(), EditorEvent::ReleaseInput));
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(500))
        ));
        interaction.pointer(&bridge, Some((TRACK.x, TRACK.y)), snapshot.timeline());
        interaction.mouse(
            &bridge,
            &snapshot,
            MouseButton::Left,
            ElementState::Released,
        );
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(0))
        ));
        interaction.key(&bridge, VirtualKeyCode::Left, ElementState::Pressed);
        assert!(events.try_recv().is_err());

        interaction.cursor = Some((TRACK.x + TRACK.w, TRACK.y));
        interaction.mouse(&bridge, &snapshot, MouseButton::Left, ElementState::Pressed);
        assert!(matches!(
            events.recv().unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(u64::MAX))
        ));
    }
}
