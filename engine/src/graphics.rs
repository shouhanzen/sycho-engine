use crate::{surface::SurfaceSize, ui::Rect};
use pixels::{wgpu, PixelsContext};
use wgpu::util::DeviceExt;

pub type Color = [u8; 4];

// A tiny block font (no external deps). Kept deliberately simple.
pub const DEFAULT_TEXT_SCALE: u32 = 2;
const GLYPH_W: u32 = 3;
const GLYPH_H: u32 = 5;

fn glyph_advance_x(scale: u32) -> u32 {
    (GLYPH_W + 1) * scale.max(1)
}

fn line_advance_y(scale: u32) -> u32 {
    (GLYPH_H + 1) * scale.max(1)
}

/// Unified 2D rendering interface.
///
/// Game code should only talk to this trait — it must not care whether the underlying renderer is
/// CPU (RGBA buffer) or GPU (instanced rects).
pub trait Renderer2d {
    fn begin_frame(&mut self, size: SurfaceSize);
    fn size(&self) -> SurfaceSize;

    /// Opaque fill (matches the CPU renderer semantics).
    fn fill_rect(&mut self, rect: Rect, color: Color);

    /// Alpha-blended rect over existing content (alpha is applied to `color`'s RGB).
    fn blend_rect(&mut self, rect: Rect, color: Color, alpha: u8);

    fn rect_outline(&mut self, rect: Rect, color: Color);
    fn draw_text_scaled(&mut self, x: u32, y: u32, text: &str, color: Color, scale: u32);

    fn draw_text(&mut self, x: u32, y: u32, text: &str, color: Color) {
        self.draw_text_scaled(x, y, text, color, DEFAULT_TEXT_SCALE);
    }

    fn clear(&mut self, color: Color) {
        let s = self.size();
        self.fill_rect(Rect::from_size(s.width, s.height), color);
    }
}

/// CPU renderer that draws into an RGBA frame buffer.
pub struct CpuRenderer<'a> {
    frame: &'a mut [u8],
    size: SurfaceSize,
}

impl<'a> CpuRenderer<'a> {
    pub fn new(frame: &'a mut [u8], size: SurfaceSize) -> Self {
        Self { frame, size }
    }
}

