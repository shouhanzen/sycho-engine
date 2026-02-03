use bytemuck::{Pod, Zeroable};
use pixels::{wgpu, PixelsContext};
use wgpu::util::DeviceExt;

use engine::render::{color_for_cell, CELL_SIZE};

use crate::tetris_core::{piece_board_offset, piece_grid, piece_type, Piece, TetrisCore, Vec2i};
use crate::tetris_ui::{
    compute_layout, GameOverMenuLayout, MainMenuLayout, PauseMenuLayout, Rect, SkillTreeLayout, UiLayout,
    MAIN_MENU_TITLE,
};

const COLOR_BACKGROUND: [u8; 4] = [10, 10, 14, 255];
const COLOR_BOARD_OUTLINE: [u8; 4] = [28, 28, 38, 255];
const COLOR_GRID_DOT: [u8; 4] = [18, 18, 24, 255];

const COLOR_PANEL_BG: [u8; 4] = [16, 16, 22, 255];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];
const COLOR_PANEL_BORDER_DISABLED: [u8; 4] = [28, 28, 38, 255];
const BUTTON_HOVER_BRIGHTEN: f32 = 0.12;

const COLOR_PAUSE_ICON: [u8; 4] = [235, 235, 245, 255];

const COLOR_PAUSE_MENU_TEXT: [u8; 4] = [235, 235, 245, 255];
const COLOR_PAUSE_MENU_DIM: [u8; 4] = [0, 0, 0, 255];
const PAUSE_MENU_DIM_ALPHA: u8 = 170;
const COLOR_PAUSE_MENU_BG: [u8; 4] = [10, 10, 14, 255];
const COLOR_PAUSE_MENU_BORDER: [u8; 4] = [40, 40, 55, 255];

const HUD_COLOR_TEXT: [u8; 4] = [235, 235, 245, 255];
const HUD_PANEL_BG: [u8; 4] = [0, 0, 0, 220];
const HUD_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];

// Preview / panel sizing (matches `tetris_ui.rs`).
const PANEL_PADDING: u32 = 12;
const PREVIEW_GRID: u32 = 4;
const PREVIEW_CELL: u32 = 16;
const PREVIEW_SIZE: u32 = PREVIEW_GRID * PREVIEW_CELL;
const PREVIEW_GAP_Y: u32 = 10;

const GHOST_ALPHA_U8: u8 = 80;

// Tiny block font (matches `debug.rs` / `tetris_ui.rs` usage of `draw_text`).
const FONT_SCALE: u32 = 2;
const GLYPH_W: u32 = 3;
const GLYPH_H: u32 = 5;
const GLYPH_ADVANCE_X: u32 = (GLYPH_W + 1) * FONT_SCALE;
const LINE_ADVANCE_Y: u32 = (GLYPH_H + 1) * FONT_SCALE;

fn color_f(c: [u8; 4], alpha_mul: f32) -> [f32; 4] {
    let a = ((c[3] as f32) / 255.0) * alpha_mul;
    [
        (c[0] as f32) / 255.0,
        (c[1] as f32) / 255.0,
        (c[2] as f32) / 255.0,
        a,
    ]
}

fn dim_color(mut c: [u8; 4], factor: f32) -> [u8; 4] {
    let f = factor.clamp(0.0, 1.0);
    c[0] = ((c[0] as f32) * f) as u8;
    c[1] = ((c[1] as f32) * f) as u8;
    c[2] = ((c[2] as f32) * f) as u8;
    c
}

