use engine::timeline::ForkState;
use std::io::{self, BufReader};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use engine::embedded::{
    DEFAULT_PREVIEW_SIZE, EditorEvent, PreviewFrame, TimelineStatus, write_message,
};
use engine::profiling::wall_micros;
#[cfg(target_os = "macos")]
use engine::shared_surface::SurfaceInbox;
use engine::surface::DisplayMetrics;
use winit::event_loop::EventLoopProxy;

use super::profiler::{Arrival, FrameKey};
use super::{UiEvent, board::WindowId};

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Building,
    Reconnecting,
    Running,
    Failed,
    GpuFailed,
    ReloadFailed,
    Exited,
    Disabling,
    Disabled,
    Enabling,
    DisableFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifecycle {
    Enabled,
    Disabling,
    Disabled,
    Enabling,
}

#[derive(Clone)]
pub struct Bridge(Arc<Mutex<Shared>>);

struct Shared {
    last_generation: u64,
    generation: Option<u64>,
    phase: Phase,
    lifecycle: Lifecycle,
    input: Option<Sender<EditorEvent>>,
    display: DisplayMetrics,
    frame: Option<Arc<PreviewFrame>>,
    frozen_state: Option<Arc<ForkState>>,
    frozen_since: Option<Instant>,
    arrival: Option<Arrival>,
    redraw: Option<(WindowId, EventLoopProxy<UiEvent>)>,
    error: Option<String>,
    redraw_pending: bool,
    profiling: bool,
    #[cfg(target_os = "macos")]
    gpu_device: Option<u64>,
}

impl Shared {
    fn request_redraw(&mut self) {
        if !self.redraw_pending
            && let Some((id, redraw)) = &self.redraw
        {
            self.redraw_pending = redraw.send_event(UiEvent::PreviewChanged(*id)).is_ok();
        }
    }
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub generation: Option<u64>,
    pub phase: Phase,
    pub connected: bool,
    pub frame: Option<Arc<PreviewFrame>>,
    pub frozen_state: Option<Arc<ForkState>>,
    pub frozen_since: Option<Instant>,
    pub arrival: Option<Arrival>,
    pub error: Option<String>,
}

impl Snapshot {
    pub(super) fn timeline(&self) -> TimelineStatus {
        let mut status = self
            .frame
            .as_ref()
            .map_or(TimelineStatus::default(), |frame| frame.status);
        if !self.connected {
            status.playing = false;
        }
        status
    }
}