impl Renderer2d for CpuRenderer<'_> {
    fn begin_frame(&mut self, size: SurfaceSize) {
        self.size = size;
    }

    fn size(&self) -> SurfaceSize {
        self.size
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let width = self.size.width;
        let height = self.size.height;

        let max_x = rect.x.saturating_add(rect.w).min(width);
        let max_y = rect.y.saturating_add(rect.h).min(height);
        if rect.x >= max_x || rect.y >= max_y {
            return;
        }

        let width_usize = width as usize;
        let height_usize = height as usize;
        let expected_len = width_usize
            .checked_mul(height_usize)
            .and_then(|v| v.checked_mul(4))
            .unwrap_or(0);
        if expected_len == 0 || self.frame.len() < expected_len {
            return;
        }

        let row_pixels = (max_x - rect.x) as usize;
        let row_bytes = row_pixels.checked_mul(4).unwrap_or(0);
        if row_bytes == 0 {
            return;
        }

        let stride = width_usize.checked_mul(4).unwrap_or(0);
        let mut row_start = (rect.y as usize)
            .checked_mul(stride)
            .and_then(|v| v.checked_add((rect.x as usize).checked_mul(4)?))
            .unwrap_or(0);

        let [r, g, b, a] = color;
        for _ in rect.y..max_y {
            let row_end = row_start + row_bytes;
            let row = &mut self.frame[row_start..row_end];
            for px in row.chunks_exact_mut(4) {
                px[0] = r;
                px[1] = g;
                px[2] = b;
                px[3] = a;
            }
            row_start += stride;
        }
    }

    fn blend_rect(&mut self, rect: Rect, color: Color, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(rect, color);
            return;
        }

        let width = self.size.width;
        let height = self.size.height;

        let max_x = rect.x.saturating_add(rect.w).min(width);
        let max_y = rect.y.saturating_add(rect.h).min(height);
        if rect.x >= max_x || rect.y >= max_y {
            return;
        }

        let width_usize = width as usize;
        let height_usize = height as usize;
        let expected_len = width_usize
            .checked_mul(height_usize)
            .and_then(|v| v.checked_mul(4))
            .unwrap_or(0);
        if expected_len == 0 || self.frame.len() < expected_len {
            return;
        }

        let row_pixels = (max_x - rect.x) as usize;
        let row_bytes = row_pixels.checked_mul(4).unwrap_or(0);
        if row_bytes == 0 {
            return;
        }

        let a = alpha as u32;
        let inv = 255u32 - a;
        let stride = width_usize.checked_mul(4).unwrap_or(0);
        let mut row_start = (rect.y as usize)
            .checked_mul(stride)
            .and_then(|v| v.checked_add((rect.x as usize).checked_mul(4)?))
            .unwrap_or(0);

        for _ in rect.y..max_y {
            let row_end = row_start + row_bytes;
            let row = &mut self.frame[row_start..row_end];
            for px in row.chunks_exact_mut(4) {
                let r0 = px[0] as u32;
                let g0 = px[1] as u32;
                let b0 = px[2] as u32;

                px[0] = ((r0 * inv + (color[0] as u32) * a + 127) / 255) as u8;
                px[1] = ((g0 * inv + (color[1] as u32) * a + 127) / 255) as u8;
                px[2] = ((b0 * inv + (color[2] as u32) * a + 127) / 255) as u8;
                px[3] = 255;
            }
            row_start += stride;
        }
    }

    fn rect_outline(&mut self, rect: Rect, color: Color) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }

        let width = self.size.width;
        let height = self.size.height;

        let x1 = rect.x.saturating_add(rect.w).min(width);
        let y1 = rect.y.saturating_add(rect.h).min(height);
        if rect.x >= x1 || rect.y >= y1 {
            return;
        }

        let w = x1 - rect.x;
        let h = y1 - rect.y;

        // Top / bottom.
        self.fill_rect(Rect::new(rect.x, rect.y, w, 1), color);
        if h > 1 {
            self.fill_rect(Rect::new(rect.x, y1.saturating_sub(1), w, 1), color);
        }

        // Left / right.
        self.fill_rect(Rect::new(rect.x, rect.y, 1, h), color);
        if w > 1 {
            self.fill_rect(Rect::new(x1.saturating_sub(1), rect.y, 1, h), color);
        }
    }

    fn draw_text_scaled(&mut self, x: u32, y: u32, text: &str, color: Color, scale: u32) {
        let width = self.size.width;
        let height = self.size.height;
        let scale = scale.max(1);
        let adv_x = glyph_advance_x(scale);
        let adv_y = line_advance_y(scale);

        let mut cursor_x = x;
        let mut cursor_y = y;

        for ch in text.chars() {
            match ch {
                '\n' => {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(adv_y);
                    if cursor_y >= height {
                        break;
                    }
                    continue;
                }
                ' ' => {
                    cursor_x = cursor_x.saturating_add(adv_x);
                    if cursor_x >= width {
                        break;
                    }
                    continue;
                }
                _ => {}
            }

            draw_char_cpu(self.frame, width, height, cursor_x, cursor_y, ch, color, scale);
            cursor_x = cursor_x.saturating_add(adv_x);
            if cursor_x >= width {
                break;
            }
        }
    }
}

fn draw_char_cpu(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    ch: char,
    color: Color,
    scale: u32,
) {
    let rows = glyph_rows(ch);
    for (row, bits) in rows.into_iter().enumerate() {
        let py0 = y.saturating_add((row as u32).saturating_mul(scale));
        for col in 0..GLYPH_W {
            let mask = 1u8 << (GLYPH_W - 1 - col);
            if (bits & mask) == 0 {
                continue;
            }
            let px0 = x.saturating_add(col.saturating_mul(scale));
            for dy in 0..scale {
                for dx in 0..scale {
                    set_pixel_cpu(frame, width, height, px0 + dx, py0 + dy, color);
                }
            }
        }
    }
}

fn set_pixel_cpu(frame: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: Color) {
    if x >= width || y >= height {
        return;
    }
    let idx = ((y * width + x) * 4) as usize;
    if idx + 4 <= frame.len() {
        let [r, g, b, a] = color;
        frame[idx] = r;
        frame[idx + 1] = g;
        frame[idx + 2] = b;
        frame[idx + 3] = a;
    }
}

