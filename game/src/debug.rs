use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use engine::graphics::Renderer2d;
use engine::profiling::{Profiler, StepTimings};
use engine::ui::Rect;

const COLOR_TEXT: [u8; 4] = [235, 235, 245, 255];
const COLOR_PANEL_BG: [u8; 4] = [0, 0, 0, 220];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];

// A tiny block font (no external deps). Kept deliberately simple.
const FONT_SCALE: u32 = 2;
const GLYPH_W: u32 = 3;
const GLYPH_H: u32 = 5;
const GLYPH_ADVANCE_X: u32 = (GLYPH_W + 1) * FONT_SCALE;
const LINE_ADVANCE_Y: u32 = (GLYPH_H + 1) * FONT_SCALE;

#[derive(Debug, Clone)]
struct RollingMs {
    cap: usize,
    values: VecDeque<f64>,
    sum: f64,
}

impl RollingMs {
    fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            values: VecDeque::new(),
            sum: 0.0,
        }
    }

    fn push(&mut self, d: Duration) {
        let v = duration_ms(d);
        self.values.push_back(v);
        self.sum += v;
        if self.values.len() > self.cap {
            if let Some(old) = self.values.pop_front() {
                self.sum -= old;
            }
        }
    }

    fn last(&self) -> f64 {
        self.values.back().copied().unwrap_or(0.0)
    }

    fn avg(&self) -> f64 {
        if self.values.is_empty() {
            0.0
        } else {
            self.sum / (self.values.len() as f64)
        }
    }
}

fn duration_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Debug, Clone)]
pub struct DebugHud {
    enabled: bool,
    minimized: bool,

    pending_input: Duration,
    pending_gravity: Duration,
    last_input_at: Option<Instant>,

    frame_dt: RollingMs,
    engine_step_dt: RollingMs,
    engine_record_dt: RollingMs,
    input_dt: RollingMs,
    gravity_dt: RollingMs,
    board_dt: RollingMs,
    draw_dt: RollingMs,
    overlay_dt: RollingMs,
    present_dt: RollingMs,
    frame_total_dt: RollingMs,
}

impl DebugHud {
    pub fn new() -> Self {
        let window = 120;
        Self {
            enabled: true,
            minimized: false,
            pending_input: Duration::ZERO,
            pending_gravity: Duration::ZERO,
            last_input_at: None,
            frame_dt: RollingMs::new(window),
            engine_step_dt: RollingMs::new(window),
            engine_record_dt: RollingMs::new(window),
            input_dt: RollingMs::new(window),
            gravity_dt: RollingMs::new(window),
            board_dt: RollingMs::new(window),
            draw_dt: RollingMs::new(window),
            overlay_dt: RollingMs::new(window),
            present_dt: RollingMs::new(window),
            frame_total_dt: RollingMs::new(window),
        }
    }

