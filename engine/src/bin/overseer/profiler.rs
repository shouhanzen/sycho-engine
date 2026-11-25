use super::canvas::{BORDER, GREEN, GREEN_LIGHT, HEADER, INK, MUTED, Painter, RED};
use super::text::Style;
use engine::graphics::Color;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use engine::embedded::PreviewFrame;
use engine::profiling::{PreviewProfile, micros};
use engine::surface::SurfaceSize;

use super::board::{Bounds, Point, TITLE_HEIGHT};

const SAMPLE_LIMIT: usize = 600;
pub const GRAPH_SPAN: Duration = Duration::from_secs(8);
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct FrameKey {
    pub generation: u64,
    pub frame: u64,
}

#[derive(Clone, Copy)]
pub struct Arrival {
    pub key: FrameKey,
    pub at: Instant,
    pub transport_us: u64,
}

pub struct Sample {
    pub key: FrameKey,
    pub at: Instant,
    pub game: PreviewProfile,
    pub size: SurfaceSize,
    pub shared: bool,
    pub paused: bool,
    pub composited: bool,
    pub skipped: u64,
    pub transport_us: u64,
    pub editor_wait_us: u64,
    pub editor_render_us: u64,
    pub editor_interval_us: Option<u64>,
    pub editor_gpu_us: Option<u64>,
}

#[derive(Default)]
pub struct History {
    pub samples: VecDeque<Sample>,
    pub selected: Option<FrameKey>,
    pub revision: u64,
    refreshed: Option<Instant>,
}

impl History {
    pub fn clear(&mut self) {
        *self = Self {
            revision: self.revision.wrapping_add(1),
            ..Self::default()
        };
    }

    pub fn wants_frame(&self, arrival: Arrival) -> bool {
        self.samples
            .back()
            .is_none_or(|sample| sample.key != arrival.key)
    }

    pub fn record(
        &mut self,
        frame: &PreviewFrame,
        arrival: Arrival,
        editor_start: Instant,
        editor_render_us: u64,
        editor_interval_us: Option<u64>,
        composited: bool,
    ) {
        let Some(game) = frame.profile else { return };
        if !self.wants_frame(arrival) {
            return;
        }
        let skipped = self
            .samples
            .back()
            .filter(|previous| previous.key.generation == arrival.key.generation)
            .map_or(0, |previous| {
                arrival
                    .key
                    .frame
                    .saturating_sub(previous.key.frame)
                    .saturating_sub(1)
            });
        self.samples.push_back(Sample {
            key: arrival.key,
            at: arrival.at,
            game,
            size: frame.size,
            #[cfg(target_os = "macos")]
            shared: frame.surface.is_some(),
            #[cfg(not(target_os = "macos"))]
            shared: false,
            paused: frame.status.paused,
            composited,
            skipped,
            transport_us: arrival.transport_us,
            editor_wait_us: micros(editor_start.saturating_duration_since(arrival.at)),
            editor_render_us,
            editor_interval_us,
            editor_gpu_us: None,
        });
        if self.samples.len() > SAMPLE_LIMIT
            && self
                .samples
                .pop_front()
                .is_some_and(|sample| Some(sample.key) == self.selected)
        {
            self.selected = None;
        }
    }

    pub fn set_editor_gpu(&mut self, key: FrameKey, elapsed: Option<u64>) {
        if let Some(sample) = self
            .samples
            .iter_mut()
            .rev()
            .find(|sample| sample.key == key)
        {
            sample.editor_gpu_us = elapsed;
        }
    }

    pub fn refresh(&mut self, now: Instant) {
        if self
            .refreshed
            .is_none_or(|last| now.duration_since(last) >= REFRESH_INTERVAL)
        {
            self.revision = self.revision.wrapping_add(1);
            self.refreshed = Some(now);
        }
    }

    pub fn detail(&self) -> Option<&Sample> {
        if let Some(key) = self.selected {
            return self.samples.iter().find(|sample| sample.key == key);
        }
        self.samples.back()
    }