fn glyph_rows(ch: char) -> [u8; GLYPH_H as usize] {
    let c = ch.to_ascii_uppercase();
    match c {
        // Digits
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b001, 0b001, 0b001],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],

        // Letters
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b111, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b111, 0b100, 0b100],
        'G' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b111, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b110, 0b100, 0b110, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'P' => [0b111, 0b101, 0b111, 0b100, 0b100],
        'Q' => [0b111, 0b101, 0b101, 0b111, 0b001],
        'R' => [0b111, 0b101, 0b111, 0b110, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],

        // Punctuation
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '(' => [0b010, 0b100, 0b100, 0b100, 0b010],
        ')' => [0b010, 0b001, 0b001, 0b001, 0b010],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '?' => [0b111, 0b001, 0b010, 0b000, 0b010],

        // Extras used in formatting.
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '\'' => [0b010, 0b010, 0b000, 0b000, 0b000],

        _ => [0b111, 0b001, 0b010, 0b000, 0b010], // '?'
    }
}

fn color_f(c: Color, alpha: f32) -> [f32; 4] {
    [
        (c[0] as f32) / 255.0,
        (c[1] as f32) / 255.0,
        (c[2] as f32) / 255.0,
        alpha.clamp(0.0, 1.0),
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    screen: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    pos: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
}

const SHADER: &str = r#"
struct Globals {
  screen: vec2<f32>,
  _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct VsIn {
  @location(0) pos: vec2<f32>,
  @location(1) inst_pos: vec2<f32>,
  @location(2) inst_size: vec2<f32>,
  @location(3) inst_color: vec4<f32>,
};

struct VsOut {
  @builtin(position) position: vec4<f32>,
  @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(input: VsIn) -> VsOut {
  let world = input.inst_pos + input.pos * input.inst_size;
  let ndc_x = (world.x / globals.screen.x) * 2.0 - 1.0;
  let ndc_y = 1.0 - (world.y / globals.screen.y) * 2.0;

  var out: VsOut;
  out.position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
  out.color = input.inst_color;
  return out;
}

@fragment
fn fs_main(input: VsOut) -> @location(0) vec4<f32> {
  return input.color;
}
"#;

const DEFAULT_CLEAR_COLOR: Color = [10, 10, 14, 255];

/// GPU renderer that records instanced rects and renders them via `pixels.render_with(...)`.
pub struct GpuRenderer2d {
    size: SurfaceSize,

    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    quad_vertices: wgpu::Buffer,
    quad_indices: wgpu::Buffer,
    quad_index_count: u32,

    instance_buf: wgpu::Buffer,
    instance_capacity: usize,
    instances: Vec<Instance>,
}

impl GpuRenderer2d {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx2d_globals_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx2d_globals_buf"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx2d_globals_bind_group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx2d_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx2d_pipeline_layout"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gfx2d_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Instance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            1 => Float32x2, // pos
                            2 => Float32x2, // size
                            3 => Float32x4  // color
                        ],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let quad = [
            Vertex { pos: [0.0, 0.0] },
            Vertex { pos: [1.0, 0.0] },
            Vertex { pos: [1.0, 1.0] },
            Vertex { pos: [0.0, 1.0] },
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];

        let quad_vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gfx2d_quad_vertices"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let quad_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gfx2d_quad_indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let instance_capacity = 8192;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx2d_instances"),
            size: (instance_capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            size: SurfaceSize::new(1, 1),
            pipeline,
            globals_buf,
            globals_bind_group,
            quad_vertices,
            quad_indices,
            quad_index_count: indices.len() as u32,
            instance_buf,
            instance_capacity,
            instances: Vec::with_capacity(8192),
        }
    }

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_target: &wgpu::TextureView,
        ctx: &PixelsContext,
    ) {
        let width = self.size.width.max(1);
        let height = self.size.height.max(1);

        self.ensure_instance_capacity(&ctx.device, self.instances.len());

        let globals = Globals {
            screen: [width as f32, height as f32],
            _pad: [0.0, 0.0],
        };
        ctx.queue
            .write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&globals));

        if !self.instances.is_empty() {
            ctx.queue.write_buffer(
                &self.instance_buf,
                0,
                bytemuck::cast_slice(&self.instances),
            );
        }

        let clear = wgpu::Color {
            r: (DEFAULT_CLEAR_COLOR[0] as f64) / 255.0,
            g: (DEFAULT_CLEAR_COLOR[1] as f64) / 255.0,
            b: (DEFAULT_CLEAR_COLOR[2] as f64) / 255.0,
            a: 1.0,
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gfx2d_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: render_target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: true,
                },
            })],
            depth_stencil_attachment: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, self.quad_vertices.slice(..));
        pass.set_vertex_buffer(1, self.instance_buf.slice(..));
        pass.set_index_buffer(self.quad_indices.slice(..), wgpu::IndexFormat::Uint16);

        let instance_count = self.instances.len() as u32;
        if instance_count > 0 {
            pass.draw_indexed(0..self.quad_index_count, 0, 0..instance_count);
        }
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: usize) {
        if needed <= self.instance_capacity {
            return;
        }

        let mut cap = self.instance_capacity.max(1);
        while cap < needed {
            cap = cap.saturating_mul(2);
        }

        self.instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx2d_instances"),
            size: (cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = cap;
    }

    fn push_rect_alpha(&mut self, rect: Rect, color: Color, alpha: f32) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.instances.push(Instance {
            pos: [rect.x as f32, rect.y as f32],
            size: [rect.w as f32, rect.h as f32],
            color: color_f(color, alpha),
        });
    }

    fn push_char(&mut self, x: u32, y: u32, ch: char, color: Color, scale: u32) {
        let rows = glyph_rows(ch);
        for (row, bits) in rows.into_iter().enumerate() {
            let py0 = y.saturating_add((row as u32).saturating_mul(scale));
            for col in 0..GLYPH_W {
                let mask = 1u8 << (GLYPH_W - 1 - col);
                if (bits & mask) == 0 {
                    continue;
                }
                let px0 = x.saturating_add(col.saturating_mul(scale));
                self.push_rect_alpha(
                    Rect::new(px0, py0, scale, scale),
                    color,
                    1.0,
                );
            }
        }
    }
}

