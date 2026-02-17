#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfBudgetThreshold {
    pub warn_ms: f64,
    pub critical_ms: f64,
}

impl PerfBudgetThreshold {
    pub const fn new(warn_ms: f64, critical_ms: f64) -> Self {
        Self {
            warn_ms,
            critical_ms,
        }
    }

    fn is_valid(self) -> bool {
        self.warn_ms.is_finite()
            && self.critical_ms.is_finite()
            && self.warn_ms > 0.0
            && self.critical_ms > 0.0
            && self.warn_ms < self.critical_ms
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PerfBudgetStatus {
    #[default]
    Ok,
    Warn,
    Critical,
}

impl PerfBudgetStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Critical => "CRIT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PerfBudgetSample {
    pub last_ms: f64,
    pub avg_ms: f64,
    pub over_warn_count: usize,
    pub over_critical_count: usize,
}

impl PerfBudgetSample {
    pub fn observe(
        &mut self,
        last_ms: f64,
        avg_ms: f64,
        threshold: PerfBudgetThreshold,
    ) -> PerfBudgetStatus {
        self.last_ms = last_ms;
        self.avg_ms = avg_ms;
        let status = classify_budget(self, threshold);
        if status >= PerfBudgetStatus::Warn {
            self.over_warn_count = self.over_warn_count.saturating_add(1);
        }
        if status == PerfBudgetStatus::Critical {
            self.over_critical_count = self.over_critical_count.saturating_add(1);
        }
        status
    }
}

pub fn classify_budget(
    sample: &PerfBudgetSample,
    threshold: PerfBudgetThreshold,
) -> PerfBudgetStatus {
    if sample.last_ms > threshold.critical_ms || sample.avg_ms > threshold.critical_ms {
        return PerfBudgetStatus::Critical;
    }
    if sample.last_ms > threshold.warn_ms || sample.avg_ms > threshold.warn_ms {
        return PerfBudgetStatus::Warn;
    }
    PerfBudgetStatus::Ok
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfBudgetConfig {
    pub frame_total: PerfBudgetThreshold,
    pub engine_total: PerfBudgetThreshold,
    pub draw: PerfBudgetThreshold,
    pub overlay: PerfBudgetThreshold,
}

impl Default for PerfBudgetConfig {
    fn default() -> Self {
        Self {
            frame_total: PerfBudgetThreshold::new(16.67, 22.0),
            engine_total: PerfBudgetThreshold::new(6.0, 10.0),
            draw: PerfBudgetThreshold::new(6.0, 10.0),
            overlay: PerfBudgetThreshold::new(1.5, 3.0),
        }
    }
}

impl PerfBudgetConfig {
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    fn from_lookup<F>(mut lookup: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let defaults = Self::default();
        Self {
            frame_total: threshold_from_lookup(
                &mut lookup,
                "ROLLOUT_BUDGET_FRAME_WARN_MS",
                "ROLLOUT_BUDGET_FRAME_CRIT_MS",
                defaults.frame_total,
            ),
            engine_total: threshold_from_lookup(
                &mut lookup,
                "ROLLOUT_BUDGET_ENGINE_WARN_MS",
                "ROLLOUT_BUDGET_ENGINE_CRIT_MS",
                defaults.engine_total,
            ),
            draw: threshold_from_lookup(
                &mut lookup,
                "ROLLOUT_BUDGET_DRAW_WARN_MS",
                "ROLLOUT_BUDGET_DRAW_CRIT_MS",
                defaults.draw,
            ),
            overlay: threshold_from_lookup(
                &mut lookup,
                "ROLLOUT_BUDGET_OVERLAY_WARN_MS",
                "ROLLOUT_BUDGET_OVERLAY_CRIT_MS",
                defaults.overlay,
            ),
        }
    }
}

fn threshold_from_lookup<F>(
    lookup: &mut F,
    warn_key: &str,
    critical_key: &str,
    defaults: PerfBudgetThreshold,
) -> PerfBudgetThreshold
where
    F: FnMut(&str) -> Option<String>,
{
    let warn_ms = parse_override_ms(lookup(warn_key)).unwrap_or(defaults.warn_ms);
    let critical_ms = parse_override_ms(lookup(critical_key)).unwrap_or(defaults.critical_ms);
    let threshold = PerfBudgetThreshold::new(warn_ms, critical_ms);
    if threshold.is_valid() {
        threshold
    } else {
        defaults
    }
}

fn parse_override_ms(raw: Option<String>) -> Option<f64> {
    raw.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfBudgetFrameSummary {
    pub warn_metrics: usize,
    pub critical_metrics: usize,
    pub overall_status: PerfBudgetStatus,
}

pub fn summarize_statuses<I>(statuses: I) -> PerfBudgetFrameSummary
where
    I: IntoIterator<Item = PerfBudgetStatus>,
{
    let mut summary = PerfBudgetFrameSummary::default();
    for status in statuses {
        summary.overall_status = summary.overall_status.max(status);
        match status {
            PerfBudgetStatus::Ok => {}
            PerfBudgetStatus::Warn => {
                summary.warn_metrics = summary.warn_metrics.saturating_add(1);
            }
            PerfBudgetStatus::Critical => {
                summary.warn_metrics = summary.warn_metrics.saturating_add(1);
                summary.critical_metrics = summary.critical_metrics.saturating_add(1);
            }
        }
    }
    summary
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PerfBudgetHealth {
    pub total_frames: usize,
    pub over_warn_frames: usize,
    pub over_critical_frames: usize,
    pub consecutive_critical_frames: usize,
    pub max_consecutive_critical_frames: usize,
}

impl PerfBudgetHealth {
    pub fn observe_summary(&mut self, summary: PerfBudgetFrameSummary) {
        self.total_frames = self.total_frames.saturating_add(1);
        if summary.overall_status >= PerfBudgetStatus::Warn {
            self.over_warn_frames = self.over_warn_frames.saturating_add(1);
        }
        if summary.overall_status == PerfBudgetStatus::Critical {
            self.over_critical_frames = self.over_critical_frames.saturating_add(1);
            self.consecutive_critical_frames = self.consecutive_critical_frames.saturating_add(1);
            self.max_consecutive_critical_frames = self
                .max_consecutive_critical_frames
                .max(self.consecutive_critical_frames);
        } else {
            self.consecutive_critical_frames = 0;
        }
    }

    pub fn warn_pct(&self) -> f64 {
        pct(self.over_warn_frames, self.total_frames)
    }

    pub fn critical_pct(&self) -> f64 {
        pct(self.over_critical_frames, self.total_frames)
    }
}

fn pct(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        (part as f64) * 100.0 / (whole as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_budget_returns_ok_warn_and_critical() {
        let threshold = PerfBudgetThreshold::new(10.0, 20.0);

        let ok = PerfBudgetSample {
            last_ms: 8.0,
            avg_ms: 9.0,
            ..PerfBudgetSample::default()
        };
        let warn = PerfBudgetSample {
            last_ms: 11.0,
            avg_ms: 9.0,
            ..PerfBudgetSample::default()
        };
        let critical = PerfBudgetSample {
            last_ms: 19.0,
            avg_ms: 21.0,
            ..PerfBudgetSample::default()
        };

        assert_eq!(classify_budget(&ok, threshold), PerfBudgetStatus::Ok);
        assert_eq!(classify_budget(&warn, threshold), PerfBudgetStatus::Warn);
        assert_eq!(
            classify_budget(&critical, threshold),
            PerfBudgetStatus::Critical
        );
    }

    #[test]
    fn observe_tracks_warn_and_critical_counters() {
        let threshold = PerfBudgetThreshold::new(10.0, 20.0);
        let mut sample = PerfBudgetSample::default();

        let s0 = sample.observe(9.0, 9.0, threshold);
        let s1 = sample.observe(12.0, 9.0, threshold);
        let s2 = sample.observe(25.0, 9.0, threshold);

        assert_eq!(s0, PerfBudgetStatus::Ok);
        assert_eq!(s1, PerfBudgetStatus::Warn);
        assert_eq!(s2, PerfBudgetStatus::Critical);
        assert_eq!(sample.over_warn_count, 2);
        assert_eq!(sample.over_critical_count, 1);
    }

    #[test]
    fn env_override_invalid_values_fall_back_to_defaults() {
        let defaults = PerfBudgetConfig::default();
        let cfg = PerfBudgetConfig::from_lookup(|key| match key {
            "ROLLOUT_BUDGET_FRAME_WARN_MS" => Some("invalid".to_string()),
            "ROLLOUT_BUDGET_FRAME_CRIT_MS" => Some("-5".to_string()),
            "ROLLOUT_BUDGET_DRAW_WARN_MS" => Some("12".to_string()),
            "ROLLOUT_BUDGET_DRAW_CRIT_MS" => Some("11".to_string()),
            _ => None,
        });

        assert_eq!(cfg.frame_total, defaults.frame_total);
        assert_eq!(cfg.draw, defaults.draw);
    }

    #[test]
    fn env_override_valid_values_are_applied() {
        let cfg = PerfBudgetConfig::from_lookup(|key| match key {
            "ROLLOUT_BUDGET_ENGINE_WARN_MS" => Some("7.5".to_string()),
            "ROLLOUT_BUDGET_ENGINE_CRIT_MS" => Some("11.0".to_string()),
            _ => None,
        });
        assert_eq!(cfg.engine_total, PerfBudgetThreshold::new(7.5, 11.0));
    }

    #[test]
    fn health_tracks_percentages_and_consecutive_critical_frames() {
        let mut health = PerfBudgetHealth::default();
        health.observe_summary(summarize_statuses([
            PerfBudgetStatus::Warn,
            PerfBudgetStatus::Ok,
        ]));
        health.observe_summary(summarize_statuses([
            PerfBudgetStatus::Critical,
            PerfBudgetStatus::Ok,
        ]));
        health.observe_summary(summarize_statuses([
            PerfBudgetStatus::Critical,
            PerfBudgetStatus::Warn,
        ]));
        health.observe_summary(summarize_statuses([
            PerfBudgetStatus::Ok,
            PerfBudgetStatus::Ok,
        ]));

        assert_eq!(health.total_frames, 4);
        assert_eq!(health.over_warn_frames, 3);
        assert_eq!(health.over_critical_frames, 2);
        assert_eq!(health.consecutive_critical_frames, 0);
        assert_eq!(health.max_consecutive_critical_frames, 2);
        assert_eq!(health.warn_pct(), 75.0);
        assert_eq!(health.critical_pct(), 50.0);
    }
}