    pub fn select(&mut self, panel: Bounds, point: Point) {
        let layout = ProfileLayout::new(panel);
        let key = if layout.live.contains(point) {
            None
        } else if layout.graph.contains(point) {
            let Some(latest) = self.samples.back() else {
                return;
            };
            let seconds =
                GRAPH_SPAN.as_secs_f64() * (layout.graph.right() - point.x) / layout.graph.w;
            self.samples
                .iter()
                .rev()
                .take_while(|sample| latest.at.saturating_duration_since(sample.at) <= GRAPH_SPAN)
                .min_by(|a, b| {
                    let distance = |sample: &Sample| {
                        (latest.at.saturating_duration_since(sample.at).as_secs_f64() - seconds)
                            .abs()
                    };
                    distance(a).total_cmp(&distance(b))
                })
                .map(|sample| sample.key)
        } else {
            return;
        };
        self.selected = key;
        self.revision = self.revision.wrapping_add(1);
    }
}

pub struct ProfileLayout {
    pub graph: Bounds,
    pub live: Bounds,
}

impl ProfileLayout {
    pub fn new(panel: Bounds) -> Self {
        Self {
            graph: Bounds::new(
                panel.x + 16.0,
                panel.y + TITLE_HEIGHT + 38.0,
                panel.w - 32.0,
                panel.h - 286.0,
            ),
            live: Bounds::new(panel.right() - 60.0, panel.y + 5.0, 47.0, 20.0),
        }
    }
}

const PROFILE_CPU: Color = [33, 106, 70, 255];
const PROFILE_GPU: Color = [104, 72, 162, 255];
const PROFILE_EDITOR: Color = [159, 92, 21, 255];

pub(super) fn draw(paint: &mut Painter<'_>, panel: Bounds, history: &History) {
    let layout = ProfileLayout::new(panel);
    paint.text(
        Point::new(panel.x + 12.0, panel.y + 6.0),
        "Profiler",
        INK,
        Style::Title,
    );
    paint.fill(
        layout.live,
        if history.selected.is_none() {
            GREEN_LIGHT
        } else {
            HEADER
        },
    );
    paint.centered_text(layout.live, "Live", GREEN, Style::Label);
    let graph = layout.graph;
    for (offset, label, color) in [
        (0.0, "Game draw CPU", PROFILE_CPU),
        (112.0, "Game GPU", PROFILE_GPU),
        (200.0, "Editor interval", PROFILE_EDITOR),
    ] {
        paint.fill(
            Bounds::new(graph.x + offset, graph.y - 18.0, 5.0, 5.0),
            color,
        );
        paint.text(
            Point::new(graph.x + offset + 9.0, graph.y - 23.0),
            label,
            INK,
            Style::Caption,
        );
    }
    paint.fill(graph, HEADER);
    let Some(latest) = history.samples.back() else {
        paint.centered_text(graph, "Waiting for frame timings", MUTED, Style::Label);
        return;
    };
    let visible: Vec<_> = history
        .samples
        .iter()
        .rev()
        .take_while(|sample| latest.at.saturating_duration_since(sample.at) <= GRAPH_SPAN)
        .collect();
    let ceiling = visible
        .iter()
        .flat_map(|sample| {
            [
                sample.game.draw_us,
                sample.game.gpu_draw_us.unwrap_or(0),
                sample.editor_interval_us.unwrap_or(0),
            ]
        })
        .max()
        .unwrap_or(0)
        .clamp(25_000, 120_000) as f64;
    let x_of = |sample: &Sample| {
        graph.right()
            - 1.0
            - (graph.w - 1.0) * latest.at.saturating_duration_since(sample.at).as_secs_f64()
                / GRAPH_SPAN.as_secs_f64()
    };
    let y_of = |us: u64| graph.bottom() - 2.0 - (graph.h - 4.0) * (us as f64 / ceiling).min(1.0);
    let budget = y_of(16_667);
    paint.horizontal_rule(Bounds::new(graph.x, budget, graph.w, 1.0), BORDER);
    paint.text(
        Point::new(graph.x + 3.0, budget - 15.0),
        "16.7 ms",
        MUTED,
        Style::Number,
    );

    let mut previous: Option<&Sample> = None;
    for sample in visible.into_iter().rev() {
        let x = x_of(sample);
        if let Some(prev) = previous {
            if sample.key.generation != prev.key.generation
                || sample.size != prev.size
                || sample.shared != prev.shared
                || sample.paused != prev.paused
            {
                paint.vertical_rule(Bounds::new(x, graph.y, 1.0, graph.h), BORDER);
            } else {
                let prior_x = x_of(prev);
                for (from, to, color) in [
                    (
                        Some(prev.game.draw_us),
                        Some(sample.game.draw_us),
                        PROFILE_CPU,
                    ),
                    (prev.game.gpu_draw_us, sample.game.gpu_draw_us, PROFILE_GPU),
                    (
                        prev.editor_interval_us,
                        sample.editor_interval_us,
                        PROFILE_EDITOR,
                    ),
                ] {
                    if let (Some(from), Some(to)) = (from, to) {
                        let (y1, y2) = (y_of(from), y_of(to));
                        paint.horizontal_rule(
                            Bounds::new(prior_x, y1, (x - prior_x).max(1.0), 1.0),
                            color,
                        );
                        paint.vertical_rule(
                            Bounds::new(x, y1.min(y2), 1.0, (y2 - y1).abs().max(1.0)),
                            color,
                        );
                    }
                }
            }
        }
        if sample.skipped > 0 {
            paint.fill(Bounds::new(x, graph.y, 2.0, 5.0), RED);
        }
        if Some(sample.key) == history.selected {
            paint.vertical_rule(Bounds::new(x, graph.y, 1.0, graph.h), GREEN);
        }
        previous = Some(sample);
    }
    paint.text(
        Point::new(graph.x, graph.bottom() + 3.0),
        "8 s ago",
        MUTED,
        Style::Caption,
    );
    paint.text(
        Point::new(graph.right() - 27.0, graph.bottom() + 3.0),
        "Now",
        MUTED,
        Style::Caption,
    );
    if let Some(sample) = history.detail() {
        draw_frame_details(paint, graph, sample);
    }
}

