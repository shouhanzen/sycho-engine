//! Local input and frame transport for the overseer's embedded game.

use std::env;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use winit::event::{ElementState, MouseButton, VirtualKeyCode};

#[cfg(target_os = "macos")]
use crate::shared_surface::{Surface, SurfaceInbox, SurfaceLeases, SurfaceSender};
use crate::surface::SurfaceSize;
use crate::timeline::{Frame, TimelineCommand};

pub const SURFACE_ENV: &str = "SYCHO_PREVIEW_SURFACE";
pub const ADDRESS_ENV: &str = "SYCHO_EMBEDDED_ADDRESS";
pub const GPU_PREVIEW_EXIT_CODE: i32 = 76;
pub const DEFAULT_PREVIEW_SIZE: SurfaceSize = SurfaceSize::new(960, 640);
pub const MAX_PREVIEW_DIMENSION: u32 = 4096;
const MAX_MESSAGE_BYTES: usize = 4096;

pub fn valid_preview_size(size: SurfaceSize) -> bool {
    !size.is_empty() && size.width <= MAX_PREVIEW_DIMENSION && size.height <= MAX_PREVIEW_DIMENSION
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EditorEvent {
    Key(VirtualKeyCode, ElementState),
    Button(MouseButton, ElementState),
    Pointer(Option<(u32, u32)>),
    Scroll(f32, f32),
    Focus(bool),
    ReleaseInput,
    Resize(SurfaceSize),
    ReleaseSurface(u32),
    UseRgba,
    Timeline(TimelineCommand),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineStatus {
    pub cursor: Option<Frame>,
    pub tip: Option<Frame>,
    pub paused: bool,
    pub playing: bool,
}

pub struct PreviewFrame {
    pub size: SurfaceSize,
    pub status: TimelineStatus,
    pub pixels: Vec<u8>,
    #[cfg(target_os = "macos")]
    pub surface: Option<PreviewSurface>,
}

#[cfg(target_os = "macos")]
pub struct PreviewSurface {
    pub id: u32,
    pub surface: Arc<Surface>,
    release: mpsc::Sender<EditorEvent>,
}

#[cfg(target_os = "macos")]
impl PreviewSurface {
    pub fn use_rgba(&self) {
        let _ = self.release.send(EditorEvent::UseRgba);
    }
}

#[cfg(target_os = "macos")]
impl Drop for PreviewSurface {
    fn drop(&mut self) {
        let _ = self.release.send(EditorEvent::ReleaseSurface(self.id));
    }
}

#[derive(Serialize, Deserialize)]
struct FrameHeader {
    size: SurfaceSize,
    status: TimelineStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared: Option<u32>,
}

impl FrameHeader {
    fn read(stream: &mut impl Read) -> io::Result<Self> {
        let header: Self = read_message(stream)?;
        if !valid_preview_size(header.size) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid game preview size",
            ));
        }
        Ok(header)
    }
}

impl PreviewFrame {
    pub fn read(stream: &mut impl Read) -> io::Result<Self> {
        let header = FrameHeader::read(stream)?;
        Self::read_pixels(header, stream)
    }

    #[cfg(target_os = "macos")]
    pub fn read_with_surfaces(
        stream: &mut impl Read,
        inbox: Option<&SurfaceInbox>,
        release: &mpsc::Sender<EditorEvent>,
    ) -> io::Result<Option<Self>> {
        let header = FrameHeader::read(stream)?;
        let Some(id) = header.shared else {
            return Self::read_pixels(header, stream).map(Some);
        };
        let inbox = inbox.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "unexpected shared preview")
        })?;
        let surface = match inbox.receive(id, header.size) {
            Ok(surface) => surface,
            Err(err) => {
                eprintln!("editor could not receive the shared preview, using RGBA: {err}");
                let _ = release.send(EditorEvent::UseRgba);
                let _ = release.send(EditorEvent::ReleaseSurface(id));
                return Ok(None);
            }
        };
        Ok(Some(Self {
            size: header.size,
            status: header.status,
            pixels: Vec::new(),
            surface: Some(PreviewSurface {
                id,
                surface: Arc::new(surface),
                release: release.clone(),
            }),
        }))
    }

    fn read_pixels(header: FrameHeader, stream: &mut impl Read) -> io::Result<Self> {
        if header.shared.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared preview is unavailable",
            ));
        }
        let mut pixels = vec![0; header.size.rgba_len()];
        stream.read_exact(&mut pixels)?;
        Ok(Self {
            size: header.size,
            status: header.status,
            pixels,
            #[cfg(target_os = "macos")]
            surface: None,
        })
    }

    fn write(&self, stream: &mut impl Write) -> io::Result<()> {
        write_message(
            stream,
            &FrameHeader {
                size: self.size,
                status: self.status,
                shared: None,
            },
        )?;
        stream.write_all(&self.pixels)
    }
}

