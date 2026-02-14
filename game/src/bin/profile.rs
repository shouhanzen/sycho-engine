use std::time::{Duration, Instant};

use engine::HeadlessRunner;
use engine::graphics::CpuRenderer;
use engine::profiling::{Profiler, StepTimings};
use engine::surface::{RgbaBufferSurface, Surface, SurfaceSize};

use game::playtest::{InputAction, TetrisLogic};
use game::tetris_core::Piece;
use game::tetris_ui::draw_tetris;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Debug, Default, Clone)]
struct Samples {
    xs: Vec<f64>,
}

impl Samples {
    fn with_capacity(n: usize) -> Self {
        Self {
            xs: Vec::with_capacity(n),
        }
    }

    fn push(&mut self, d: Duration) {
        self.xs.push(ms(d));
    }

    fn push_ms(&mut self, v: f64) {
        self.xs.push(v);
    }

    fn stats(&mut self) -> Stats {
        Stats::from_samples(&mut self.xs)
    }
}

#[derive(Debug, Clone, Copy)]
struct Stats {
    n: usize,
    avg: f64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    fn from_samples(xs: &mut [f64]) -> Self {
        if xs.is_empty() {
            return Self {
                n: 0,
                avg: 0.0,
                p50: 0.0,
                p95: 0.0,
                p99: 0.0,
                max: 0.0,
            };
        }

        xs.sort_by(|a, b| a.total_cmp(b));

        let n = xs.len();
        let max = xs[n - 1];
        let sum: f64 = xs.iter().sum();
        let avg = sum / (n as f64);

        let p50 = percentile_sorted(xs, 0.50);
        let p95 = percentile_sorted(xs, 0.95);
        let p99 = percentile_sorted(xs, 0.99);

        Self {
            n,
            avg,
            p50,
            p95,
            p99,
            max,
        }
    }
}

fn percentile_sorted(xs: &[f64], p: f64) -> f64 {
    debug_assert!(!xs.is_empty());
    debug_assert!((0.0..=1.0).contains(&p));

    if xs.len() == 1 {
        return xs[0];
    }

    // Nearest-rank percentile on [0, n-1].
    let rank = ((xs.len() - 1) as f64 * p).round() as usize;
    xs[rank.min(xs.len() - 1)]
}

#[derive(Debug)]
struct StepCollector {
    warmup_frames: usize,
    step: Samples,
    record: Samples,
    total: Samples,
}

impl StepCollector {
    fn new(warmup_frames: usize, frames: usize) -> Self {
        Self {
            warmup_frames,
            step: Samples::with_capacity(frames),
            record: Samples::with_capacity(frames),
            total: Samples::with_capacity(frames),
        }
    }
}

impl Profiler for StepCollector {
    fn on_step(&mut self, frame: usize, timings: StepTimings) {
        if frame <= self.warmup_frames {
            return;
        }
        self.step.push(timings.step);
        self.record.push(timings.record);
        self.total.push(timings.total);
    }
}

fn print_stats(label: &str, s: Stats) {
    println!(
        "{label:<10} n={n:<6} avg={avg:>7.3}ms p50={p50:>7.3}ms p95={p95:>7.3}ms p99={p99:>7.3}ms max={max:>7.3}ms",
        n = s.n,
        avg = s.avg,
        p50 = s.p50,
        p95 = s.p95,
        p99 = s.p99,
        max = s.max
    );
}

fn main() {
    let frames = env_usize("ROLLOUT_PROFILE_FRAMES", 10_000).max(1);
    let warmup = env_usize("ROLLOUT_PROFILE_WARMUP", 200);
    let width = env_u32("ROLLOUT_PROFILE_WIDTH", 960).max(1);
    let height = env_u32("ROLLOUT_PROFILE_HEIGHT", 720).max(1);

    println!("rollout_engine profile (headless)");
    println!("frames={frames} warmup={warmup} surface={width}x{height}");
    println!();

    let logic = TetrisLogic::new(0, Piece::all()).with_gravity(true);
    let mut runner = HeadlessRunner::new(logic);

    let mut surface = RgbaBufferSurface::new(SurfaceSize::new(width, height));
    let rgba_len = surface.size().rgba_len().max(1);

    let mut steps = StepCollector::new(warmup, frames);
    let mut draw = Samples::with_capacity(frames);
    let mut frame_total = Samples::with_capacity(frames);

    // Prevent the compiler from eliminating rendering as dead stores.
    let mut sink: u8 = 0;

    for i in 0..(frames + warmup) {
        let frame_start = Instant::now();

        runner.step_profiled(InputAction::Noop, &mut steps);

        let draw_start = Instant::now();
        let mut gfx = CpuRenderer::new(surface.frame_mut(), SurfaceSize::new(width, height));
        draw_tetris(&mut gfx, width, height, runner.state().tetris());
        let draw_dt = draw_start.elapsed();

        if i >= warmup {
            draw.push(draw_dt);
            frame_total.push_ms(ms(frame_start.elapsed()));

            let idx = runner.frame().wrapping_mul(997) % rgba_len;
            sink ^= surface.frame()[idx];
        }
    }

    std::hint::black_box(sink);

    println!("(ms) lower is better");
    print_stats("step", steps.step.stats());
    print_stats("record", steps.record.stats());
    print_stats("engine", steps.total.stats());
    print_stats("draw", draw.stats());
    print_stats("frame", frame_total.stats());
}
