use std::error::Error;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use pixels::wgpu;

use crate::embedded::{
    FramePublisher, PREVIEW_BYTES, PREVIEW_HEIGHT, PREVIEW_WIDTH, TimelineStatus,
};
use crate::graphics::{GpuRenderer2d, Renderer2d};
use crate::surface::SurfaceSize;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const ROW_BYTES: u32 = PREVIEW_WIDTH * 4;
const _: () = assert!(ROW_BYTES % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT == 0);
const EXTENT: wgpu::Extent3d = wgpu::Extent3d {
    width: PREVIEW_WIDTH,
    height: PREVIEW_HEIGHT,
    depth_or_array_layers: 1,
};

struct Readback {
    buffer: wgpu::Buffer,
    submission: wgpu::SubmissionIndex,
    status: TimelineStatus,
}

pub(crate) struct PreviewWorker<S, F> {
    work: Arc<(Mutex<PendingPreview<S>>, Condvar)>,
    feedback: Receiver<F>,
}

struct PendingPreview<S> {
    frame: Option<(TimelineStatus, S)>,
    closed: bool,
}

impl<S, F> PreviewWorker<S, F> {
    pub fn publish(&self, status: TimelineStatus, snapshot: S) {
        let (pending, changed) = &*self.work;
        let old = pending
            .lock()
            .expect("preview work")
            .frame
            .replace((status, snapshot));
        changed.notify_one();
        drop(old);
    }

    pub fn take_feedback(&self) -> io::Result<Option<F>> {
        let mut latest = None;
        loop {
            match self.feedback.try_recv() {
                Ok(feedback) => latest = Some(feedback),
                Err(TryRecvError::Empty) => return Ok(latest),
                Err(TryRecvError::Disconnected) => {
                    return Err(io::Error::other("preview render worker stopped"));
                }
            }
        }
    }
}

impl<S, F> Drop for PreviewWorker<S, F> {
    fn drop(&mut self) {
        let (pending, changed) = &*self.work;
        pending.lock().expect("preview work").closed = true;
        changed.notify_one();
    }
}

pub(crate) struct GpuPreview {
    device: Arc<wgpu::Device>,
    queue: wgpu::Queue,
    renderer: GpuRenderer2d,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    ready: Receiver<wgpu::Buffer>,
    readbacks: Sender<Readback>,
}

impl GpuPreview {
    pub fn new(publisher: FramePublisher) -> Result<Self, Box<dyn Error>> {
        let backends = wgpu::util::backend_bits_from_env().unwrap_or_else(wgpu::Backends::all);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| io::Error::other("no GPU adapter available for offscreen rendering"))?;
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return Err(io::Error::other("only a software GPU adapter is available").into());
        }
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("embedded preview device"),
                features: wgpu::Features::empty(),
                limits: wgpu::Limits::downlevel_defaults(),
            },
            None,
        ))?;
        let renderer = GpuRenderer2d::new(&device, FORMAT);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("embedded preview render target"),
            size: EXTENT,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let device = Arc::new(device);
        let (recycle, ready) = mpsc::channel();
        for _ in 0..2 {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("embedded preview staging buffer"),
                size: PREVIEW_BYTES as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            recycle.send(buffer)?;
        }
        let (readbacks, requests) = mpsc::channel();
        let worker_device = device.clone();
        thread::Builder::new()
            .name("preview readback".into())
            .spawn(move || {
                if let Err(err) = readback_frames(&worker_device, requests, recycle, publisher) {
                    eprintln!("could not read the editor GPU preview: {err}");
                }
            })?;

        Ok(Self {
            device,
            queue,
            renderer,
            texture,
            view,
            ready,
            readbacks,
        })
    }

    pub fn spawn<S: Send + 'static, F: Send + 'static>(
        mut self,
        draw: fn(S, &mut dyn Renderer2d) -> F,
    ) -> io::Result<PreviewWorker<S, F>> {
        let work = Arc::new((
            Mutex::new(PendingPreview {
                frame: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let worker = work.clone();
        let (complete, feedback) = mpsc::channel();
        thread::Builder::new()
            .name("preview draw".into())
            .spawn(move || {
                let (pending, changed) = &*worker;
                loop {
                    let (status, snapshot) = {
                        let pending = pending.lock().expect("preview work");
                        let mut pending = changed
                            .wait_while(pending, |slot| slot.frame.is_none() && !slot.closed)
                            .expect("preview work");
                        if pending.closed {
                            return;
                        }
                        pending.frame.take().expect("pending preview frame")
                    };
                    match self.draw(status, |renderer| draw(snapshot, renderer)) {
                        Ok(Some(feedback)) => {
                            if complete.send(feedback).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            eprintln!("could not draw the editor GPU preview: {err}");
                            return;
                        }
                    }
                }
            })?;
        Ok(PreviewWorker { work, feedback })
    }

    fn draw<F>(
        &mut self,
        status: TimelineStatus,
        render: impl FnOnce(&mut dyn Renderer2d) -> F,
    ) -> io::Result<Option<F>> {
        let buffer = match self.ready.try_recv() {
            Ok(buffer) => buffer,
            // Keep updating the game when the GPU hasn't finished either staging buffer.
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => return Err(stopped()),
        };
        self.renderer
            .begin_frame(SurfaceSize::new(PREVIEW_WIDTH, PREVIEW_HEIGHT));
        let feedback = render(&mut self.renderer);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("embedded preview"),
            });
        self.renderer
            .render(&mut encoder, &self.view, &self.device, &self.queue);
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::ImageCopyBuffer {
                buffer: &buffer,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(ROW_BYTES),
                    rows_per_image: None,
                },
            },
            EXTENT,
        );
        let submission = self.queue.submit([encoder.finish()]);
        self.readbacks
            .send(Readback {
                buffer,
                submission,
                status,
            })
            .map_err(|_| stopped())?;
        Ok(Some(feedback))
    }
}

fn stopped() -> io::Error {
    io::Error::other("GPU preview readback stopped")
}

fn readback_frames(
    device: &wgpu::Device,
    requests: Receiver<Readback>,
    recycle: Sender<wgpu::Buffer>,
    mut publisher: FramePublisher,
) -> Result<(), Box<dyn Error>> {
    for frame in requests {
        let slice = frame.buffer.slice(..);
        let (done, mapped) = mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = done.send(result);
        });
        device.poll(wgpu::Maintain::WaitForSubmissionIndex(frame.submission));
        mapped.recv()??;
        publisher.publish(frame.status, &slice.get_mapped_range());
        frame.buffer.unmap();
        if recycle.send(frame.buffer).is_err() {
            break;
        }
    }
    Ok(())
}
