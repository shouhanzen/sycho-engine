use std::collections::HashMap;

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont, point};
use engine::graphics::Color;
use engine::surface::SurfaceSize;
use pixels::wgpu;

use super::board::{Bounds, Point};

const ATLAS_SIZE: u32 = 2048;
const MAX_GLYPH_PX: u16 = 128;
const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

const SHADER: &str = r#"
@group(0) @binding(0) var atlas: texture_2d<f32>;
@group(0) @binding(1) var atlas_sampler: sampler;

struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(
    @builtin(vertex_index) index: u32,
    @location(0) bounds: vec4<f32>,
    @location(1) uv: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> Vertex {
    let corner = vec2<f32>(f32(index & 1u), f32(index >> 1u));
    var out: Vertex;
    out.position = vec4<f32>(mix(bounds.xy, bounds.zw, corner), 0.0, 1.0);
    out.uv = mix(uv.xy, uv.zw, corner);
    out.color = color;
    return out;
}

@fragment
fn fragment(input: Vertex) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas, atlas_sampler, input.uv).r;
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}
"#;

#[derive(Clone, Copy)]
pub enum Style {
    Title,
    Label,
    Caption,
    Number,
}

impl Style {
    pub fn size(self) -> f64 {
        match self {
            Self::Title => 14.0,
            Self::Label => 13.0,
            Self::Caption => 11.0,
            Self::Number => 12.0,
        }
    }

    fn face(self) -> usize {
        match self {
            Self::Label | Self::Caption => 0,
            Self::Title => 1,
            Self::Number => 2,
        }
    }
}

#[derive(Clone, Copy)]
struct GlyphImage {
    x: u32,
    y: u32,
    bounds: QuadBounds,
}

#[derive(Clone, Copy)]
struct QuadBounds {
    left: f32,
    top: f32,
    width: u32,
    height: u32,
}

struct Upload {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Atlas {
    glyphs: HashMap<(usize, u16, GlyphId), Option<GlyphImage>>,
    uploads: Vec<Upload>,
    x: u32,
    y: u32,
    row_height: u32,
    full: bool,
}

impl Atlas {
    fn begin_frame(&mut self) {
        // Leave space for a redraw at a new zoom level. The current draw
        // commands are replaced before any reused atlas slots are uploaded.
        if self.full || self.y + self.row_height > ATLAS_SIZE * 3 / 4 {
            *self = Self::default();
        }
    }

