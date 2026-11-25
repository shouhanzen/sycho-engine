use std::{
    env, fs,
    io::{self, Write},
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::surface::SurfaceSize;

#[derive(Debug, Clone)]
pub struct ProfileConfig {
    pub target_frames: usize,
    pub warmup_frames: usize,
}

/// CPU time measured around the game hooks and renderer output for one frame.
#[derive(Debug, Clone, Copy)]
pub struct FrameTimings {
    pub interval: Duration,
    pub update: Duration,
    pub draw: Duration,
    pub output: FrameOutput,
}

impl FrameTimings {
    pub fn cpu_work(self) -> Duration {
        self.update + self.draw + self.output.elapsed()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FrameOutput {
    Submit(Duration),
    Present(Duration),
}

impl FrameOutput {
    pub fn elapsed(self) -> Duration {
        match self {
            Self::Submit(elapsed) | Self::Present(elapsed) => elapsed,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Span {
    pub start: Instant,
    pub elapsed: Duration,
}

impl Span {
    pub fn measure<T>(work: impl FnOnce() -> T) -> (T, Self) {
        let start = Instant::now();
        let result = work();
        (
            result,
            Self {
                start,
                elapsed: start.elapsed(),
            },
        )
    }
}

/// Timings for one delivered game preview. Durations are in microseconds.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewProfile {
    pub frame: u64,
    pub tick: u64,
    pub update_us: u64,
    pub timeline_us: u64,
    pub snapshot_us: u64,
    pub queue_us: u64,
    pub draw_us: u64,
    pub submit_us: u64,
    pub gpu_completion_us: u64,
    pub gpu_draw_us: Option<u64>,
    pub published_us: u64,
}

pub fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// The game and editor run on the same machine. Use wall time only to
/// estimate the duration of the handoff between processes.
pub fn wall_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, micros)
}

#[derive(Debug, Default, Clone)]
struct DurationAgg {
    n: usize,
    sum: Duration,
    max: Duration,
}

impl DurationAgg {
    fn push(&mut self, d: Duration) {
        self.n = self.n.saturating_add(1);
        self.sum = self.sum.saturating_add(d);
        if d > self.max {
            self.max = d;
        }
    }

    fn avg_ms(&self) -> f64 {
        if self.n == 0 {
            return 0.0;
        }
        (self.sum.as_secs_f64() * 1000.0) / (self.n as f64)
    }

    fn max_ms(&self) -> f64 {
        self.max.as_secs_f64() * 1000.0
    }
}

#[derive(Debug, Clone, Copy)]
struct TraceEvent {
    name: &'static str,
    ts_us: u64,
    dur_us: u64,
}

#[derive(Debug)]
pub(crate) struct TraceCapture {
    target_frames: usize,
    warmup_frames: usize,
    captured_frames: usize,
    start: Instant,
    events: Vec<TraceEvent>,

    update: DurationAgg,
    draw: DurationAgg,
    present: DurationAgg,
    frame_total: DurationAgg,
}

impl TraceCapture {
    pub fn new(config: ProfileConfig) -> Self {
        Self {
            target_frames: config.target_frames.max(1),
            warmup_frames: config.warmup_frames,
            captured_frames: 0,
            start: Instant::now(),
            events: Vec::new(),
            update: DurationAgg::default(),
            draw: DurationAgg::default(),
            present: DurationAgg::default(),
            frame_total: DurationAgg::default(),
        }
    }

    fn record(&mut self, name: &'static str, span: Span) {
        self.events.push(TraceEvent {
            name,
            ts_us: micros(span.start.duration_since(self.start)),
            dur_us: micros(span.elapsed),
        });
    }

    pub fn record_frame(
        &mut self,
        start: Instant,
        update: Option<Span>,
        draw: Option<Span>,
        present: Span,
        timings: FrameTimings,
    ) -> bool {
        if self.warmup_frames > 0 {
            self.warmup_frames -= 1;
            return false;
        }
        let total = present.start + present.elapsed - start;
        if let Some(update) = update {
            self.record("engine.update", update);
        }
        if let Some(draw) = draw {
            self.record("render.draw", draw);
        }
        self.record("render.present", present);
        self.record(
            "frame.total",
            Span {
                start,
                elapsed: total,
            },
        );
        self.update.push(timings.update);
        self.draw.push(timings.draw);
        self.present.push(timings.output.elapsed());
        self.frame_total.push(total);
        self.captured_frames += 1;
        self.captured_frames >= self.target_frames
    }

    pub fn finish(self, size: SurfaceSize) -> bool {
        if self.captured_frames == 0 {
            return true;
        }
        match self.write(size) {
            Ok(path) => {
                println!("trace written: {}", path.display());
                self.print_summary();
                true
            }
            Err(err) => {
                eprintln!("failed writing trace: {err}");
                false
            }
        }
    }

    fn write(&self, size: SurfaceSize) -> io::Result<PathBuf> {
        let dir = env::current_dir()?.join("target").join("perf_traces");
        fs::create_dir_all(&dir)?;

        let epoch_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let file_name = format!(
            "perf_trace_{epoch_ms}_{}x{}_{}f.json",
            size.width, size.height, self.captured_frames
        );
        let path = dir.join(file_name);

        let mut f = fs::File::create(&path)?;
        writeln!(f, "{{\"traceEvents\":[")?;
        for (i, e) in self.events.iter().enumerate() {
            if i > 0 {
                writeln!(f, ",")?;
            }
            write!(
                f,
                "  {{\"name\":\"{}\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":1}}",
                e.name, e.ts_us, e.dur_us
            )?;
        }
        writeln!(f, "\n]}}")?;
        Ok(path)
    }

    fn print_summary(&self) {
        println!("profile summary (ms; lower is better)");
        println!(
            "update(avg/max)  {:>7.3} / {:>7.3}",
            self.update.avg_ms(),
            self.update.max_ms()
        );
        println!(
            "draw  (avg/max)  {:>7.3} / {:>7.3}",
            self.draw.avg_ms(),
            self.draw.max_ms()
        );
        println!(
            "present(avg/max) {:>7.3} / {:>7.3}",
            self.present.avg_ms(),
            self.present.max_ms()
        );
        println!(
            "frame (avg/max)  {:>7.3} / {:>7.3}",
            self.frame_total.avg_ms(),
            self.frame_total.max_ms()
        );
    }
}
