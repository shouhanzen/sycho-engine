use std::error::Error;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use pixels::wgpu;

use crate::embedded::{DEFAULT_PREVIEW_SIZE, FramePublisher, PreviewMetadata};
use crate::gpu_timer::{GpuTimer, TimestampReadback};
use crate::graphics::{GpuRenderer2d, Renderer2d};
use crate::profiling::{FrameOutput, FrameTimings, PreviewProfile, Span, micros};
use crate::surface::SurfaceSize;
#[cfg(target_os = "macos")]
use crate::{
    embedded::SurfaceLease,
    shared_surface::{self, Surface, SurfaceLeases},
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
fn extent(size: SurfaceSize) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width: size.width,
        height: size.height,
        depth_or_array_layers: 1,
    }
}

fn row_stride(size: SurfaceSize) -> u32 {
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (size.width * 4).div_ceil(alignment) * alignment
}

fn target(device: &wgpu::Device, size: SurfaceSize) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("embedded preview render target"),
        size: extent(size),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn staging_buffer(device: &wgpu::Device, size: SurfaceSize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("embedded preview staging buffer"),
        size: u64::from(row_stride(size)) * u64::from(size.height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

enum FrameStorage {
    Pixels(wgpu::Buffer),
    #[cfg(target_os = "macos")]
    Surface(SurfaceLease),
}

struct CompletedFrame {
    storage: FrameStorage,
    submission: wgpu::SubmissionIndex,
    size: SurfaceSize,
    metadata: PreviewMetadata,
    profile: Option<PreviewProfile>,
    submitted_at: Instant,
    timestamps: Option<TimestampReadback>,
}

pub(crate) struct PreviewWorker<S> {
    work: Arc<(Mutex<PendingPreview<S>>, Condvar)>,
    feedback: Receiver<(SurfaceSize, FrameTimings)>,
}

struct PendingPreview<S> {
    frame: Option<RenderRequest<S>>,
    closed: bool,
}

struct RenderRequest<S> {
    size: SurfaceSize,
    metadata: PreviewMetadata,
    snapshot: S,
    update: Duration,
    profile: Option<(PreviewProfile, Instant)>,
}

impl<S> PreviewWorker<S> {
    pub fn publish(
        &self,
        size: SurfaceSize,
        metadata: PreviewMetadata,
        snapshot: S,
        update: Duration,
        profile: Option<PreviewProfile>,
    ) {
        let (pending, changed) = &*self.work;
        let old = pending
            .lock()
            .expect("preview work")
            .frame
            .replace(RenderRequest {
                size,
                metadata,
                snapshot,
                update,
                profile: profile.map(|timing| (timing, Instant::now())),
            });
        changed.notify_one();
        drop(old);
    }

    pub fn take_feedback(&self) -> io::Result<Option<(SurfaceSize, FrameTimings)>> {
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

impl<S> Drop for PreviewWorker<S> {
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
    gpu_timer: Option<GpuTimer>,
    size: SurfaceSize,
    texture: wgpu::Texture,
    view: Arc<wgpu::TextureView>,
    ready: Receiver<wgpu::Buffer>,
    completed: Sender<CompletedFrame>,
    #[cfg(target_os = "macos")]
    shared: Option<SharedTargets>,
}

impl GpuPreview {
    pub fn new(publisher: FramePublisher) -> Result<Self, Box<dyn Error>> {
        let backends = wgpu::util::backend_bits_from_env().unwrap_or_else(wgpu::Backends::all);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = None;
        #[cfg(target_os = "macos")]
        let adapter = if backends.contains(wgpu::Backends::METAL) {
            publisher
                .shared_output()
                .and_then(|(device, _)| shared_surface::adapter_for_device(&instance, device))
        } else {
            adapter
        };
        let adapter = adapter
            .or_else(|| wgpu::util::initialize_adapter_from_env(&instance, backends))
            .or_else(|| {
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                }))
            })
            .ok_or_else(|| io::Error::other("no GPU adapter available for offscreen rendering"))?;
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return Err(io::Error::other("only a software GPU adapter is available").into());
        }
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("embedded preview device"),
                features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
                limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            },
            None,
        ))?;
        let size = DEFAULT_PREVIEW_SIZE;
        #[cfg(target_os = "macos")]
        let shared = publisher
            .shared_output()
            .and_then(|(editor_device, leases)| {
                if shared_surface::device_id(&device) != Some(editor_device) {
                    return None;
                }
                match SharedTargets::new(&device, size, leases) {
                    Ok(shared) => Some(shared),
                    Err(err) => {
                        eprintln!("could not create shared preview surfaces, using RGBA: {err}");
                        None
                    }
                }
            });
        let format = FORMAT;
        #[cfg(target_os = "macos")]
        let format = if shared.is_some() {
            shared_surface::FORMAT
        } else {
            format
        };
        let renderer = GpuRenderer2d::new(&device, format);
        let texture = target(&device, size);
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        let gpu_timer = GpuTimer::new(&device, &queue);
        let device = Arc::new(device);
        let (recycle, ready) = mpsc::channel();
        for _ in 0..2 {
            recycle.send(staging_buffer(&device, size))?;
        }
        let (completed, requests) = mpsc::channel();
        let worker_device = device.clone();
        thread::Builder::new()
            .name("preview GPU completion".into())
            .spawn(move || {
                if let Err(err) = complete_frames(&worker_device, requests, recycle, publisher) {
                    eprintln!("could not read the editor GPU preview: {err}");
                }
            })?;

        Ok(Self {
            device,
            queue,
            renderer,
            gpu_timer,
            size,
            texture,
            view,
            ready,
            completed,
            #[cfg(target_os = "macos")]
            shared,
        })
    }

    pub fn spawn<S: Send + 'static>(
        mut self,
        draw: fn(S, &mut dyn Renderer2d),
    ) -> io::Result<PreviewWorker<S>> {
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
                let mut last_frame = Instant::now();
                loop {
                    let RenderRequest {
                        size,
                        metadata,
                        snapshot,
                        update,
                        profile,
                    } = {
                        let pending = pending.lock().expect("preview work");
                        let mut pending = changed
                            .wait_while(pending, |slot| slot.frame.is_none() && !slot.closed)
                            .expect("preview work");
                        if pending.closed {
                            return;
                        }
                        pending.frame.take().expect("pending preview frame")
                    };
                    let profile = profile.map(|(mut timing, queued)| {
                        timing.queue_us = micros(queued.elapsed());
                        timing
                    });
                    match self.draw(size, metadata, update, profile, |renderer| {
                        draw(snapshot, renderer)
                    }) {
                        Ok(Some(mut timings)) => {
                            let now = Instant::now();
                            timings.interval = now.duration_since(last_frame);
                            last_frame = now;
                            if complete.send((size, timings)).is_err() {
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

    fn acquire_target(
        &mut self,
        size: SurfaceSize,
    ) -> io::Result<Option<(Arc<wgpu::TextureView>, FrameStorage)>> {
        #[cfg(target_os = "macos")]
        if let Some(shared) = &mut self.shared {
            if shared.leases.enabled() {
                match shared.acquire(&self.device, size) {
                    Ok(target) => return Ok(target),
                    Err(err) => {
                        eprintln!("could not resize shared preview surfaces, using RGBA: {err}")
                    }
                }
            }
            shared.leases.disable();
            self.shared = None;
            self.renderer = GpuRenderer2d::new(&self.device, FORMAT);
        }
        let mut buffer = match self.ready.try_recv() {
            Ok(buffer) => buffer,
            // Keep updating the game when the GPU hasn't finished either staging buffer.
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => return Err(stopped()),
        };
        if size != self.size {
            self.texture = target(&self.device, size);
            self.view = Arc::new(
                self.texture
                    .create_view(&wgpu::TextureViewDescriptor::default()),
            );
            self.size = size;
        }
        if buffer.size() != u64::from(row_stride(size)) * u64::from(size.height) {
            buffer = staging_buffer(&self.device, size);
        }
        Ok(Some((self.view.clone(), FrameStorage::Pixels(buffer))))
    }

    fn draw(
        &mut self,
        size: SurfaceSize,
        metadata: PreviewMetadata,
        update: Duration,
        mut profile: Option<PreviewProfile>,
        render: impl FnOnce(&mut dyn Renderer2d),
    ) -> io::Result<Option<FrameTimings>> {
        if size.width.max(size.height) > self.device.limits().max_texture_dimension_2d {
            return Err(io::Error::other(
                "game viewport exceeds the GPU texture size limit",
            ));
        }
        let Some((view, storage)) = self.acquire_target(size)? else {
            return Ok(None);
        };
        self.renderer.begin_frame(size);
        let (_, draw) = Span::measure(|| render(&mut self.renderer));
        let encode_start = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("embedded preview"),
            });
        let timestamps = profile
            .as_ref()
            .and(self.gpu_timer.as_ref())
            .and_then(|timer| timer.begin(&mut encoder));
        self.renderer
            .render(&mut encoder, &view, &self.device, &self.queue);
        if let Some((timer, readback)) = self.gpu_timer.as_ref().zip(timestamps.as_ref()) {
            timer.end(&mut encoder, readback);
        }
        match &storage {
            FrameStorage::Pixels(buffer) => encoder.copy_texture_to_buffer(
                self.texture.as_image_copy(),
                wgpu::ImageCopyBuffer {
                    buffer,
                    layout: wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(row_stride(size)),
                        rows_per_image: None,
                    },
                },
                extent(size),
            ),
            #[cfg(target_os = "macos")]
            FrameStorage::Surface(_) => {}
        }
        let submission = self.queue.submit([encoder.finish()]);
        let submitted_at = Instant::now();
        let submit = submitted_at.duration_since(encode_start);
        let timings = FrameTimings {
            interval: Duration::ZERO,
            update,
            draw: draw.elapsed,
            output: FrameOutput::Submit(submit),
        };
        if let Some(profile) = &mut profile {
            profile.update_us = micros(timings.update);
            profile.draw_us = micros(timings.draw);
            profile.submit_us = micros(timings.output.elapsed());
        }
        self.completed
            .send(CompletedFrame {
                storage,
                submission,
                size,
                metadata,
                profile,
                submitted_at,
                timestamps,
            })
            .map_err(|_| stopped())?;
        Ok(Some(timings))
    }
}

#[cfg(target_os = "macos")]
struct SharedTarget {
    id: u32,
    surface: Arc<Surface>,
    _texture: wgpu::Texture,
    view: Arc<wgpu::TextureView>,
}

#[cfg(target_os = "macos")]
struct SharedTargets {
    next_id: u32,
    size: SurfaceSize,
    slots: Vec<SharedTarget>,
    leases: Arc<SurfaceLeases>,
}

#[cfg(target_os = "macos")]
impl SharedTargets {
    fn new(
        device: &wgpu::Device,
        size: SurfaceSize,
        leases: Arc<SurfaceLeases>,
    ) -> io::Result<Self> {
        let mut pool = Self {
            next_id: 1,
            size,
            slots: Vec::with_capacity(3),
            leases,
        };
        pool.allocate(device)?;
        Ok(pool)
    }

    fn allocate(&mut self, device: &wgpu::Device) -> io::Result<()> {
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("preview surface IDs exhausted"))?;
        let surface = Arc::new(Surface::new(self.size)?);
        let texture = surface.render_texture(device, self.size)?;
        let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.slots.push(SharedTarget {
            id,
            surface,
            _texture: texture,
            view,
        });
        Ok(())
    }

    fn acquire(
        &mut self,
        device: &wgpu::Device,
        size: SurfaceSize,
    ) -> io::Result<Option<(Arc<wgpu::TextureView>, FrameStorage)>> {
        if self.size != size {
            self.slots.clear();
            self.size = size;
        }
        let slot = if let Some(index) = self
            .slots
            .iter()
            .position(|slot| self.leases.acquire(slot.id))
        {
            &self.slots[index]
        } else if self.slots.len() < 3 {
            self.allocate(device)?;
            let slot = self.slots.last().unwrap();
            self.leases.acquire(slot.id);
            slot
        } else {
            // The editor can hold its current frame and two in-flight frames.
            // Skip this render rather than blocking simulation or overwriting pixels.
            return Ok(None);
        };
        Ok(Some((
            slot.view.clone(),
            FrameStorage::Surface(SurfaceLease {
                id: slot.id,
                surface: slot.surface.clone(),
                leases: self.leases.clone(),
                sent: false,
            }),
        )))
    }
}