pub fn write_message(stream: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "editor message is too large",
        ));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}

pub fn read_message<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "editor message is too large",
        ));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) struct GameLink {
    stream: TcpStream,
    pub events: Receiver<EditorEvent>,
    frames: Arc<(Mutex<Outbox>, Condvar)>,
}

pub(crate) struct FramePublisher {
    frames: Arc<(Mutex<Outbox>, Condvar)>,
    recycled: Receiver<Vec<u8>>,
    pixels: Vec<u8>,
    #[cfg(target_os = "macos")]
    shared: Option<(u64, Arc<SurfaceLeases>)>,
}

enum OutgoingFrame {
    Pixels(PreviewFrame),
    #[cfg(target_os = "macos")]
    Surface(FrameHeader, SurfaceLease),
}

/// The producer owns the lease until a frame is sent. From that point the
/// editor releases it after the last GPU submission sampling that frame.
#[cfg(target_os = "macos")]
pub(crate) struct SurfaceLease {
    pub id: u32,
    pub surface: Arc<Surface>,
    pub leases: Arc<SurfaceLeases>,
    pub sent: bool,
}

#[cfg(target_os = "macos")]
impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if !self.sent {
            self.leases.release(self.id);
        }
    }
}

#[derive(Default)]
struct Outbox {
    frame: Option<OutgoingFrame>,
    closed: bool,
}