fn brighten_color(mut c: [u8; 4], amount: f32) -> [u8; 4] {
    let t = amount.clamp(0.0, 1.0);
    for i in 0..3 {
        let v = c[i] as f32;
        c[i] = (v + (255.0 - v) * t).round().clamp(0.0, 255.0) as u8;
    }
    c
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Globals {
    screen: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferMode {
    /// The pixel buffer is sized to the surface; used for CPU rendering (uploads full frame).
    CpuMatchesSurface,
    /// The pixel buffer is tiny; used for GPU rendering to avoid full-frame uploads.
    GpuTiny,
}

/// A minimal GPU renderer for the Tetris gameplay view.
///
/// This renders colored rects directly to the swapchain texture via `pixels.render_with(...)`,
/// avoiding full-frame texture uploads (the pixel buffer can be kept at 1x1).
pub struct GpuTetrisRenderer {
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

impl GpuTetrisRenderer {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tetris_gpu_globals_layout"),
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
            label: Some("tetris_gpu_globals_buf"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tetris_gpu_globals_bind_group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tetris_gpu_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tetris_gpu_pipeline_layout"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tetris_gpu_pipeline"),
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
            label: Some("tetris_gpu_quad_vertices"),
            contents: bytemuck::cast_slice(&quad),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let quad_indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tetris_gpu_quad_indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let instance_capacity = 8192;
        let instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tetris_gpu_instances"),
            size: (instance_capacity * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
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

    pub fn begin_frame(&mut self) {
        self.instances.clear();
    }

    /// Convenience: clears the current instances and pushes the Tetris gameplay view.
    pub fn prepare_tetris(&mut self, width: u32, height: u32, state: &TetrisCore) -> UiLayout {
        self.begin_frame();
        self.push_tetris(width, height, state, None, None)
    }

    /// Push the Tetris world (board + pieces). Does not clear existing instances.
    pub fn push_tetris_world(&mut self, width: u32, height: u32, state: &TetrisCore) -> UiLayout {
        // Layout for positioning the board/panels and for input hit-testing.
        let board = state.board();
        let board_h = board.len().max(1) as u32;
        let board_w = board.first().map(|r| r.len()).unwrap_or(1).max(1) as u32;
        let layout = compute_layout(width, height, board_w, board_h, state.next_queue().len());

        // Board outline (subtle, outside the board region).
        self.push_board_outline(width, height, layout.board);

        // Fixed board cells + grid dots.
        let cell_size = CELL_SIZE;
        let dot_size = 2u32;
        for (y, row) in board.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                let x = x as u32;
                let y = y as u32;
                if cell == 0 {
                    // Dot at the center of each empty cell.
                    let inverted_y = board_h.saturating_sub(1).saturating_sub(y);
                    let cell_px = layout.board.x + x * cell_size;
                    let cell_py = layout.board.y + inverted_y * cell_size;
                    let dot_x = cell_px + (cell_size / 2).saturating_sub(dot_size / 2);
                    let dot_y = cell_py + (cell_size / 2).saturating_sub(dot_size / 2);
                    self.push_rect(dot_x, dot_y, dot_size, dot_size, COLOR_GRID_DOT, 1.0);
                } else {
                    self.push_cell(&layout, board_h, x, y, cell, 1.0);
                }
            }
        }

        // Ghost piece (optional).
        if let (Some(piece), Some(ghost_pos)) = (state.current_piece(), state.ghost_piece_pos()) {
            let ghost_alpha = (GHOST_ALPHA_U8 as f32) / 255.0;
            self.push_piece(
                &layout,
                board_w,
                board_h,
                piece,
                ghost_pos,
                state.current_piece_rotation(),
                ghost_alpha,
            );
        }

        // Active piece.
        if let Some(piece) = state.current_piece() {
            self.push_piece(
                &layout,
                board_w,
                board_h,
                piece,
                state.current_piece_pos(),
                state.current_piece_rotation(),
                1.0,
            );
        }

        layout
    }

    /// Push the Tetris gameplay view (board + panels + HUD). Does not clear existing instances.
    pub fn push_tetris(
        &mut self,
        width: u32,
        height: u32,
        state: &TetrisCore,
        timer_text: Option<&str>,
        cursor: Option<(u32, u32)>,
    ) -> UiLayout {
        let layout = self.push_tetris_world(width, height, state);

        // Panels.
        self.push_hold_panel(layout.hold_panel, state.held_piece(), state.can_hold());
        self.push_next_panel(layout.next_panel, state.next_queue());
        let pause_hovered = cursor
            .map(|(x, y)| layout.pause_button.contains(x, y))
            .unwrap_or(false);
        self.push_pause_button(layout.pause_button, pause_hovered);

        // HUD: score + lines.
        let hud_x = layout.pause_button.x.saturating_sub(180);
        let hud_y = layout.pause_button.y.saturating_add(6);
        let score_text = format!("SCORE {}", state.score());
        let lines_text = format!("LINES {}", state.lines_cleared());
        self.push_text(hud_x, hud_y, &score_text, COLOR_PAUSE_ICON);
        self.push_text(hud_x, hud_y.saturating_add(14), &lines_text, COLOR_PAUSE_ICON);

        // Optional timer line (headful-only HUD).
        if let Some(timer_text) = timer_text {
            self.push_text(
                hud_x,
                hud_y.saturating_add(28),
                timer_text,
                COLOR_PAUSE_ICON,
            );
        }

        layout
    }

    pub fn push_main_menu(
        &mut self,
        _width: u32,
        _height: u32,
        layout: MainMenuLayout,
        cursor: Option<(u32, u32)>,
    ) {
        // Main menu is its own scene: no modal panel, just title + buttons over the cleared background.
        let safe = layout.panel;
        let pad = 18u32;
        let content = Rect {
            x: safe.x.saturating_add(pad),
            y: safe.y.saturating_add(pad),
            w: safe.w.saturating_sub(pad.saturating_mul(2)),
            h: safe.h.saturating_sub(pad.saturating_mul(2)),
        };

        let title = MAIN_MENU_TITLE;
        let title_chars = title.chars().count() as u32;
        let glyph_cols = 4u32;
        let denom = title_chars.saturating_mul(glyph_cols).max(1);
        let max_scale = 12u32;
        let title_scale = (safe.w / denom).clamp(2, max_scale);
        let title_w = denom.saturating_mul(title_scale).min(safe.w);
        let title_h = (5u32).saturating_mul(title_scale).min(safe.h);

        let title_x = content.x.saturating_add(content.w.saturating_sub(title_w) / 2);
        let title_button_gap = 28u32;
        let title_y = if layout.start_button.h > 0 {
            layout
                .start_button
                .y
                .saturating_sub(title_button_gap)
                .saturating_sub(title_h)
        } else {
            content.y
        };

        self.push_text_scaled(
            title_x,
            title_y,
            title,
            COLOR_PAUSE_MENU_TEXT,
            title_scale,
        );

        for (rect, label) in [(layout.start_button, "START"), (layout.quit_button, "QUIT")] {
            let hovered = cursor.map(|(x, y)| rect.contains(x, y)).unwrap_or(false);
            self.push_button(rect, label, hovered);
        }
    }

    pub fn push_game_over_menu(
        &mut self,
        width: u32,
        height: u32,
        layout: GameOverMenuLayout,
        cursor: Option<(u32, u32)>,
    ) {
        self.push_dim_overlay(width, height);
        self.push_panel(layout.panel);

        let pad = 18u32;
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad),
            "GAME OVER",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 24),
            "RUN ENDED",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 48),
            "ENTER TO RESTART",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 72),
            "K: SKILL TREE",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 96),
            "ESC: MAIN MENU",
            COLOR_PAUSE_MENU_TEXT,
        );

        for (rect, label) in [
            (layout.restart_button, "RESTART"),
            (layout.skilltree_button, "SKILL TREE"),
            (layout.quit_button, "QUIT"),
        ] {
            let hovered = cursor.map(|(x, y)| rect.contains(x, y)).unwrap_or(false);
            self.push_button(rect, label, hovered);
        }
    }

    pub fn push_pause_menu(
        &mut self,
        width: u32,
        height: u32,
        layout: PauseMenuLayout,
        cursor: Option<(u32, u32)>,
    ) {
        self.push_dim_overlay(width, height);
        self.push_panel(layout.panel);

        let pad = 18u32;
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad),
            "PAUSED",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 24),
            "ESC TO RESUME",
            COLOR_PAUSE_MENU_TEXT,
        );

        let end_run_hovered = cursor
            .map(|(x, y)| layout.end_run_button.contains(x, y))
            .unwrap_or(false);
        self.push_button(layout.end_run_button, "END RUN", end_run_hovered);
        let resume_hovered = cursor
            .map(|(x, y)| layout.resume_button.contains(x, y))
            .unwrap_or(false);
        self.push_button(layout.resume_button, "RESUME", resume_hovered);
    }

    pub fn push_skilltree(
        &mut self,
        _width: u32,
        _height: u32,
        layout: SkillTreeLayout,
        cursor: Option<(u32, u32)>,
    ) {
        let pad = 18u32;
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad),
            "SKILL TREE",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 24),
            "TODO: add progression nodes",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 48),
            "ENTER: START NEW RUN",
            COLOR_PAUSE_MENU_TEXT,
        );
        self.push_text(
            layout.panel.x.saturating_add(pad),
            layout.panel.y.saturating_add(pad + 72),
            "ESC: MAIN MENU",
            COLOR_PAUSE_MENU_TEXT,
        );

        // Placeholder "nodes" so the skilltree reads like an in-world scene rather than a modal panel.
        let content = Rect {
            x: layout.panel.x.saturating_add(pad),
            y: layout.panel.y.saturating_add(pad),
            w: layout.panel.w.saturating_sub(pad.saturating_mul(2)),
            h: layout.panel.h.saturating_sub(pad.saturating_mul(2)),
        };

        let node_w = 140u32.min(content.w);
        let node_h = 64u32.min(content.h);
        if node_w > 0 && node_h > 0 {
            let nodes_band_y = content.y.saturating_add(120);
            let total_nodes_w = node_w.saturating_mul(3);
            let mut x = content
                .x
                .saturating_add(content.w.saturating_sub(total_nodes_w) / 2);
            let y = nodes_band_y;
            let gap = 24u32;

            for label in ["+SCORE", "+TIME", "+SPEED"] {
                let r = Rect {
                    x,
                    y,
                    w: node_w,
                    h: node_h,
                };
                self.push_rect(r.x, r.y, r.w, r.h, COLOR_PANEL_BG, 1.0);
                self.push_outline(r, COLOR_PANEL_BORDER);
                self.push_text(
                    r.x.saturating_add(16),
                    r.y.saturating_add(r.h / 2).saturating_sub(6),
                    label,
                    COLOR_PAUSE_MENU_TEXT,
                );
                x = x.saturating_add(node_w.saturating_add(gap));
            }
        }

        let hovered = cursor
            .map(|(x, y)| layout.start_new_game_button.contains(x, y))
            .unwrap_or(false);
        self.push_button(layout.start_new_game_button, "START NEW RUN", hovered);
    }

    pub fn push_debug_hud(&mut self, width: u32, height: u32, lines: &[String]) {
        if lines.is_empty() {
            return;
        }

        let max_chars = lines.iter().map(|l| l.len() as u32).max().unwrap_or(0);

        let pad = 6u32 * FONT_SCALE;
        let inner_w = max_chars.saturating_mul(GLYPH_ADVANCE_X);
        let inner_h = (lines.len() as u32).saturating_mul(LINE_ADVANCE_Y);
        let panel_w = (inner_w + pad * 2).min(width);
        let panel_h = (inner_h + pad * 2).min(height);

        let x0 = 10u32;
        let y0 = 10u32;

        self.push_rect(x0, y0, panel_w, panel_h, HUD_PANEL_BG, 1.0);
        self.push_outline(
            Rect {
                x: x0,
                y: y0,
                w: panel_w,
                h: panel_h,
            },
            HUD_PANEL_BORDER,
        );

        let mut y = y0 + pad;
        for line in lines {
            self.push_text(x0 + pad, y, line, HUD_COLOR_TEXT);
            y = y.saturating_add(LINE_ADVANCE_Y);
            if y >= y0 + panel_h {
                break;
            }
        }
    }

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_target: &wgpu::TextureView,
        ctx: &PixelsContext,
        width: u32,
        height: u32,
    ) {
        let width = width.max(1);
        let height = height.max(1);

        // Ensure instance buffer is large enough.
        self.ensure_instance_capacity(&ctx.device, self.instances.len());

        // Update globals + instances.
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

        // Clear color matches the CPU renderer background.
        let clear = wgpu::Color {
            r: (COLOR_BACKGROUND[0] as f64) / 255.0,
            g: (COLOR_BACKGROUND[1] as f64) / 255.0,
            b: (COLOR_BACKGROUND[2] as f64) / 255.0,
            a: 1.0,
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tetris_gpu_pass"),
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
            label: Some("tetris_gpu_instances"),
            size: (cap * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = cap;
    }

    fn push_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: [u8; 4], alpha_mul: f32) {
        if w == 0 || h == 0 {
            return;
        }
        self.instances.push(Instance {
            pos: [x as f32, y as f32],
            size: [w as f32, h as f32],
            color: color_f(color, alpha_mul),
        });
    }

    fn push_outline(&mut self, rect: Rect, color: [u8; 4]) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.push_rect(rect.x, rect.y, rect.w, 1, color, 1.0);
        if rect.h > 1 {
            self.push_rect(rect.x, rect.y + rect.h - 1, rect.w, 1, color, 1.0);
        }
        self.push_rect(rect.x, rect.y, 1, rect.h, color, 1.0);
        if rect.w > 1 {
            self.push_rect(rect.x + rect.w - 1, rect.y, 1, rect.h, color, 1.0);
        }
    }

    fn push_text(&mut self, x: u32, y: u32, text: &str, color: [u8; 4]) {
        let mut cursor_x = x;
        let mut cursor_y = y;

        for ch in text.chars() {
            match ch {
                '\n' => {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(LINE_ADVANCE_Y);
                    continue;
                }
                ' ' => {
                    cursor_x = cursor_x.saturating_add(GLYPH_ADVANCE_X);
                    continue;
                }
                _ => {}
            }

            self.push_char(cursor_x, cursor_y, ch, color);
            cursor_x = cursor_x.saturating_add(GLYPH_ADVANCE_X);
        }
    }

    fn push_text_scaled(&mut self, x: u32, y: u32, text: &str, color: [u8; 4], scale: u32) {
        let scale = scale.max(1);
        let glyph_advance_x = (GLYPH_W + 1).saturating_mul(scale);
        let line_advance_y = (GLYPH_H + 1).saturating_mul(scale);

        let mut cursor_x = x;
        let mut cursor_y = y;

        for ch in text.chars() {
            match ch {
                '\n' => {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(line_advance_y);
                    continue;
                }
                ' ' => {
                    cursor_x = cursor_x.saturating_add(glyph_advance_x);
                    continue;
                }
                _ => {}
            }

            self.push_char_scaled(cursor_x, cursor_y, ch, color, scale);
            cursor_x = cursor_x.saturating_add(glyph_advance_x);
        }
    }

    fn push_char(&mut self, x: u32, y: u32, ch: char, color: [u8; 4]) {
        let rows = glyph_rows(ch);
        for (row, bits) in rows.into_iter().enumerate() {
            let py0 = y.saturating_add((row as u32).saturating_mul(FONT_SCALE));
            for col in 0..GLYPH_W {
                let mask = 1u8 << (GLYPH_W - 1 - col);
                if (bits & mask) == 0 {
                    continue;
                }
                let px0 = x.saturating_add(col.saturating_mul(FONT_SCALE));
                self.push_rect(px0, py0, FONT_SCALE, FONT_SCALE, color, 1.0);
            }
        }
    }

    fn push_char_scaled(&mut self, x: u32, y: u32, ch: char, color: [u8; 4], scale: u32) {
        let scale = scale.max(1);
        let rows = glyph_rows(ch);
        for (row, bits) in rows.into_iter().enumerate() {
            let py0 = y.saturating_add((row as u32).saturating_mul(scale));
            for col in 0..GLYPH_W {
                let mask = 1u8 << (GLYPH_W - 1 - col);
                if (bits & mask) == 0 {
                    continue;
                }
                let px0 = x.saturating_add(col.saturating_mul(scale));
                self.push_rect(px0, py0, scale, scale, color, 1.0);
            }
        }
    }

    fn push_dim_overlay(&mut self, width: u32, height: u32) {
        let a = (PAUSE_MENU_DIM_ALPHA as f32) / 255.0;
        self.push_rect(0, 0, width, height, COLOR_PAUSE_MENU_DIM, a);
    }

    fn push_panel(&mut self, rect: Rect) {
        self.push_rect(rect.x, rect.y, rect.w, rect.h, COLOR_PAUSE_MENU_BG, 1.0);
        self.push_outline(rect, COLOR_PAUSE_MENU_BORDER);
    }

    fn push_button(&mut self, rect: Rect, label: &str, hovered: bool) {
        let (fill, border) = if hovered {
            (
                brighten_color(COLOR_PANEL_BG, BUTTON_HOVER_BRIGHTEN),
                brighten_color(COLOR_PANEL_BORDER, BUTTON_HOVER_BRIGHTEN),
            )
        } else {
            (COLOR_PANEL_BG, COLOR_PANEL_BORDER)
        };
        self.push_rect(rect.x, rect.y, rect.w, rect.h, fill, 1.0);
        self.push_outline(rect, border);
        self.push_text(
            rect.x.saturating_add(16),
            rect.y
                .saturating_add(rect.h / 2)
                .saturating_sub(6),
            label,
            COLOR_PAUSE_MENU_TEXT,
        );
    }

    fn push_board_outline(&mut self, width: u32, height: u32, board: Rect) {
        let offset_x = board.x;
        let offset_y = board.y;
        let board_pixel_width = board.w;
        let board_pixel_height = board.h;

        // Top border
        if offset_y > 0 {
            self.push_rect(
                offset_x,
                offset_y - 1,
                board_pixel_width,
                1,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }

        // Bottom border
        if offset_y + board_pixel_height < height {
            self.push_rect(
                offset_x,
                offset_y + board_pixel_height,
                board_pixel_width,
                1,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }

        // Left border
        if offset_x > 0 {
            self.push_rect(
                offset_x - 1,
                offset_y,
                1,
                board_pixel_height,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }

        // Right border
        if offset_x + board_pixel_width < width {
            self.push_rect(
                offset_x + board_pixel_width,
                offset_y,
                1,
                board_pixel_height,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }

        // Corners (only if both adjacent sides exist)
        if offset_x > 0 && offset_y > 0 {
            self.push_rect(offset_x - 1, offset_y - 1, 1, 1, COLOR_BOARD_OUTLINE, 1.0);
        }
        if offset_x + board_pixel_width < width && offset_y > 0 {
            self.push_rect(
                offset_x + board_pixel_width,
                offset_y - 1,
                1,
                1,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }
        if offset_x > 0 && offset_y + board_pixel_height < height {
            self.push_rect(
                offset_x - 1,
                offset_y + board_pixel_height,
                1,
                1,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }
        if offset_x + board_pixel_width < width && offset_y + board_pixel_height < height {
            self.push_rect(
                offset_x + board_pixel_width,
                offset_y + board_pixel_height,
                1,
                1,
                COLOR_BOARD_OUTLINE,
                1.0,
            );
        }
    }

    fn push_hold_panel(&mut self, rect: Rect, held_piece: Option<Piece>, can_hold: bool) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.push_rect(rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG, 1.0);
        let border = if can_hold {
            COLOR_PANEL_BORDER
        } else {
            COLOR_PANEL_BORDER_DISABLED
        };
        self.push_outline(rect, border);

        let preview_x = rect.x.saturating_add(PANEL_PADDING);
        let preview_y = rect.y.saturating_add(PANEL_PADDING);
        self.push_piece_preview(preview_x, preview_y, held_piece, can_hold);
    }

    fn push_next_panel(&mut self, rect: Rect, next_queue: &[Piece]) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        self.push_rect(rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG, 1.0);
        self.push_outline(rect, COLOR_PANEL_BORDER);

        let mut y = rect.y.saturating_add(PANEL_PADDING);
        let x = rect.x.saturating_add(PANEL_PADDING);
        for &piece in next_queue {
            if y.saturating_add(PREVIEW_SIZE) > rect.y.saturating_add(rect.h) {
                break;
            }
            self.push_piece_preview(x, y, Some(piece), true);
            y = y.saturating_add(PREVIEW_SIZE + PREVIEW_GAP_Y);
        }
    }

    fn push_piece_preview(&mut self, x: u32, y: u32, piece: Option<Piece>, enabled: bool) {
        // Preview background area.
        self.push_rect(x, y, PREVIEW_SIZE, PREVIEW_SIZE, COLOR_BACKGROUND, 1.0);

        let Some(piece) = piece else {
            return;
        };

        let grid = piece_grid(piece, 0);
        let grid_h = grid.size() as u32;
        let grid_w = grid.size() as u32;

        let offset_x = (PREVIEW_GRID.saturating_sub(grid_w)) / 2;
        let offset_y = (PREVIEW_GRID.saturating_sub(grid_h)) / 2;

        let mut c = color_for_cell(piece_type(piece));
        if !enabled {
            c = dim_color(c, 0.55);
        }

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }
                let px = x + (offset_x + gx as u32) * PREVIEW_CELL;
                let py = y + (offset_y + gy as u32) * PREVIEW_CELL;
                self.push_rect(px, py, PREVIEW_CELL, PREVIEW_CELL, c, 1.0);
            }
        }
    }

    fn push_pause_button(&mut self, rect: Rect, hovered: bool) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }

        let (fill, border) = if hovered {
            (
                brighten_color(COLOR_PANEL_BG, BUTTON_HOVER_BRIGHTEN),
                brighten_color(COLOR_PANEL_BORDER, BUTTON_HOVER_BRIGHTEN),
            )
        } else {
            (COLOR_PANEL_BG, COLOR_PANEL_BORDER)
        };
        self.push_rect(rect.x, rect.y, rect.w, rect.h, fill, 1.0);
        self.push_outline(rect, border);

        // Pause icon: two vertical bars.
        let bar_w = (rect.w / 6).max(3).min(rect.w);
        let bar_h = (rect.h * 2 / 3).max(6).min(rect.h);
        let gap = (rect.w / 5).max(4);

        let icon_total_w = bar_w.saturating_mul(2).saturating_add(gap);
        let icon_x0 = rect.x + rect.w.saturating_sub(icon_total_w) / 2;
        let icon_y0 = rect.y + rect.h.saturating_sub(bar_h) / 2;

        self.push_rect(icon_x0, icon_y0, bar_w, bar_h, COLOR_PAUSE_ICON, 1.0);
        self.push_rect(
            icon_x0.saturating_add(bar_w + gap),
            icon_y0,
            bar_w,
            bar_h,
            COLOR_PAUSE_ICON,
            1.0,
        );
    }

    fn push_cell(&mut self, layout: &UiLayout, board_h: u32, x: u32, y: u32, cell: u8, alpha: f32) {
        let cell_size = CELL_SIZE as f32;
        let inverted_y = board_h.saturating_sub(1).saturating_sub(y);

        let px = layout.board.x as f32 + (x as f32) * cell_size;
        let py = layout.board.y as f32 + (inverted_y as f32) * cell_size;

        let color = color_f(color_for_cell(cell), alpha);

        self.instances.push(Instance { pos: [px, py], size: [cell_size, cell_size], color });
    }

    fn push_piece(
        &mut self,
        layout: &UiLayout,
        board_w: u32,
        board_h: u32,
        piece: Piece,
        pos: Vec2i,
        rotation: u8,
        alpha: f32,
    ) {
        let grid = piece_grid(piece, rotation);
        let offset = piece_board_offset(piece);
        let cell = piece_type(piece);

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }

                let board_x = pos.x + gx as i32 - offset;
                let board_y = pos.y - gy as i32 + offset;

                if board_x < 0 || board_x >= board_w as i32 {
                    continue;
                }
                if board_y < 0 || board_y >= board_h as i32 {
                    continue;
                }

                self.push_cell(layout, board_h, board_x as u32, board_y as u32, cell, alpha);
            }
        }
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
