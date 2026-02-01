use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use engine::profiling::{Profiler, StepTimings};

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

    pub fn draw_overlay(&self, frame: &mut [u8], width: u32, height: u32) {
        if !self.enabled {
            return;
        }

        let lines = self.lines();
        let max_chars = lines.iter().map(|l| l.len() as u32).max().unwrap_or(0);

        let pad = 6u32 * FONT_SCALE;
        let inner_w = max_chars.saturating_mul(GLYPH_ADVANCE_X);
        let inner_h = (lines.len() as u32).saturating_mul(LINE_ADVANCE_Y);
        let panel_w = (inner_w + pad * 2).min(width);
        let panel_h = (inner_h + pad * 2).min(height);

        let x0 = 10u32;
        let y0 = 10u32;

        fill_rect(frame, width, height, x0, y0, panel_w, panel_h, COLOR_PANEL_BG);
        draw_rect_outline(
            frame,
            width,
            height,
            x0,
            y0,
            panel_w,
            panel_h,
            COLOR_PANEL_BORDER,
        );

        let mut y = y0 + pad;
        for line in &lines {
            draw_text(frame, width, height, x0 + pad, y, line, COLOR_TEXT);
            y = y.saturating_add(LINE_ADVANCE_Y);
            if y >= y0 + panel_h {
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

/// Draw a tiny blocky text string into an RGBA frame buffer.
///
/// This is intentionally minimal (no font dependency); drawing is clipped to bounds.
pub fn draw_text(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
    color: [u8; 4],
) {
    let mut cursor_x = x;
    let mut cursor_y = y;

    for ch in text.chars() {
        match ch {
            '\n' => {
                cursor_x = x;
                cursor_y = cursor_y.saturating_add(LINE_ADVANCE_Y);
                if cursor_y >= height {
                    break;
                }
                continue;
            }
            ' ' => {
                cursor_x = cursor_x.saturating_add(GLYPH_ADVANCE_X);
                if cursor_x >= width {
                    break;
                }
                continue;
            }
            _ => {}
        }

        draw_char(frame, width, height, cursor_x, cursor_y, ch, color);
        cursor_x = cursor_x.saturating_add(GLYPH_ADVANCE_X);
        if cursor_x >= width {
            break;
        }
    }
}

fn draw_char(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    ch: char,
    color: [u8; 4],
) {
    let rows = glyph_rows(ch);
    for (row, bits) in rows.into_iter().enumerate() {
        let py0 = y.saturating_add((row as u32).saturating_mul(FONT_SCALE));
        for col in 0..GLYPH_W {
            let mask = 1u8 << (GLYPH_W - 1 - col);
            if (bits & mask) == 0 {
                continue;
            }
            let px0 = x.saturating_add(col.saturating_mul(FONT_SCALE));
            for dy in 0..FONT_SCALE {
                for dx in 0..FONT_SCALE {
                    set_pixel(frame, width, height, px0 + dx, py0 + dy, color);
                }
            }
        }
    }
}

fn glyph_rows(ch: char) -> [u8; GLYPH_H as usize] {
    // 3x5 bitmap font. Each u8 is a row, with the top 3 bits representing the pixels:
    // bit2=left, bit1=middle, bit0=right (so 0b101 means left+right).
    //
    // This is intentionally tiny and only needs to cover the HUD's ASCII-ish strings.
    // For unsupported characters we draw '?' so issues are visible instead of silent.
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

        // A few extras we might display in numbers/formatting.
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],

        _ => [0b111, 0b001, 0b010, 0b000, 0b010], // '?'
    }
}

fn set_pixel(frame: &mut [u8], width: u32, height: u32, x: u32, y: u32, color: [u8; 4]) {
    if x >= width || y >= height {
        return;
    }
    let idx = ((y * width + x) * 4) as usize;
    if idx + 4 <= frame.len() {
        frame[idx..idx + 4].copy_from_slice(&color);
    }
}

fn fill_rect(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    let max_x = (x + w).min(width);
    let max_y = (y + h).min(height);

    for py in y..max_y {
        for px in x..max_x {
            set_pixel(frame, width, height, px, py, color);
        }
    }
}

fn draw_rect_outline(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    if w == 0 || h == 0 {
        return;
    }

    // Top / bottom
    for px in x..(x + w).min(width) {
        set_pixel(frame, width, height, px, y, color);
        set_pixel(
            frame,
            width,
            height,
            px,
            y.saturating_add(h.saturating_sub(1)),
            color,
        );
    }
    // Left / right
    for py in y..(y + h).min(height) {
        set_pixel(frame, width, height, x, py, color);
        set_pixel(
            frame,
            width,
            height,
            x.saturating_add(w.saturating_sub(1)),
            py,
            color,
        );
    }
}

