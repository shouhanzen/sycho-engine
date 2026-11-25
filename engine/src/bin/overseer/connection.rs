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
use engine::surface::SurfaceSize;
use winit::event_loop::EventLoopProxy;

use super::UiEvent;
use super::profiler::{Arrival, FrameKey};

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
}

#[derive(Clone)]
pub struct Bridge(Arc<Mutex<Shared>>);

struct Shared {
    generation: Option<u64>,
    phase: Phase,
    input: Option<Sender<EditorEvent>>,
    game_size: SurfaceSize,
    frame: Option<Arc<PreviewFrame>>,
    arrival: Option<Arrival>,
    redraw: Option<EventLoopProxy<UiEvent>>,
    redraw_pending: bool,
    profiling: bool,
    #[cfg(target_os = "macos")]
    gpu_device: Option<u64>,
}

impl Shared {
    fn request_redraw(&mut self) {
        if !self.redraw_pending
            && let Some(redraw) = &self.redraw
        {
            self.redraw_pending = redraw.send_event(UiEvent::PreviewChanged).is_ok();
        }
    }
}

#[derive(Clone)]
pub(super) struct Snapshot {
    pub generation: Option<u64>,
    pub phase: Phase,
    pub connected: bool,
    pub frame: Option<Arc<PreviewFrame>>,
    pub arrival: Option<Arrival>,
}

impl Snapshot {
    pub(super) fn timeline(&self) -> TimelineStatus {
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
            arrival: None,
            redraw: None,
            redraw_pending: false,
            profiling: false,
            #[cfg(target_os = "macos")]
            gpu_device: None,
        })))
    }

    pub(super) fn attach_window(
        &self,
        redraw: EventLoopProxy<UiEvent>,
        #[cfg(target_os = "macos")] gpu_device: Option<u64>,
    ) {
        let mut shared = self.0.lock().expect("editor bridge");
        shared.redraw = Some(redraw);
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
                    generation,
                    frame: profile.frame,
                },
                at: Instant::now(),
                transport_us: wall_micros().saturating_sub(profile.published_us),
            });
            shared.frame = Some(Arc::new(frame));
            shared.request_redraw();
        }
        self.disconnect_generation(generation);
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        let shared = self.0.lock().expect("editor bridge");
        Snapshot {
            generation: shared.generation,
            phase: shared.phase,
            connected: shared.input.is_some(),
            frame: shared.frame.clone(),
            arrival: shared
                .arrival
                .filter(|arrival| Some(arrival.key.generation) == shared.generation),
        }
    }

    pub(super) fn send(&self, event: EditorEvent) {
        if let Some(input) = &self.0.lock().expect("editor bridge").input {
            let _ = input.send(event);
        }
    }

    pub(super) fn set_profiling(&self, enabled: bool) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.profiling != enabled {
            shared.profiling = enabled;
            if let Some(input) = &shared.input {
                let _ = input.send(EditorEvent::Profile(enabled));
            }
        }
    }

    pub(super) fn resize_game(&self, size: SurfaceSize) {
        let mut shared = self.0.lock().expect("editor bridge");
        if shared.game_size != size {
            shared.game_size = size;
            if let Some(input) = &shared.input {
                let _ = input.send(EditorEvent::Resize(size));
            }
        }
    }
}
