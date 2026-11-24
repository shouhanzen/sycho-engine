//! Local input and RGBA frame transport for the overseer's embedded game.

use std::env;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use winit::event::{ElementState, MouseButton, VirtualKeyCode};

use crate::timeline::{Frame, TimelineCommand};

pub const ADDRESS_ENV: &str = "SYCHO_EMBEDDED_ADDRESS";
pub const PREVIEW_WIDTH: u32 = 960;
pub const PREVIEW_HEIGHT: u32 = 640;
pub const PREVIEW_BYTES: usize = (PREVIEW_WIDTH * PREVIEW_HEIGHT * 4) as usize;
const MAX_MESSAGE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EditorEvent {
    Key(VirtualKeyCode, ElementState),
    Button(MouseButton, ElementState),
    Pointer(Option<(u32, u32)>),
    Scroll(f32, f32),
    Focus(bool),
    ReleaseInput,
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
    pub status: TimelineStatus,
    pub pixels: Vec<u8>,
}

impl PreviewFrame {
    pub fn read(stream: &mut impl Read) -> io::Result<Self> {
        let status = read_message(stream)?;
        let mut pixels = vec![0; PREVIEW_BYTES];
        stream.read_exact(&mut pixels)?;
        Ok(Self { status, pixels })
    }

    fn write(&self, stream: &mut impl Write) -> io::Result<()> {
        write_message(stream, &self.status)?;
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
    recycled: Receiver<Vec<u8>>,
}

#[derive(Default)]
struct Outbox {
    frame: Option<PreviewFrame>,
    closed: bool,
}

impl GameLink {
    pub fn connect() -> io::Result<Option<Self>> {
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
        let mut reader = stream.try_clone()?;
        let (events_tx, events) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(event) = read_message(&mut reader) {
                if events_tx.send(event).is_err() {
                    break;
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
                if frame.write(&mut writer).is_err() {
                    let _ = writer.shutdown(Shutdown::Both);
                    break;
                }
                let _ = recycle.try_send(frame.pixels);
            }
        });
        Ok(Some(Self {
            stream,
            events,
            frames,
            recycled,
        }))
    }

    pub fn publish(&self, status: TimelineStatus, buffer: &mut Vec<u8>) {
        let (outbox, changed) = &*self.frames;
        let mut slot = outbox.lock().expect("preview outbox");
        let frame = PreviewFrame {
            status,
            pixels: std::mem::take(buffer),
        };
        let old = slot.frame.replace(frame);
        changed.notify_one();
        drop(slot);
        *buffer = old
            .map(|frame| frame.pixels)
            .or_else(|| self.recycled.try_recv().ok())
            .unwrap_or_else(|| vec![0; PREVIEW_BYTES]);
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
        let rgba = vec![0xA7; PREVIEW_BYTES];
        PreviewFrame {
            status,
            pixels: rgba.clone(),
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
        assert_eq!(frame.status, status);
        assert_eq!(frame.pixels, rgba);
        assert!(matches!(
            read_message(&mut stream).unwrap(),
            EditorEvent::Timeline(TimelineCommand::Seek(900))
        ));
    }
}
