use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::perf_budget::{
    PerfBudgetConfig, PerfBudgetHealth, PerfBudgetSample, PerfBudgetStatus, PerfBudgetThreshold,
    classify_budget, summarize_statuses,
};
use engine::graphics::Renderer2d;
use engine::profiling::{FrameOutput, FrameTimings};
use engine::ui::Rect;

const COLOR_TEXT: [u8; 4] = [235, 235, 245, 255];
const COLOR_TEXT_WARN: [u8; 4] = [245, 198, 92, 255];
const COLOR_TEXT_CRITICAL: [u8; 4] = [245, 96, 96, 255];
const COLOR_PANEL_BG: [u8; 4] = [0, 0, 0, 220];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];
const MAX_LOG_LINES: usize = 8;

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
struct HudLine {
    text: String,
    color: [u8; 4],
}

impl HudLine {
    fn plain(text: String) -> Self {
        Self {
            text,
            color: COLOR_TEXT,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DebugHud {
    enabled: bool,
    minimized: bool,

    last_input_at: Option<Instant>,

    frame_dt: RollingMs,
    engine_dt: RollingMs,
    draw_dt: RollingMs,
    output: FrameOutput,
    frame_total_dt: RollingMs,

    budget_config: PerfBudgetConfig,
    budget_frame_total: PerfBudgetSample,
    budget_engine_total: PerfBudgetSample,
    budget_draw: PerfBudgetSample,
    budget_health: PerfBudgetHealth,
    logs: VecDeque<HudLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugHudAction {
    ToggleMinimized,
    ToggleRoundTimer,
}

impl DebugHud {
    pub fn new() -> Self {
        let window = 120;
        Self {
            enabled: true,
            minimized: false,
            last_input_at: None,
            frame_dt: RollingMs::new(window),
            engine_dt: RollingMs::new(window),
            draw_dt: RollingMs::new(window),
            output: FrameOutput::Present(Duration::ZERO),
            frame_total_dt: RollingMs::new(window),
            budget_config: PerfBudgetConfig::from_env(),
            budget_frame_total: PerfBudgetSample::default(),
            budget_engine_total: PerfBudgetSample::default(),
            budget_draw: PerfBudgetSample::default(),
            budget_health: PerfBudgetHealth::default(),
            logs: VecDeque::new(),
        }
    }

    pub fn log_warning(&mut self, message: impl Into<String>) {
        let line = HudLine {
            text: format!("LOG WARN {}", message.into()),
            color: COLOR_TEXT_WARN,
        };
        if self.logs.back().is_some_and(|last| last.text == line.text) {
            return;
        }
        self.logs.push_back(line);
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
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

    pub fn mark_input(&mut self) {
        self.last_input_at = Some(Instant::now());
    }

    pub fn on_frame(&mut self, timings: FrameTimings) {
        if !self.enabled {
            return;
        }
        self.frame_dt.push(timings.interval);
        self.engine_dt.push(timings.update);
        self.draw_dt.push(timings.draw);
        self.output = timings.output;
        self.frame_total_dt.push(timings.cpu_work());
        self.update_budget_samples();
    }

    fn body_lines_colored(&self, round_timer_disabled: bool) -> Vec<HudLine> {
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

        let frame_status =
            classify_budget(&self.budget_frame_total, self.budget_config.frame_total);
        let engine_status =
            classify_budget(&self.budget_engine_total, self.budget_config.engine_total);
        let draw_status = classify_budget(&self.budget_draw, self.budget_config.draw);
        let summary = summarize_statuses([frame_status, engine_status, draw_status]);
        let output_label = match self.output {
            FrameOutput::Submit(_) => "SUBMIT",
            FrameOutput::Present(_) => "PRESENT",
        };

        vec![
            HudLine::plain(format!(
                "FPS {:>5.1}  FRAME {:>5.1}MS (AVG {:>5.1}MS)",
                fps,
                self.frame_dt.last(),
                self.frame_dt.avg()
            )),
            HudLine::plain(format!("INPUT AGE {:>5.1}MS", since_input_ms)),
            HudLine::plain(format!(
                "UPDATE {:>5.2}MS  DRAW {:>5.2}MS",
                self.engine_dt.last(),
                self.draw_dt.last()
            )),
            HudLine::plain(format!(
                "{output_label} {:>5.2}MS  CPU WORK {:>5.2}MS",
                duration_ms(self.output.elapsed()),
                self.frame_total_dt.last()
            )),
            self.budget_metric_line(
                "FRAME",
                frame_status,
                self.budget_config.frame_total,
                &self.budget_frame_total,
            ),
            self.budget_metric_line(
                "ENGINE",
                engine_status,
                self.budget_config.engine_total,
                &self.budget_engine_total,
            ),
            self.budget_metric_line(
                "DRAW",
                draw_status,
                self.budget_config.draw,
                &self.budget_draw,
            ),
            HudLine {
                text: format!(
                    "BUD NOW WARN {} CRIT {}",
                    summary.warn_metrics, summary.critical_metrics
                ),
                color: Self::status_color(summary.overall_status),
            },
            HudLine {
                text: format!(
                    "BUD HEALTH WARN {:>5.1}% CRIT {:>5.1}% CRIT_STREAK {:>3} MAX {:>3}",
                    self.budget_health.warn_pct(),
                    self.budget_health.critical_pct(),
                    self.budget_health.consecutive_critical_frames,
                    self.budget_health.max_consecutive_critical_frames
                ),
                color: Self::status_color(summary.overall_status),
            },
            HudLine {
                text: format!(
                    "TIMER {} [CLICK]",
                    if round_timer_disabled { "OFF" } else { "ON" }
                ),
                color: if round_timer_disabled {
                    COLOR_TEXT_WARN
                } else {
                    COLOR_TEXT
                },
            },
            HudLine::plain("F3 TOGGLE".to_string()),
        ]
        .into_iter()
        .chain(if self.logs.is_empty() {
            Vec::<HudLine>::new()
        } else {
            let mut log_lines = Vec::with_capacity(self.logs.len() + 1);
            log_lines.push(HudLine::plain("LOGS".to_string()));
            log_lines.extend(self.logs.iter().cloned());
            log_lines
        })
        .collect()
    }

    pub fn lines(&self, round_timer_disabled: bool) -> Vec<String> {
        self.body_lines_colored(round_timer_disabled)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    fn overlay_lines_colored(&self, round_timer_disabled: bool) -> Vec<HudLine> {
        if self.minimized {
            return vec![HudLine::plain("DEBUG [+]".to_string())];
        }

        let mut lines = self.body_lines_colored(round_timer_disabled);
        lines.insert(0, HudLine::plain("DEBUG [-]".to_string()));
        lines
    }

    pub fn overlay_lines(&self, round_timer_disabled: bool) -> Vec<String> {
        self.overlay_lines_colored(round_timer_disabled)
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    fn panel_rect_for_lines(width: u32, height: u32, lines: &[HudLine]) -> (Rect, u32) {
        let max_chars = lines
            .iter()
            .map(|line| line.text.len() as u32)
            .max()
            .unwrap_or(0);

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

    fn line_rect_for_index(panel: Rect, pad: u32, line_index: usize) -> Option<Rect> {
        let line_index = u32::try_from(line_index).ok()?;
        let y = panel
            .y
            .saturating_add(pad)
            .saturating_add(line_index.saturating_mul(LINE_ADVANCE_Y));
        let bottom = panel.y.saturating_add(panel.h);
        if y >= bottom {
            return None;
        }
        let h = LINE_ADVANCE_Y.min(bottom.saturating_sub(y));
        if h == 0 {
            return None;
        }
        let x = panel.x.saturating_add(pad);
        let w = panel.w.saturating_sub(pad.saturating_mul(2));
        if w == 0 {
            return None;
        }
        Some(Rect::new(x, y, w, h))
    }

    fn timer_toggle_rect_for_lines(panel: Rect, pad: u32, lines: &[HudLine]) -> Option<Rect> {
        let index = lines
            .iter()
            .position(|line| line.text.starts_with("TIMER "))?;
        Self::line_rect_for_index(panel, pad, index)
    }

    pub fn timer_toggle_rect(
        &self,
        width: u32,
        height: u32,
        round_timer_disabled: bool,
    ) -> Option<Rect> {
        if !self.enabled || self.minimized {
            return None;
        }
        let lines = self.overlay_lines_colored(round_timer_disabled);
        let (panel, pad) = Self::panel_rect_for_lines(width, height, &lines);
        Self::timer_toggle_rect_for_lines(panel, pad, &lines)
    }

    pub fn action_at(
        &self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        round_timer_disabled: bool,
    ) -> Option<DebugHudAction> {
        if !self.enabled {
            return None;
        }

        let lines = self.overlay_lines_colored(round_timer_disabled);
        let (panel, pad) = Self::panel_rect_for_lines(width, height, &lines);
        let header = Self::header_rect_for_panel(panel, pad);
        if header.contains(x, y) {
            return Some(DebugHudAction::ToggleMinimized);
        }
        Self::timer_toggle_rect_for_lines(panel, pad, &lines)
            .filter(|rect| rect.contains(x, y))
            .map(|_| DebugHudAction::ToggleRoundTimer)
    }

    pub fn draw_overlay(
        &self,
        gfx: &mut dyn Renderer2d,
        width: u32,
        height: u32,
        round_timer_disabled: bool,
    ) {
        if !self.enabled {
            return;
        }

        let lines = self.overlay_lines_colored(round_timer_disabled);
        let (panel, pad) = Self::panel_rect_for_lines(width, height, &lines);
        gfx.fill_rect(panel, COLOR_PANEL_BG);
        gfx.rect_outline(panel, COLOR_PANEL_BORDER);

        let mut y = panel.y + pad;
        for line in &lines {
            gfx.draw_text(panel.x + pad, y, &line.text, line.color);
            y = y.saturating_add(LINE_ADVANCE_Y);
            if y >= panel.y + panel.h {
                break;
            }
        }
    }

    fn update_budget_samples(&mut self) {
        let frame_status = self.budget_frame_total.observe(
            self.frame_total_dt.last(),
            self.frame_total_dt.avg(),
            self.budget_config.frame_total,
        );
        let engine_status = self.budget_engine_total.observe(
            self.engine_dt.last(),
            self.engine_dt.avg(),
            self.budget_config.engine_total,
        );
        let draw_status = self.budget_draw.observe(
            self.draw_dt.last(),
            self.draw_dt.avg(),
            self.budget_config.draw,
        );
        self.budget_health.observe_summary(summarize_statuses([
            frame_status,
            engine_status,
            draw_status,
        ]));
    }

    fn budget_metric_line(
        &self,
        label: &str,
        status: PerfBudgetStatus,
        threshold: PerfBudgetThreshold,
        sample: &PerfBudgetSample,
    ) -> HudLine {
        HudLine {
            text: format!(
                "BUD {label:<7} {:<4} L {:>5.2} A {:>5.2} T {:>5.2}/{:>5.2} HIT {:>4}/{:>4}",
                status.label(),
                sample.last_ms,
                sample.avg_ms,
                threshold.warn_ms,
                threshold.critical_ms,
                sample.over_warn_count,
                sample.over_critical_count
            ),
            color: Self::status_color(status),
        }
    }

    fn status_color(status: PerfBudgetStatus) -> [u8; 4] {
        match status {
            PerfBudgetStatus::Ok => COLOR_TEXT,
            PerfBudgetStatus::Warn => COLOR_TEXT_WARN,
            PerfBudgetStatus::Critical => COLOR_TEXT_CRITICAL,
        }
    }
}