impl Renderer2d for GpuRenderer2d {
    fn begin_frame(&mut self, size: SurfaceSize) {
        self.size = size;
        self.instances.clear();
    }

    fn size(&self) -> SurfaceSize {
        self.size
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        // Match CPU semantics: opaque fill.
        self.push_rect_alpha(rect, color, 1.0);
    }

    fn blend_rect(&mut self, rect: Rect, color: Color, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(rect, color);
            return;
        }
        let a = (alpha as f32) / 255.0;
        self.push_rect_alpha(rect, color, a);
    }

    fn rect_outline(&mut self, rect: Rect, color: Color) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.fill_rect(Rect::new(rect.x, rect.y, rect.w, 1), color);
        if rect.h > 1 {
            self.fill_rect(
                Rect::new(rect.x, rect.y.saturating_add(rect.h).saturating_sub(1), rect.w, 1),
                color,
            );
        }
        self.fill_rect(Rect::new(rect.x, rect.y, 1, rect.h), color);
        if rect.w > 1 {
            self.fill_rect(
                Rect::new(rect.x.saturating_add(rect.w).saturating_sub(1), rect.y, 1, rect.h),
                color,
            );
        }
    }

    fn draw_text_scaled(&mut self, x: u32, y: u32, text: &str, color: Color, scale: u32) {
        let scale = scale.max(1);
        let adv_x = glyph_advance_x(scale);
        let adv_y = line_advance_y(scale);

        let mut cursor_x = x;
        let mut cursor_y = y;
        for ch in text.chars() {
            match ch {
                '\n' => {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(adv_y);
                    continue;
                }
                ' ' => {
                    cursor_x = cursor_x.saturating_add(adv_x);
                    continue;
                }
                _ => {}
            }

            self.push_char(cursor_x, cursor_y, ch, color, scale);
            cursor_x = cursor_x.saturating_add(adv_x);
        }
    }
}

