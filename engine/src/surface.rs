use std::convert::Infallible;

use serde::{Deserialize, Serialize};

use crate::ui::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSize {
    pub width: u32,
    pub height: u32,
}

impl SurfaceSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn rgba_len(self) -> usize {
        (self.width as usize)
            .saturating_mul(self.height as usize)
            .saturating_mul(4)
    }

    pub fn to_logical(self, scale_factor: f64) -> Self {
        let axis = |pixels| (f64::from(pixels) / scale_factor).round().max(1.0) as u32;
        Self::new(axis(self.width), axis(self.height))
    }
}

pub(crate) fn fit_viewport(viewport: SurfaceSize, output: SurfaceSize) -> Rect {
    if viewport.is_empty() || output.is_empty() {
        return Rect::default();
    }
    let scale = (f64::from(output.width) / f64::from(viewport.width))
        .min(f64::from(output.height) / f64::from(viewport.height));
    let width = (f64::from(viewport.width) * scale).round().max(1.0) as u32;
    let height = (f64::from(viewport.height) * scale).round().max(1.0) as u32;
    Rect::new(
        (output.width - width) / 2,
        (output.height - height) / 2,
        width,
        height,
    )
}

/// Game coordinates, framebuffer dimensions, and the host monitor scale.
/// A game can keep layout in logical points while drawing into the full buffer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "SavedDisplayMetrics")]
pub struct DisplayMetrics {
    #[serde(flatten)]
    pub render_size: SurfaceSize,
    pub logical_size: SurfaceSize,
    pub scale_factor: f64,
}

#[derive(Deserialize)]
struct SavedDisplayMetrics {
    #[serde(flatten)]
    render_size: SurfaceSize,
    logical_size: Option<SurfaceSize>,
    scale_factor: Option<f64>,
}

impl From<SavedDisplayMetrics> for DisplayMetrics {
    fn from(saved: SavedDisplayMetrics) -> Self {
        Self {
            render_size: saved.render_size,
            logical_size: saved.logical_size.unwrap_or(saved.render_size),
            scale_factor: saved.scale_factor.unwrap_or(1.0),
        }
    }
}

impl DisplayMetrics {
    pub const fn at_1x(size: SurfaceSize) -> Self {
        Self {
            render_size: size,
            logical_size: size,
            scale_factor: 1.0,
        }
    }

    pub fn for_surface(size: SurfaceSize, scale_factor: f64) -> Self {
        Self {
            render_size: size,
            logical_size: size.to_logical(scale_factor),
            scale_factor,
        }
    }

    pub fn logical_pointer(self, point: (u32, u32)) -> Option<(u32, u32)> {
        let (x, y) = point;
        let fitted = fit_viewport(self.logical_size, self.render_size);
        if !fitted.contains(x, y) {
            return None;
        }
        // Invert the same rounded-down edges used by FittedRenderer.
        let map = |coordinate: u32, logical, physical| {
            (((u64::from(coordinate) + 1) * u64::from(logical) - 1) / u64::from(physical)) as u32
        };
        Some((
            map(x - fitted.x, self.logical_size.width, fitted.w),
            map(y - fitted.y, self.logical_size.height, fitted.h),
        ))
    }
}

/// A resizable RGBA surface.
///
/// This is the abstraction layer between:
/// - rendering (writing RGBA pixels into `frame_mut()`), and
/// - presentation (showing or recording those pixels somewhere).
///
/// Importantly: this works for both headful (windowed) and headless (offscreen) runs.
pub trait Surface {
    type Error;

    fn size(&self) -> SurfaceSize;
    fn frame_mut(&mut self) -> &mut [u8];

    fn resize(&mut self, size: SurfaceSize) -> Result<(), Self::Error>;
    fn present(&mut self) -> Result<(), Self::Error>;
}

/// A simple in-memory RGBA surface for headless execution and tests.
#[derive(Debug, Clone)]
pub struct RgbaBufferSurface {
    size: SurfaceSize,
    buf: Vec<u8>,
}

impl RgbaBufferSurface {
    pub fn new(size: SurfaceSize) -> Self {
        Self {
            size,
            buf: vec![0u8; size.rgba_len()],
        }
    }

    pub fn frame(&self) -> &[u8] {
        &self.buf
    }
}

impl Surface for RgbaBufferSurface {
    type Error = Infallible;

    fn size(&self) -> SurfaceSize {
        self.size
    }

    fn frame_mut(&mut self) -> &mut [u8] {
        &mut self.buf
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), Self::Error> {
        self.size = size;
        self.buf.resize(size.rgba_len(), 0u8);
        Ok(())
    }

    fn present(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_pointer_matches_raster_edges_and_excludes_letterboxing() {
        let logical = SurfaceSize::new(8, 6);
        let retina = DisplayMetrics::for_surface(SurfaceSize::new(16, 12), 2.0);
        assert_eq!(retina.logical_size, logical);
        assert_eq!(retina.logical_pointer((15, 11)), Some((7, 5)));
        assert_eq!(
            DisplayMetrics::at_1x(logical).logical_pointer((3, 3)),
            Some((3, 3))
        );
        let fractional = DisplayMetrics::for_surface(SurfaceSize::new(12, 9), 1.5);
        assert_eq!(fractional.logical_pointer((4, 4)), Some((3, 3)));
        let letterboxed = DisplayMetrics {
            render_size: SurfaceSize::new(20, 12),
            ..retina
        };
        assert_eq!(letterboxed.logical_pointer((1, 0)), None);
        assert_eq!(letterboxed.logical_pointer((2, 0)), Some((0, 0)));
    }
}