    pub fn toggle(&mut self) {
        self.enabled = !self.enabled;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn toggle_minimized(&mut self) {
        self.minimized = !self.minimized;
    }

    pub fn is_minimized(&self) -> bool {
        self.minimized
    }

    pub fn record_input(&mut self, dt: Duration) {
        self.pending_input = self.pending_input.saturating_add(dt);
        self.last_input_at = Some(Instant::now());
    }

    pub fn record_gravity(&mut self, dt: Duration) {
        self.pending_gravity = self.pending_gravity.saturating_add(dt);
    }

    pub fn on_frame(
        &mut self,
        frame_dt: Duration,
        board_dt: Duration,
        draw_dt: Duration,
        overlay_dt: Duration,
        present_dt: Duration,
        frame_total_dt: Duration,
    ) {
        if !self.enabled {
            self.pending_input = Duration::ZERO;
            self.pending_gravity = Duration::ZERO;
            return;
        }

        let input_dt = std::mem::take(&mut self.pending_input);
        let gravity_dt = std::mem::take(&mut self.pending_gravity);

        self.frame_dt.push(frame_dt);
        self.input_dt.push(input_dt);
        self.gravity_dt.push(gravity_dt);
        self.board_dt.push(board_dt);
        self.draw_dt.push(draw_dt);
        self.overlay_dt.push(overlay_dt);
        self.present_dt.push(present_dt);
        self.frame_total_dt.push(frame_total_dt);
    }

    pub fn lines(&self) -> Vec<String> {
        let avg_frame_ms = self.frame_dt.avg();
        let fps = if avg_frame_ms > 0.0 {
            1000.0 / avg_frame_ms
        } else {
            0.0
        };

        let since_input_ms = self
            .last_input_at
            .map(|t| duration_ms(t.elapsed()))
            .unwrap_or(0.0);

        vec![
            format!(
                "FPS {:>5.1}  FRAME {:>5.1}MS (AVG {:>5.1}MS)",
                fps,
                self.frame_dt.last(),
                self.frame_dt.avg()
            ),
            format!(
                "INPUT {:>5.2}MS (AVG {:>5.2})  AGE {:>5.1}MS",
                self.input_dt.last(),
                self.input_dt.avg(),
                since_input_ms
            ),
            format!(
                "ENGINE STEP {:>5.2}MS  REC {:>5.2}MS",
                self.engine_step_dt.last(),
                self.engine_record_dt.last()
            ),
            format!(
                "GRAV  {:>5.2}MS (AVG {:>5.2})",
                self.gravity_dt.last(),
                self.gravity_dt.avg()
            ),
            format!(
                "BOARD {:>5.2}MS  DRAW {:>5.2}MS  DBG {:>5.2}MS",
                self.board_dt.last(),
                self.draw_dt.last(),
                self.overlay_dt.last()
            ),
            format!(
                "PRESENT {:>5.2}MS  TOTAL {:>5.2}MS",
                self.present_dt.last(),
                self.frame_total_dt.last()
            ),
            "F3 TOGGLE".to_string(),
        ]
    }

    pub fn overlay_lines(&self) -> Vec<String> {
        if self.minimized {
            return vec!["DEBUG [+]".to_string()];
        }

        let mut lines = Vec::with_capacity(self.lines().len() + 1);
        lines.push("DEBUG [-]".to_string());
        lines.extend(self.lines());
        lines
    }

    fn panel_rect_for_lines(width: u32, height: u32, lines: &[String]) -> (Rect, u32) {
        let max_chars = lines.iter().map(|l| l.len() as u32).max().unwrap_or(0);

        let pad = 6u32 * FONT_SCALE;
        let inner_w = max_chars.saturating_mul(GLYPH_ADVANCE_X);
        let inner_h = (lines.len() as u32).saturating_mul(LINE_ADVANCE_Y);
        let panel_w = (inner_w + pad * 2).min(width);
        let panel_h = (inner_h + pad * 2).min(height);

        let x0 = 10u32;
        let y0 = 10u32;

        (Rect::new(x0, y0, panel_w, panel_h), pad)
    }

    fn header_rect_for_panel(panel: Rect, pad: u32) -> Rect {
        let header_h = (pad + LINE_ADVANCE_Y).min(panel.h);
        Rect::new(panel.x, panel.y, panel.w, header_h)
    }

    pub fn handle_click(&mut self, x: u32, y: u32, width: u32, height: u32) -> bool {
        if !self.enabled {
            return false;
        }

        let lines = self.overlay_lines();
        let (panel, pad) = Self::panel_rect_for_lines(width, height, &lines);
        let header = Self::header_rect_for_panel(panel, pad);
        if header.contains(x, y) {
            self.toggle_minimized();
            return true;
        }
        false
    }

    pub fn draw_overlay(&self, gfx: &mut dyn Renderer2d, width: u32, height: u32) {
        if !self.enabled {
            return;
        }

        let lines = self.overlay_lines();
        let (panel, pad) = Self::panel_rect_for_lines(width, height, &lines);
        gfx.fill_rect(panel, COLOR_PANEL_BG);
        gfx.rect_outline(panel, COLOR_PANEL_BORDER);

        let mut y = panel.y + pad;
        for line in &lines {
            gfx.draw_text(panel.x + pad, y, line, COLOR_TEXT);
            y = y.saturating_add(LINE_ADVANCE_Y);
            if y >= panel.y + panel.h {
                break;
            }
        }
    }
}

impl Profiler for DebugHud {
    fn on_step(&mut self, _frame: usize, timings: StepTimings) {
        if !self.enabled {
            return;
        }
        self.engine_step_dt.push(timings.step);
        self.engine_record_dt.push(timings.record);
    }
}