impl GameLink {
    pub fn connect() -> io::Result<Option<(Self, FramePublisher)>> {
        let Some(address) = env::var_os(ADDRESS_ENV) else {
            return Ok(None);
        };
        let address: SocketAddr = address
            .to_string_lossy()
            .parse()
            .map_err(io::Error::other)?;
        if !address.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the editor address must be local",
            ));
        }
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
        stream.set_nodelay(true)?;
        #[cfg(target_os = "macos")]
        let (surface_sender, shared) = match env::var(SURFACE_ENV)
            .ok()
            .map(|endpoint| SurfaceSender::open(&endpoint))
        {
            Some(Ok(sender)) => {
                let shared = Some((sender.device, Arc::new(SurfaceLeases::default())));
                (Some(sender), shared)
            }
            Some(Err(err)) => {
                eprintln!("could not connect shared preview surfaces, using RGBA: {err}");
                (None, None)
            }
            None => (None, None),
        };
        #[cfg(target_os = "macos")]
        let release = shared.as_ref().map(|(_, leases)| leases.clone());
        let mut reader = stream.try_clone()?;
        let (events_tx, events) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(event) = read_message(&mut reader) {
                match event {
                    EditorEvent::ReleaseSurface(_id) =>
                    {
                        #[cfg(target_os = "macos")]
                        if let Some(leases) = &release {
                            leases.release(_id);
                        }
                    }
                    EditorEvent::UseRgba =>
                    {
                        #[cfg(target_os = "macos")]
                        if let Some(leases) = &release {
                            leases.disable();
                        }
                    }
                    event => {
                        if events_tx.send(event).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut writer = stream.try_clone()?;
        writer.set_write_timeout(Some(Duration::from_secs(3)))?;
        let frames = Arc::new((Mutex::new(Outbox::default()), Condvar::new()));
        let outgoing = frames.clone();
        let (recycle, recycled) = mpsc::sync_channel(2);
        thread::spawn(move || {
            let (outbox, changed) = &*outgoing;
            loop {
                let frame = {
                    let slot = outbox.lock().expect("preview outbox");
                    let mut slot = changed
                        .wait_while(slot, |slot| slot.frame.is_none() && !slot.closed)
                        .expect("preview outbox");
                    if slot.closed {
                        break;
                    }
                    slot.frame.take().expect("pending preview frame")
                };
                let written = match frame {
                    OutgoingFrame::Pixels(frame) => {
                        let result = frame.write(&mut writer);
                        let _ = recycle.try_send(frame.pixels);
                        result
                    }
                    #[cfg(target_os = "macos")]
                    OutgoingFrame::Surface(header, mut lease) => {
                        if !lease.leases.enabled() {
                            continue;
                        }
                        let sender = surface_sender.as_ref().expect("shared surface endpoint");
                        // Notify the reader first so it can enter the Mach receive even
                        // when the bootstrap service has no space to queue a message.
                        let mut result = write_message(&mut writer, &header);
                        if result.is_ok() {
                            match sender.send(lease.id, &lease.surface) {
                                Ok(()) => lease.sent = true,
                                Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {
                                    result = Err(err)
                                }
                                Err(err) => {
                                    eprintln!("shared preview transport failed, using RGBA: {err}");
                                    lease.leases.disable();
                                }
                            }
                        }
                        result
                    }
                };
                if written.is_err() {
                    let _ = writer.shutdown(Shutdown::Both);
                    break;
                }
            }
        });
        Ok(Some((
            Self {
                stream,
                events,
                frames: frames.clone(),
            },
            FramePublisher {
                frames,
                recycled,
                pixels: Vec::new(),
                #[cfg(target_os = "macos")]
                shared,
            },
        )))
    }
}

impl FramePublisher {
    pub fn publish(
        &mut self,
        size: SurfaceSize,
        status: TimelineStatus,
        rgba: &[u8],
        stride: usize,
    ) {
        self.pixels.resize(size.rgba_len(), 0);
        let row_bytes = size.width as usize * 4;
        if stride == row_bytes {
            self.pixels.copy_from_slice(rgba);
        } else {
            for (dst, src) in self
                .pixels
                .chunks_exact_mut(row_bytes)
                .zip(rgba.chunks_exact(stride))
            {
                dst.copy_from_slice(&src[..row_bytes]);
            }
        }
        let frame = PreviewFrame {
            size,
            status,
            pixels: std::mem::take(&mut self.pixels),
            #[cfg(target_os = "macos")]
            surface: None,
        };
        let old = self.send(OutgoingFrame::Pixels(frame));
        self.pixels = match old {
            Some(OutgoingFrame::Pixels(frame)) => Some(frame.pixels),
            _ => None,
        }
        .or_else(|| self.recycled.try_recv().ok())
        .unwrap_or_default();
    }

    #[cfg(target_os = "macos")]
    pub fn shared_output(&self) -> Option<(u64, Arc<SurfaceLeases>)> {
        self.shared.clone()
    }

    #[cfg(target_os = "macos")]
    pub fn publish_surface(
        &mut self,
        size: SurfaceSize,
        status: TimelineStatus,
        lease: SurfaceLease,
    ) {
        self.send(OutgoingFrame::Surface(
            FrameHeader {
                size,
                status,
                shared: Some(lease.id),
            },
            lease,
        ));
    }

    fn send(&self, frame: OutgoingFrame) -> Option<OutgoingFrame> {
        let (outbox, changed) = &*self.frames;
        let old = outbox.lock().expect("preview outbox").frame.replace(frame);
        changed.notify_one();
        old
    }
}

impl Drop for GameLink {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        let (outbox, changed) = &*self.frames;
        outbox.lock().expect("preview outbox").closed = true;
        changed.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_preview_keeps_pixels_separate_from_timeline_metadata() {
        let mut bytes = Vec::new();
        let status = TimelineStatus {
            cursor: Some(Frame { id: 2, at_ms: 400 }),
            tip: Some(Frame { id: 5, at_ms: 1000 }),
            paused: true,
            playing: false,
        };
        let size = SurfaceSize::new(257, 185);
        let rgba = vec![0xA7; size.rgba_len()];
        PreviewFrame {
            size,
            status,
            pixels: rgba.clone(),
            #[cfg(target_os = "macos")]
            surface: None,
        }
        .write(&mut bytes)
        .unwrap();
        write_message(
            &mut bytes,
            &EditorEvent::Timeline(TimelineCommand::Seek(900)),
        )
        .unwrap();
        let mut stream = bytes.as_slice();
        let frame = PreviewFrame::read(&mut stream).unwrap();
        assert_eq!(frame.size, size);
        assert_eq!(frame.status, status);
        assert_eq!(frame.pixels, rgba);
        assert!(matches!(
            read_message(&mut stream).unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(900))
        ));
    }
}
