use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A tiny helper for "time boxed" game sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundTimer {
    #[serde(with = "crate::serde_duration")]
    elapsed: Duration,
    #[serde(with = "crate::serde_duration")]
    limit: Duration,
}

impl RoundTimer {
    pub fn new(limit: Duration) -> Self {
        Self {
            elapsed: Duration::ZERO,
            limit,
        }
    }

    pub fn reset(&mut self) {
        self.elapsed = Duration::ZERO;
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub fn limit(&self) -> Duration {
        self.limit
    }

    pub fn remaining(&self) -> Duration {
        self.limit.saturating_sub(self.elapsed)
    }

    pub fn is_up(&self) -> bool {
        self.elapsed >= self.limit
    }

    pub fn tick_if_running(&mut self, dt: Duration, running: bool) {
        if !running || self.is_up() {
            return;
        }
        self.elapsed = self.elapsed.saturating_add(dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_only_while_running() {
        let mut t = RoundTimer::new(Duration::from_secs(20));
        t.tick_if_running(Duration::from_secs(1), false);
        assert_eq!(t.elapsed(), Duration::ZERO);

        t.tick_if_running(Duration::from_secs(2), true);
        assert_eq!(t.elapsed(), Duration::from_secs(2));

        t.tick_if_running(Duration::from_secs(3), false);
        assert_eq!(t.elapsed(), Duration::from_secs(2));
    }

    #[test]
    fn timer_reports_up_at_or_past_limit() {
        let mut t = RoundTimer::new(Duration::from_secs(20));
        assert!(!t.is_up());
        assert_eq!(t.remaining(), Duration::from_secs(20));

        t.tick_if_running(Duration::from_secs(20), true);
        assert!(t.is_up());
        assert_eq!(t.remaining(), Duration::ZERO);

        // Once up, it stays up and doesn't keep accumulating.
        t.tick_if_running(Duration::from_secs(5), true);
        assert!(t.is_up());
        assert_eq!(t.elapsed(), Duration::from_secs(20));
    }

    #[test]
    fn reset_clears_elapsed() {
        let mut t = RoundTimer::new(Duration::from_secs(20));
        t.tick_if_running(Duration::from_secs(5), true);
        t.reset();
        assert_eq!(t.elapsed(), Duration::ZERO);
        assert!(!t.is_up());
    }
}
