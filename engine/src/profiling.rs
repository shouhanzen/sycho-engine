use std::time::Duration;

/// Optional hook for observing the elapsed time of a simulation step.
pub trait Profiler {
    fn on_step(&mut self, _frame: usize, _elapsed: Duration) {}
}
