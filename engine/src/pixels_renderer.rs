use crate::graphics::{CpuRenderer, GpuRenderer2d, Renderer2d};
use crate::surface::SurfaceSize;

use pixels::Pixels;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderBackend2d {
    Cpu,
    Gpu,
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|v| match v.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

/// Headful renderer built on `pixels`, with a pluggable CPU/GPU 2D backend.
///
/// The rest of the game should be renderer-agnostic: it draws via `Renderer2d`, and this type
/// handles the details of presenting (including keeping the pixel buffer tiny in GPU mode).
pub struct PixelsRenderer2d {
    pixels: Pixels,
    size: SurfaceSize,
    backend: RenderBackend2d,
    gpu: Option<GpuRenderer2d>,
}

impl PixelsRenderer2d {
    /// Picks a backend once at startup, based on environment.
    ///
    /// - `ROLLOUT_HEADFUL_GPU=0` forces CPU rendering.
    /// - Any other value (or unset) defaults to GPU rendering.
    pub fn new_auto(pixels: Pixels, size: SurfaceSize) -> Result<Self, pixels::Error> {
        let gpu_enabled = env_bool("ROLLOUT_HEADFUL_GPU").unwrap_or(true);
        let backend = if gpu_enabled {
            RenderBackend2d::Gpu
        } else {
            RenderBackend2d::Cpu
        };
        Self::new(pixels, size, backend)
    }

    pub fn new(
        mut pixels: Pixels,
        size: SurfaceSize,
        backend: RenderBackend2d,
    ) -> Result<Self, pixels::Error> {
        let gpu = match backend {
            RenderBackend2d::Cpu => {
                pixels.resize_buffer(size.width, size.height)?;
                None
            }
            RenderBackend2d::Gpu => {
                pixels.resize_buffer(1, 1)?;
                let gpu =
                    GpuRenderer2d::new(&pixels.context().device, pixels.surface_texture_format());
                Some(gpu)
            }
        };

        Ok(Self {
            pixels,
            size,
            backend,
            gpu,
        })
    }

    pub fn size(&self) -> SurfaceSize {
        self.size
    }

    pub fn pixels(&self) -> &Pixels {
        &self.pixels
    }

    pub fn pixels_mut(&mut self) -> &mut Pixels {
        &mut self.pixels
    }

    pub fn resize(&mut self, size: SurfaceSize) -> Result<(), pixels::Error> {
        self.size = size;
        self.pixels.resize_surface(size.width, size.height)?;

        match self.backend {
            RenderBackend2d::Cpu => {
                self.pixels.resize_buffer(size.width, size.height)?;
            }
            RenderBackend2d::Gpu => {
                // Keep the buffer tiny to avoid full-frame uploads.
                self.pixels.resize_buffer(1, 1)?;
            }
        }

        Ok(())
    }

    pub fn draw_frame<F, R>(&mut self, f: F) -> Result<R, pixels::Error>
    where
        F: FnOnce(&mut dyn Renderer2d) -> R,
    {
        match self.backend {
            RenderBackend2d::Cpu => {
                let mut cpu = CpuRenderer::new(self.pixels.frame_mut(), self.size);
                cpu.begin_frame(self.size);
                Ok(f(&mut cpu))
            }
            RenderBackend2d::Gpu => {
                let gpu = self
                    .gpu
                    .as_mut()
                    .expect("RenderBackend2d::Gpu requires gpu renderer to be initialized");
                gpu.begin_frame(self.size);
                Ok(f(gpu))
            }
        }
    }

    pub fn present(&mut self) -> Result<(), pixels::Error> {
        match self.backend {
            RenderBackend2d::Cpu => self.pixels.render(),
            RenderBackend2d::Gpu => {
                let mut gpu = self
                    .gpu
                    .take()
                    .expect("RenderBackend2d::Gpu requires gpu renderer to be initialized");
                let res = self.pixels.render_with(|encoder, render_target, ctx| {
                    gpu.render(encoder, render_target, ctx);
                    Ok(())
                });
                self.gpu = Some(gpu);
                res
            }
        }
    }
}