fn draw_frame_details(paint: &mut Painter<'_>, graph: Bounds, sample: &Sample) {
    let y = graph.bottom() + 22.0;
    let transport = if sample.shared { "Shared" } else { "RGBA" };
    let mode = if !sample.composited {
        "Hidden"
    } else if sample.paused {
        "Paused"
    } else {
        "Live"
    };
    let title = format!(
        "G{} · Frame {} · Tick {}  {}×{}  {} · {}",
        sample.key.generation,
        sample.key.frame,
        sample.game.tick,
        sample.size.width,
        sample.size.height,
        transport,
        mode
    );
    paint.text(Point::new(graph.x, y), &title, INK, Style::Number);
    let stages = [
        ("Update", sample.game.update_us, PROFILE_CPU),
        ("Record queue", sample.game.timeline_us, PROFILE_CPU),
        ("Snapshot", sample.game.snapshot_us, PROFILE_CPU),
        ("Render queue", sample.game.queue_us, MUTED),
        ("Draw CPU", sample.game.draw_us, PROFILE_CPU),
        ("Submit CPU", sample.game.submit_us, PROFILE_CPU),
        ("GPU completion", sample.game.gpu_completion_us, PROFILE_GPU),
        ("Handoff est.", sample.transport_us, MUTED),
        ("Editor queue", sample.editor_wait_us, MUTED),
        ("Editor render", sample.editor_render_us, PROFILE_EDITOR),
    ];
    let total = stages
        .iter()
        .fold(0u64, |total, (_, duration, _)| {
            total.saturating_add(*duration)
        })
        .max(1) as f64;
    let start_x = graph.x + 105.0;
    let width = graph.w - 170.0;
    let mut elapsed = 0.0;
    for (row, (label, duration, color)) in stages.into_iter().enumerate() {
        let row_y = y + 19.0 + row as f64 * 15.0;
        paint.text(Point::new(graph.x, row_y), label, INK, Style::Caption);
        paint.fill(Bounds::new(start_x, row_y + 5.0, width, 4.0), HEADER);
        if duration > 0 {
            let bar_x = start_x + elapsed / total * width;
            let bar_w = (duration as f64 / total * width).max(1.0);
            paint.fill(
                Bounds::new(bar_x, row_y + 5.0, bar_w.min(start_x + width - bar_x), 4.0),
                color,
            );
        }
        elapsed += duration as f64;
        paint.text(
            Point::new(start_x + width + 7.0, row_y),
            &format!("{:.2} ms", duration as f64 / 1_000.0),
            INK,
            Style::Number,
        );
    }
    let gpu = |time: Option<u64>| {
        time.map_or_else(
            || "N/A".to_string(),
            |us| format!("{:.2} ms", us as f64 / 1_000.0),
        )
    };
    let footer = format!(
        "GPU game {} · editor {} · skipped {}",
        gpu(sample.game.gpu_draw_us),
        gpu(sample.editor_gpu_us),
        sample.skipped
    );
    paint.text(
        Point::new(graph.x, y + 174.0),
        &footer,
        MUTED,
        Style::Number,
    );
}
