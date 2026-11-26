use std::{
    fs::File,
    io::{self, BufReader},
    path::Path,
    sync::{Arc, OnceLock},
};

use crate::{surface::SurfaceSize, ui::Rect};

const MAX_IMAGE_DIMENSION: u32 = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub(super) size: SurfaceSize,
    pub(super) pixels: Vec<u8>,
}

impl Image {
    pub fn read_png(path: &Path) -> io::Result<Self> {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_transformations(
            png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
        );
        let mut reader = decoder.read_info().map_err(io::Error::other)?;
        let info = reader.info();
        if info.width > MAX_IMAGE_DIMENSION || info.height > MAX_IMAGE_DIMENSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PNG dimensions must be at most 4096 × 4096",
            ));
        }
        let len = reader
            .output_buffer_size()
            .ok_or_else(|| io::Error::other("invalid PNG dimensions"))?;
        let mut pixels = vec![0; len];
        let output = reader.next_frame(&mut pixels).map_err(io::Error::other)?;
        let size = SurfaceSize::new(output.width, output.height);
        match output.color_type {
            png::ColorType::Rgba => pixels.truncate(size.rgba_len()),
            png::ColorType::GrayscaleAlpha => {
                pixels = pixels[..output.buffer_size()]
                    .chunks_exact(2)
                    .flat_map(|pair| [pair[0], pair[0], pair[0], pair[1]])
                    .collect();
            }
            _ => return Err(io::Error::other("PNG could not be decoded as RGBA")),
        }
        Ok(Self { size, pixels })
    }

    pub(crate) fn missing_texture() -> &'static Arc<Self> {
        static IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
        IMAGE.get_or_init(|| {
            Arc::new(Self {
                size: SurfaceSize::new(2, 2),
                pixels: [
                    [255, 0, 255, 255],
                    [0, 0, 0, 255],
                    [0, 0, 0, 255],
                    [255, 0, 255, 255],
                ]
                .concat(),
            })
        })
    }

    pub fn size(&self) -> SurfaceSize {
        self.size
    }
}

/// Signed position allows a sprite to extend past the edge of the viewport.
#[derive(Clone, Copy)]
pub struct ImageRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl ImageRect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub(super) fn visible(self, clip: Rect, surface: SurfaceSize) -> Option<Rect> {
        let left = i64::from(self.x).max(i64::from(clip.x));
        let top = i64::from(self.y).max(i64::from(clip.y));
        let right = (i64::from(self.x) + i64::from(self.w))
            .min(i64::from(clip.x.saturating_add(clip.w).min(surface.width)));
        let bottom = (i64::from(self.y) + i64::from(self.h))
            .min(i64::from(clip.y.saturating_add(clip.h).min(surface.height)));
        (right > left && bottom > top).then(|| {
            Rect::new(
                left as u32,
                top as u32,
                (right - left) as u32,
                (bottom - top) as u32,
            )
        })
    }
}

pub(super) fn valid_source(image: &Image, source: Rect) -> bool {
    source.w > 0
        && source.h > 0
        && source.x.saturating_add(source.w) <= image.size.width
        && source.y.saturating_add(source.h) <= image.size.height
}

pub(super) fn scale_coordinate(at: u32, origin: i32, dest: u32, source: u32) -> u32 {
    let offset = (i64::from(at) - i64::from(origin)) as u64;
    (((2 * offset + 1) * u64::from(source)) / (2 * u64::from(dest))) as u32
}
