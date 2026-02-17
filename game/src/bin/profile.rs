use std::time::{Duration, Instant};

use engine::HeadlessRunner;
use engine::graphics::CpuRenderer;
use engine::profiling::{Profiler, StepTimings};
use engine::surface::{RgbaBufferSurface, Surface, SurfaceSize};

use game::perf_budget::{
    PerfBudgetConfig, PerfBudgetHealth, PerfBudgetSample, PerfBudgetThreshold, classify_budget,
    summarize_statuses,
};
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

#[derive(Debug, Default, Clone, Copy)]
struct CliArgs {
    fail_on_budget: bool,
}

fn parse_args() -> CliArgs {
    let mut args = CliArgs::default();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fail-on-budget" => args.fail_on_budget = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}");
                print_usage();
                std::process::exit(2);
            }
        }
    }
    args
}

fn print_usage() {
    println!("usage: cargo run -p game --bin profile -- [--fail-on-budget]");
    println!("env overrides:");
    println!("  ROLLOUT_PROFILE_FRAMES=10000");
    println!("  ROLLOUT_PROFILE_WARMUP=200");
    println!("  ROLLOUT_PROFILE_WIDTH=960");
    println!("  ROLLOUT_PROFILE_HEIGHT=720");
    println!("  ROLLOUT_BUDGET_FRAME_WARN_MS=16.67");
    println!("  ROLLOUT_BUDGET_FRAME_CRIT_MS=22");
    println!("  ROLLOUT_BUDGET_ENGINE_WARN_MS=6");
    println!("  ROLLOUT_BUDGET_ENGINE_CRIT_MS=10");
    println!("  ROLLOUT_BUDGET_DRAW_WARN_MS=6");
    println!("  ROLLOUT_BUDGET_DRAW_CRIT_MS=10");
    println!("  ROLLOUT_BUDGET_OVERLAY_WARN_MS=1.5");
    println!("  ROLLOUT_BUDGET_OVERLAY_CRIT_MS=3");
}

#[derive(Debug, Default, Clone)]
struct Samples {
    xs: Vec<f64>,
    sum: f64,
}

impl Samples {
    fn with_capacity(n: usize) -> Self {
        Self {
            xs: Vec::with_capacity(n),
            sum: 0.0,
        }
    }

    fn push(&mut self, d: Duration) {
        let value = ms(d);
        self.xs.push(value);
        self.sum += value;
    }

    fn push_ms(&mut self, v: f64) {
        self.xs.push(v);
        self.sum += v;
    }

    fn last(&self) -> f64 {
        self.xs.last().copied().unwrap_or(0.0)
    }

