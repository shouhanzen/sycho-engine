use std::time::Duration;

#[derive(Debug, Clone, Copy, Default)]
pub struct StepTimings {
    pub step: Duration,
    pub record: Duration,
    pub total: Duration,
}

/// Optional hook interface for capturing engine step timings.
///
/// This is intentionally generic: it avoids depending on game-specific State/Input types
/// so it can be used across headful, headless, and editor integrations.
pub trait Profiler {
    fn on_step(&mut self, _frame: usize, _timings: StepTimings) {}
}
