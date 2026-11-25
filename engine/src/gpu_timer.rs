use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use pixels::wgpu;

const QUERY_BYTES: wgpu::BufferAddress = 16;

pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    available: Receiver<Arc<wgpu::Buffer>>,
    recycle: Sender<Arc<wgpu::Buffer>>,
    timestamp_period: f32,
}

pub struct TimestampReadback {
    buffer: Arc<wgpu::Buffer>,
    recycle: Sender<Arc<wgpu::Buffer>>,
    timestamp_period: f32,
}

impl GpuTimer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        let timestamp_period = queue.get_timestamp_period();
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) || timestamp_period <= 0.0 {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("profiler timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profiler timestamp resolve"),
            size: QUERY_BYTES,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let (recycle, available) = mpsc::channel();
        for _ in 0..3 {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("profiler timestamp readback"),
                size: QUERY_BYTES,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            recycle
                .send(Arc::new(buffer))
                .expect("profiler buffer pool");
        }
        Some(Self {
            queries,
            resolve,
            available,
            recycle,
            timestamp_period,
        })
    }

    pub fn begin(&self, encoder: &mut wgpu::CommandEncoder) -> Option<TimestampReadback> {
        let buffer = self.available.try_recv().ok()?;
        encoder.write_timestamp(&self.queries, 0);
        Some(TimestampReadback {
            buffer,
            recycle: self.recycle.clone(),
            timestamp_period: self.timestamp_period,
        })
    }

    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, readback: &TimestampReadback) {
        encoder.write_timestamp(&self.queries, 1);
        encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(&self.resolve, 0, &readback.buffer, 0, QUERY_BYTES);
    }
}

impl TimestampReadback {
    pub fn map(self, completed: impl FnOnce(Option<u64>) + Send + 'static) {
        let mapping = self.buffer.clone();
        mapping
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let time_us = if result.is_ok() {
                    let bytes = self.buffer.slice(..).get_mapped_range();
                    let start = u64::from_le_bytes(bytes[..8].try_into().unwrap());
                    let end = u64::from_le_bytes(bytes[8..].try_into().unwrap());
                    drop(bytes);
                    self.buffer.unmap();
                    end.checked_sub(start).map(|ticks| {
                        (ticks as f64 * f64::from(self.timestamp_period) / 1_000.0).round() as u64
                    })
                } else {
                    None
                };
                let _ = self.recycle.send(self.buffer);
                completed(time_us);
            });
    }
}