    fn avg(&self) -> f64 {
        if self.xs.is_empty() {
            0.0
        } else {
            self.sum / (self.xs.len() as f64)
        }
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

fn print_budget_result(
    label: &str,
    threshold: PerfBudgetThreshold,
    avg_ms: f64,
    p95_ms: f64,
) -> bool {
    let avg_pass = avg_ms <= threshold.warn_ms;
    let p95_pass = p95_ms <= threshold.critical_ms;
    let pass = avg_pass && p95_pass;
    let verdict = if pass { "PASS" } else { "FAIL" };

    let reason = if pass {
        "within budget".to_string()
    } else {
        let mut reasons = Vec::new();
        if !avg_pass {
            reasons.push(format!("avg {:.3}>{:.3} warn", avg_ms, threshold.warn_ms));
        }
        if !p95_pass {
            reasons.push(format!(
                "p95 {:.3}>{:.3} crit",
                p95_ms, threshold.critical_ms
            ));
        }
        reasons.join("; ")
    };

    println!(
        "budget {label:<7} {verdict:<4} avg={avg:>7.3}/{warn:>6.3}ms p95={p95:>7.3}/{crit:>6.3}ms {reason}",
        avg = avg_ms,
        p95 = p95_ms,
        warn = threshold.warn_ms,
        crit = threshold.critical_ms
    );

    pass
}

fn main() {
    let args = parse_args();
    let frames = env_usize("ROLLOUT_PROFILE_FRAMES", 10_000).max(1);
    let warmup = env_usize("ROLLOUT_PROFILE_WARMUP", 200);
    let width = env_u32("ROLLOUT_PROFILE_WIDTH", 960).max(1);
    let height = env_u32("ROLLOUT_PROFILE_HEIGHT", 720).max(1);
    let budget_config = PerfBudgetConfig::from_env();

    println!("rollout_engine profile (headless)");
    println!("frames={frames} warmup={warmup} surface={width}x{height}");
    println!(
        "budget frame={:.2}/{:.2} engine={:.2}/{:.2} draw={:.2}/{:.2} overlay={:.2}/{:.2}",
        budget_config.frame_total.warn_ms,
        budget_config.frame_total.critical_ms,
        budget_config.engine_total.warn_ms,
        budget_config.engine_total.critical_ms,
        budget_config.draw.warn_ms,
        budget_config.draw.critical_ms,
        budget_config.overlay.warn_ms,
        budget_config.overlay.critical_ms
    );
    println!();

    let logic = TetrisLogic::new(0, Piece::all()).with_gravity(true);
    let mut runner = HeadlessRunner::new(logic);

    let mut surface = RgbaBufferSurface::new(SurfaceSize::new(width, height));
    let rgba_len = surface.size().rgba_len().max(1);

    let mut steps = StepCollector::new(warmup, frames);
    let mut draw = Samples::with_capacity(frames);
    let mut frame_total = Samples::with_capacity(frames);
    let mut frame_budget = PerfBudgetSample::default();
    let mut engine_budget = PerfBudgetSample::default();
    let mut draw_budget = PerfBudgetSample::default();
    let mut budget_health = PerfBudgetHealth::default();

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
            let frame_status = frame_budget.observe(
                frame_total.last(),
                frame_total.avg(),
                budget_config.frame_total,
            );
            let engine_status = engine_budget.observe(
                steps.total.last(),
                steps.total.avg(),
                budget_config.engine_total,
            );
            let draw_status = draw_budget.observe(draw.last(), draw.avg(), budget_config.draw);
            budget_health.observe_summary(summarize_statuses([
                frame_status,
                engine_status,
                draw_status,
            ]));

            let idx = runner.frame().wrapping_mul(997) % rgba_len;
            sink ^= surface.frame()[idx];
        }
    }

    std::hint::black_box(sink);

    let step_stats = steps.step.stats();
    let record_stats = steps.record.stats();
    let engine_stats = steps.total.stats();
    let draw_stats = draw.stats();
    let frame_stats = frame_total.stats();

    println!("(ms) lower is better");
    print_stats("step", step_stats);
    print_stats("record", record_stats);
    print_stats("engine", engine_stats);
    print_stats("draw", draw_stats);
    print_stats("frame", frame_stats);

    println!();
    println!("budget checks (pass requires avg<=warn and p95<=critical)");
    let mut budget_failures = 0usize;
    if !print_budget_result(
        "engine",
        budget_config.engine_total,
        engine_stats.avg,
        engine_stats.p95,
    ) {
        budget_failures = budget_failures.saturating_add(1);
    }
    if !print_budget_result("draw", budget_config.draw, draw_stats.avg, draw_stats.p95) {
        budget_failures = budget_failures.saturating_add(1);
    }
    if !print_budget_result(
        "frame",
        budget_config.frame_total,
        frame_stats.avg,
        frame_stats.p95,
    ) {
        budget_failures = budget_failures.saturating_add(1);
    }
    println!("budget overlay n/a  headless profile does not include debug overlay rendering");

    let current_summary = summarize_statuses([
        classify_budget(&engine_budget, budget_config.engine_total),
        classify_budget(&draw_budget, budget_config.draw),
        classify_budget(&frame_budget, budget_config.frame_total),
    ]);
    println!(
        "budget health frames={} warn={:.1}% critical={:.1}% crit_streak={} max_crit_streak={}",
        budget_health.total_frames,
        budget_health.warn_pct(),
        budget_health.critical_pct(),
        budget_health.consecutive_critical_frames,
        budget_health.max_consecutive_critical_frames
    );
    println!(
        "budget current warn_metrics={} critical_metrics={}",
        current_summary.warn_metrics, current_summary.critical_metrics
    );

    if args.fail_on_budget && budget_failures > 0 {
        eprintln!("budget gate failed: {budget_failures} metric(s) exceeded thresholds");
        std::process::exit(1);
    }
}