impl Bridge {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Shared {
            generation: None,
            last_generation: 0,
            phase: Phase::Building,
            lifecycle: Lifecycle::Enabled,
            input: None,
            display: DisplayMetrics::at_1x(DEFAULT_PREVIEW_SIZE),
            frame: None,
            frozen_since: None,
            frozen_state: None,
            arrival: None,
            redraw: None,
            error: None,
            redraw_pending: false,
            profiling: false,
            #[cfg(target_os = "macos")]
            gpu_device: None,
        })))
    }

    pub(super) fn attach_window(
        &self,
        id: WindowId,
        redraw: EventLoopProxy<UiEvent>,
        #[cfg(target_os = "macos")] gpu_device: Option<u64>,
    ) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.redraw = Some((id, redraw));
        #[cfg(target_os = "macos")]
        {
            shared.gpu_device = gpu_device;
        }
    }

    pub(super) fn acknowledge_redraw(&self) {
        self.0.lock().expect("editor bridge").redraw_pending = false;
    }

    #[cfg(test)]
    pub(super) fn with_test_input() -> (Self, mpsc::Receiver<EditorEvent>) {
        let bridge = Self::new();
        let (sender, events) = mpsc::channel();
        bridge.0.lock().unwrap().input = Some(sender);
        (bridge, events)
    }

    pub fn set_phase(&self, phase: Phase) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.phase = phase;
        shared.error = None;
        shared.request_redraw();
    }

    pub(crate) fn fail(&self, message: String) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.generation = None;
        shared.input = None;
        shared.phase = Phase::Failed;
        shared.error = Some(message);
        shared.request_redraw();
    }

    pub fn lifecycle(&self) -> Lifecycle {
        self.0.lock().expect("editor bridge").lifecycle
    }

    pub(super) fn toggle_lifecycle(&self) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.lifecycle = match shared.lifecycle {
            Lifecycle::Enabled => Lifecycle::Disabling,
            Lifecycle::Disabled => Lifecycle::Enabling,
            Lifecycle::Disabling | Lifecycle::Enabling => return,
        };
        shared.request_redraw();
    }

    pub fn freeze_preview(&self) -> io::Result<Option<Arc<PreviewFrame>>> {
        let frame = self.0.lock().expect("editor bridge").frame.clone();
        frame.map(|frame| frame.freeze()).transpose()
    }

    pub fn finish_disable(&self, frame: Option<Arc<PreviewFrame>>, state: Option<ForkState>) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.lifecycle = Lifecycle::Disabled;
        shared.generation = None;
        shared.input = None;
        shared.frozen_state = state.map(Arc::new);
        shared.frozen_since = frame.as_ref().map(|_| Instant::now());
        shared.frame = frame;
        shared.arrival = None;
        shared.request_redraw();
    }

    pub fn reject_disable(&self) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.lifecycle = Lifecycle::Enabled;
        shared.phase = Phase::DisableFailed;
        if let Some(input) = &shared.input {
            let _ = input.send(EditorEvent::Resize(shared.display));
            let _ = input.send(EditorEvent::Profile(shared.profiling));
        }
        shared.request_redraw();
    }

    pub fn finish_enable(&self) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.lifecycle = Lifecycle::Enabled;
        shared.phase = Phase::Building;
        shared.error = None;
        shared.request_redraw();
    }

    pub fn disconnect(&self, phase: Phase) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.generation = None;
        shared.phase = phase;
        shared.input = None;
        shared.request_redraw();
    }

    pub(crate) fn last_generation(&self) -> u64 {
        self.0.lock().expect("editor bridge").last_generation
    }

    pub fn endpoint(
        &self,
        generation: u64,
    ) -> io::Result<(SocketAddr, Option<String>, DisplayMetrics)> {
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
        let display = {
            let mut shared = self.0.lock().expect("editor bridge");
            shared.generation = Some(generation);
            shared.last_generation = generation;
            shared.phase = Phase::Reconnecting;
            shared.input = None;
            shared.request_redraw();
            shared.display
        };
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
        Ok((address, transport, display))
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
            if shared.lifecycle != Lifecycle::Enabled {
                continue;
            }
            if let Some(input) = input.take() {
                if frame.display != Some(shared.display) {
                    let _ = input.send(EditorEvent::Resize(shared.display));
                }
                if shared.profiling {
                    let _ = input.send(EditorEvent::Profile(true));
                }
                shared.input = Some(input);
                if shared.phase == Phase::Reconnecting {
                    shared.phase = Phase::Running;
                }
            }
            shared.arrival = frame.profile.map(|profile| Arrival {
                key: FrameKey {
                    game: shared.redraw.as_ref().map_or(0, |(id, _)| *id),
                    generation,
                    frame: profile.frame,
                },
                at: Instant::now(),
                transport_us: wall_micros().saturating_sub(profile.published_us),
            });
            shared.frame = Some(Arc::new(frame));
            shared.frozen_since = None;
            shared.frozen_state = None;
            shared.request_redraw();
        }
        self.disconnect_generation(generation);
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        let shared = self.0.lock().expect("editor bridge");
        Snapshot {
            generation: shared.generation,
            phase: match shared.lifecycle {
                Lifecycle::Enabled => shared.phase,
                Lifecycle::Disabling => Phase::Disabling,
                Lifecycle::Disabled => Phase::Disabled,
                Lifecycle::Enabling => Phase::Enabling,
            },
            connected: shared.lifecycle == Lifecycle::Enabled && shared.input.is_some(),
            frame: shared.frame.clone(),
            error: shared.error.clone(),
            frozen_since: shared.frozen_since,
            frozen_state: shared.frozen_state.clone(),
            arrival: shared.arrival.filter(|arrival| {
                shared.lifecycle == Lifecycle::Enabled
                    && Some(arrival.key.generation) == shared.generation
            }),
        }
    }

    pub(super) fn send(&self, event: EditorEvent) {
        let shared = self.0.lock().expect("editor bridge");
        if shared.lifecycle == Lifecycle::Enabled
            && let Some(input) = &shared.input
        {
            let _ = input.send(event);
        }
    }

    pub fn capture_state(&self, stamp: u64) -> io::Result<ForkState> {
        let staging = crate::Staging::new()?;
        let reply = staging.0.join("fork-state.json");
        {
            let shared = self.0.lock().expect("editor bridge");
            shared
                .input
                .as_ref()
                .ok_or_else(|| {
                    io::Error::other("the game disconnected before providing its preview")
                })?
                .send(EditorEvent::ForkPreview {
                    stamp,
                    reply: reply.clone(),
                })
                .map_err(|_| {
                    io::Error::other("the game disconnected before providing its preview")
                })?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match std::fs::File::open(&reply) {
                Ok(file) => {
                    let result: Result<ForkState, String> =
                        serde_json::from_reader(BufReader::new(file))?;
                    return result.map_err(io::Error::other);
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "the game did not return the preview state",
                    ));
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub(super) fn set_profiling(&self, enabled: bool) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.profiling != enabled {
            shared.profiling = enabled;
            if shared.lifecycle == Lifecycle::Enabled
                && let Some(input) = &shared.input
            {
                let _ = input.send(EditorEvent::Profile(enabled));
            }
        }
    }

    pub(super) fn resize_game(&self, display: DisplayMetrics) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.display != display {
            shared.display = display;
            if shared.lifecycle == Lifecycle::Enabled
                && let Some(input) = &shared.input
            {
                let _ = input.send(EditorEvent::Resize(display));
            }
        }
    }
}