fn stopped() -> io::Error {
    io::Error::other("GPU preview completion worker stopped")
}

fn complete_frames(
    device: &wgpu::Device,
    requests: Receiver<CompletedFrame>,
    recycle: Sender<wgpu::Buffer>,
    mut publisher: FramePublisher,
) -> Result<(), Box<dyn Error>> {
    for frame in requests {
        let timestamp_result = frame.timestamps.map(|readback| {
            let (sender, receiver) = mpsc::channel();
            readback.map(move |elapsed| {
                let _ = sender.send(elapsed);
            });
            receiver
        });
        let finish_profile = || {
            frame.profile.map(|mut profile| {
                profile.gpu_completion_us = micros(frame.submitted_at.elapsed());
                profile.gpu_draw_us = timestamp_result.and_then(|rx| rx.try_recv().ok().flatten());
                profile
            })
        };
        match frame.storage {
            FrameStorage::Pixels(buffer) => {
                let slice = buffer.slice(..);
                let (done, mapped) = mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = done.send(result);
                });
                device.poll(wgpu::Maintain::WaitForSubmissionIndex(frame.submission));
                mapped.recv()??;
                publisher.publish(
                    frame.size,
                    frame.metadata,
                    finish_profile(),
                    &slice.get_mapped_range(),
                    row_stride(frame.size) as usize,
                );
                buffer.unmap();
                if recycle.send(buffer).is_err() {
                    break;
                }
            }
            #[cfg(target_os = "macos")]
            FrameStorage::Surface(lease) => {
                device.poll(wgpu::Maintain::WaitForSubmissionIndex(frame.submission));
                publisher.publish_surface(frame.size, frame.metadata, finish_profile(), lease);
            }
        }
    }
    Ok(())
}
