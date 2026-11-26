use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Weak},
};

use pixels::wgpu;

use super::{Image, ImageRect, Vertex, image::valid_source};
use crate::{surface::SurfaceSize, ui::Rect};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Sprite {
    pos: [f32; 2],
    size: [f32; 2],
    uv: [f32; 2],
    uv_size: [f32; 2],
}

pub(super) struct Batch {
    image: Arc<Image>,
    pub after_rects: u32,
    sprites: Range<u32>,
}

struct Texture {
    image: Weak<Image>,
    bindings: wgpu::BindGroup,
}

pub(super) struct Images {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    buffer: wgpu::Buffer,
    capacity: usize,
    textures: HashMap<usize, Texture>,
    sprites: Vec<Sprite>,
    pub batches: Vec<Batch>,
}

impl Images {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        globals: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image pipeline layout"),
            bind_group_layouts: &[globals, &layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nearest-neighbor images"),
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
                        array_stride: std::mem::size_of::<Sprite>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x2, 2 => Float32x2, 3 => Float32x2, 4 => Float32x2],
                    },
                ],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        Self {
            pipeline,
            layout,
            sampler,
            buffer: sprite_buffer(device, 256),
            capacity: 256,
            textures: HashMap::new(),
            sprites: Vec::new(),
            batches: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.sprites.clear();
        self.batches.clear();
    }

    pub fn push(
        &mut self,
        image: &Arc<Image>,
        source: Rect,
        dest: ImageRect,
        clip: Rect,
        surface: SurfaceSize,
        after_rects: u32,
    ) {
        let Some(area) = dest
            .visible(clip, surface)
            .filter(|_| valid_source(image, source))
        else {
            return;
        };
        let axis = |at: u32,
                    len: u32,
                    origin: i32,
                    dest_len: u32,
                    start: u32,
                    span: u32,
                    tex_size: u32| {
            let step = f64::from(span) / (f64::from(dest_len) * f64::from(tex_size));
            (
                (f64::from(start) / f64::from(tex_size)
                    + (f64::from(at) - f64::from(origin)) * step) as f32,
                (f64::from(len) * step) as f32,
            )
        };
        let (u, du) = axis(
            area.x,
            area.w,
            dest.x,
            dest.w,
            source.x,
            source.w,
            image.size.width,
        );
        let (v, dv) = axis(
            area.y,
            area.h,
            dest.y,
            dest.h,
            source.y,
            source.h,
            image.size.height,
        );
        let index = self.sprites.len() as u32;
        self.sprites.push(Sprite {
            pos: [area.x as f32, area.y as f32],
            size: [area.w as f32, area.h as f32],
            uv: [u, v],
            uv_size: [du, dv],
        });
        if let Some(batch) = self.batches.last_mut()
            && batch.after_rects == after_rects
            && Arc::ptr_eq(&batch.image, image)
        {
            batch.sprites.end += 1;
        } else {
            self.batches.push(Batch {
                image: Arc::clone(image),
                after_rects,
                sprites: index..index + 1,
            });
        }
    }

    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.textures
            .retain(|_, texture| texture.image.strong_count() > 0);
        if self.sprites.is_empty() {
            return;
        }
        if self.sprites.len() > self.capacity {
            self.capacity = self.sprites.len().next_power_of_two();
            self.buffer = sprite_buffer(device, self.capacity);
        }
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&self.sprites));
        for batch in &self.batches {
            let image = &batch.image;
            self.textures.entry(key(image)).or_insert_with(|| {
                let size = wgpu::Extent3d {
                    width: image.size.width,
                    height: image.size.height,
                    depth_or_array_layers: 1,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("project PNG"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    texture.as_image_copy(),
                    &image.pixels,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(size.width * 4),
                        rows_per_image: Some(size.height),
                    },
                    size,
                );
                let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("project PNG bindings"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &texture.create_view(&Default::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                Texture {
                    bindings,
                    image: Arc::downgrade(image),
                }
            });
        }
    }

    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        globals: &'a wgpu::BindGroup,
        indices: u32,
        batch: &Batch,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, globals, &[]);
        pass.set_bind_group(1, &self.textures[&key(&batch.image)].bindings, &[]);
        pass.set_vertex_buffer(1, self.buffer.slice(..));
        pass.draw_indexed(0..indices, 0, batch.sprites.clone());
    }
}

fn key(image: &Arc<Image>) -> usize {
    Arc::as_ptr(image) as usize
}

fn sprite_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("image instances"),
        size: (capacity * std::mem::size_of::<Sprite>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

const SHADER: &str = r#"
struct Globals { screen: vec2<f32>, pad: vec2<f32> }
@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var nearest: sampler;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) point: vec2<f32>, @location(1) pos: vec2<f32>,
           @location(2) size: vec2<f32>, @location(3) uv: vec2<f32>,
           @location(4) uv_size: vec2<f32>) -> VertexOut {
    let at = (pos + point * size) / globals.screen;
    return VertexOut(vec4<f32>(2.0 * at.x - 1.0, 1.0 - 2.0 * at.y, 0.0, 1.0),
                     uv + point * uv_size);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    return textureSample(image, nearest, input.uv);
}
"#;
