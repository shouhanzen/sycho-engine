use std::convert::Infallible;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