    fn glyph(
        &mut self,
        face: usize,
        font: &FontRef<'_>,
        px: u16,
        id: GlyphId,
    ) -> Option<GlyphImage> {
        let key = (face, px, id);
        if let Some(&cached) = self.glyphs.get(&key) {
            return cached;
        }
        let Some(outline) =
            font.outline_glyph(id.with_scale_and_position(scale(font, px), point(0.0, 0.0)))
        else {
            self.glyphs.insert(key, None);
            return None;
        };
        let bbox = outline.px_bounds();
        let bounds = QuadBounds {
            left: bbox.min.x,
            top: bbox.min.y,
            width: bbox.width() as u32,
            height: bbox.height() as u32,
        };
        // A transparent perimeter keeps linear sampling inside this glyph.
        let (width, height) = (bounds.width + 2, bounds.height + 2);
        if self.x + width > ATLAS_SIZE {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
        if self.y + height > ATLAS_SIZE {
            self.full = true;
            return None;
        }
        let mut bytes = vec![0; (width * height) as usize];
        outline.draw(|x, y, coverage| {
            bytes[((y + 1) * width + x + 1) as usize] =
                (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
        });
        let image = GlyphImage {
            x: self.x + 1,
            y: self.y + 1,
            bounds,
        };
        self.uploads.push(Upload {
            x: self.x,
            y: self.y,
            width,
            height,
            bytes,
        });
        self.glyphs.insert(key, Some(image));
        self.x += width;
        self.row_height = self.row_height.max(height);
        Some(image)
    }
}

fn scale(font: &FontRef<'_>, px: u16) -> PxScale {
    // The UI size represents a font's em square. ab_glyph measures from
    // ascent to descent, so convert to its pixel-height convention.
    (f32::from(px) * font.height_unscaled() / font.units_per_em().unwrap()).into()
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    bounds: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}

pub struct TextRenderer {
    fonts: [FontRef<'static>; 3],
    atlas: Atlas,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    capacity: usize,
    instances: Vec<Instance>,
    dirty: bool,
    logical: SurfaceSize,
    pixel_scale: Point,
}

impl TextRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("editor font atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("editor font"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("editor font"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vertex",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fragment",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("editor font atlas"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let capacity = 1024;
        Self {
            fonts: [INTER, INTER_SEMIBOLD, JETBRAINS_MONO]
                .map(|font| FontRef::try_from_slice(font).expect("valid bundled editor font")),
            atlas: Atlas::default(),
            texture,
            bind_group,
            pipeline,
            buffer: instance_buffer(device, capacity),
            capacity,
            instances: Vec::with_capacity(capacity),
            dirty: false,
            logical: SurfaceSize::new(1, 1),
            pixel_scale: Point::new(1.0, 1.0),
        }
    }

    pub fn begin_frame(&mut self, logical: SurfaceSize, surface: SurfaceSize) {
        self.instances.clear();
        self.atlas.begin_frame();
        self.logical = logical;
        self.pixel_scale = Point::new(
            f64::from(surface.width.max(1)) / f64::from(logical.width.max(1)),
            f64::from(surface.height.max(1)) / f64::from(logical.height.max(1)),
        );
        self.dirty = true;
    }

    fn raster_size(&self, size: f64) -> u16 {
        (size * self.pixel_scale.y)
            .round()
            .clamp(1.0, f64::from(MAX_GLYPH_PX)) as u16
    }

    pub fn measure(&self, text: &str, style: Style, size: f64) -> Point {
        let font = &self.fonts[style.face()];
        let font = font.as_scaled(scale(font, self.raster_size(size)));
        let mut width = 0.0;
        let mut previous = None;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            width += previous.map_or(0.0, |last| font.kern(last, id)) + font.h_advance(id);
            previous = Some(id);
        }
        Point::new(
            f64::from(width) / self.pixel_scale.x,
            f64::from(font.height()) / self.pixel_scale.y,
        )
    }

    pub fn draw(
        &mut self,
        origin: Point,
        text: &str,
        style: Style,
        size: f64,
        color: Color,
        clips: &[Bounds],
    ) {
        if clips.is_empty() {
            return;
        }
        let px = self.raster_size(size);
        let face = style.face();
        let font = &self.fonts[face];
        let scaled = font.as_scaled(scale(font, px));
        let baseline = (origin.y * self.pixel_scale.y + f64::from(scaled.ascent())).round();
        let mut cursor = origin.x * self.pixel_scale.x;
        let mut previous = None;
        let color = color.map(|channel| f32::from(channel) / 255.0);
        for ch in text.chars() {
            let id = scaled.glyph_id(ch);
            cursor += f64::from(previous.map_or(0.0, |last| scaled.kern(last, id)));
            if let Some(image) = self.atlas.glyph(face, font, px, id) {
                let bounds = Bounds::new(
                    (cursor.round() + f64::from(image.bounds.left)) / self.pixel_scale.x,
                    (baseline + f64::from(image.bounds.top)) / self.pixel_scale.y,
                    f64::from(image.bounds.width) / self.pixel_scale.x,
                    f64::from(image.bounds.height) / self.pixel_scale.y,
                );
                for visible in clips.iter().filter_map(|clip| bounds.intersect(*clip)) {
                    let atlas_size = f64::from(ATLAS_SIZE);
                    let u = |x| {
                        ((f64::from(image.x) + (x - bounds.x) * self.pixel_scale.x) / atlas_size)
                            as f32
                    };
                    let v = |y| {
                        ((f64::from(image.y) + (y - bounds.y) * self.pixel_scale.y) / atlas_size)
                            as f32
                    };
                    let x = |x| (x / f64::from(self.logical.width) * 2.0 - 1.0) as f32;
                    let y = |y| (1.0 - y / f64::from(self.logical.height) * 2.0) as f32;
                    self.instances.push(Instance {
                        bounds: [
                            x(visible.x),
                            y(visible.y),
                            x(visible.right()),
                            y(visible.bottom()),
                        ],
                        uv: [
                            u(visible.x),
                            v(visible.y),
                            u(visible.right()),
                            v(visible.bottom()),
                        ],
                        color,
                    });
                }
            }
            cursor += f64::from(scaled.h_advance(id));
            previous = Some(id);
        }
    }

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        for upload in self.atlas.uploads.drain(..) {
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: upload.x,
                        y: upload.y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &upload.bytes,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(upload.width),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: upload.width,
                    height: upload.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let count = self.instances.len();
        if count == 0 {
            return;
        }
        if self.dirty {
            if count > self.capacity {
                self.capacity = count.next_power_of_two();
                self.buffer = instance_buffer(device, self.capacity);
            }
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.instances));
            self.dirty = false;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("editor text"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: true,
                },
            })],
            depth_stencil_attachment: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..4, 0..count as u32);
    }
}

fn instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("editor text instances"),
        size: (capacity * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_have_the_expected_spacing_and_cache_antialiased_glyphs() {
        let inter = FontRef::try_from_slice(INTER).unwrap();
        let mono = FontRef::try_from_slice(JETBRAINS_MONO).unwrap();
        let metrics = inter.as_scaled(scale(&inter, 26));
        assert!(
            metrics.h_advance(metrics.glyph_id('i')) < metrics.h_advance(metrics.glyph_id('m'))
        );
        let metrics = mono.as_scaled(scale(&mono, 24));
        assert_eq!(
            metrics.h_advance(metrics.glyph_id('1')),
            metrics.h_advance(metrics.glyph_id('8'))
        );

        let mut atlas = Atlas::default();
        let id = inter.glyph_id('G');
        let first = atlas.glyph(Style::Label.face(), &inter, 26, id).unwrap();
        let cached = atlas.glyph(Style::Label.face(), &inter, 26, id).unwrap();
        assert_eq!((first.x, first.y), (cached.x, cached.y));
        assert_eq!(atlas.uploads.len(), 1);
        assert!(
            atlas.uploads[0]
                .bytes
                .iter()
                .any(|&alpha| 0 < alpha && alpha < 255)
        );
    }
}
