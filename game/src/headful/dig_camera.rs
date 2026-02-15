use std::time::Duration;

const DISABLE_DIG_CAMERA_ENV: &str = "ROLLOUT_DISABLE_DIG_CAMERA";
const STEP_PX_PER_ROW_ENV: &str = "ROLLOUT_DIG_CAMERA_STEP_PX_PER_ROW";
const MAX_OFFSET_PX_ENV: &str = "ROLLOUT_DIG_CAMERA_MAX_OFFSET_PX";
const RETURN_PX_PER_S_ENV: &str = "ROLLOUT_DIG_CAMERA_RETURN_PX_PER_S";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DigCameraConfig {
    pub step_px_per_row: f32,
    pub max_offset_px: f32,
    pub return_px_per_s: f32,
}

impl Default for DigCameraConfig {
    fn default() -> Self {
        Self {
            step_px_per_row: 11.0,
            max_offset_px: 72.0,
            return_px_per_s: 96.0,
        }
    }
}

impl DigCameraConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(v) = env_f32(STEP_PX_PER_ROW_ENV) {
            config.step_px_per_row = v.clamp(0.0, 256.0);
        }
        if let Some(v) = env_f32(MAX_OFFSET_PX_ENV) {
            config.max_offset_px = v.clamp(0.0, 2048.0);
        }
        if let Some(v) = env_f32(RETURN_PX_PER_S_ENV) {
            config.return_px_per_s = v.clamp(0.0, 4096.0);
        }
        config
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DigCameraController {
    enabled: bool,
    last_depth_rows: u32,
    offset_y_px: f32,
    config: DigCameraConfig,
}

impl DigCameraController {
    pub fn from_env() -> Self {
        Self::new_with_config(
            !env_flag(DISABLE_DIG_CAMERA_ENV),
            DigCameraConfig::from_env(),
        )
    }

    pub fn new_with_config(enabled: bool, config: DigCameraConfig) -> Self {
        Self {
            enabled,
            last_depth_rows: 0,
            offset_y_px: 0.0,
            config,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn last_depth_rows(&self) -> u32 {
        self.last_depth_rows
    }

    pub fn offset_y_px(&self) -> f32 {
        self.offset_y_px
    }

    pub fn reset(&mut self, depth_rows: u32) {
        self.last_depth_rows = depth_rows;
        self.offset_y_px = 0.0;
    }

    pub fn update(&mut self, depth_rows: u32, dt: Duration, paused: bool) {
        if !self.enabled {
            self.last_depth_rows = depth_rows;
            self.offset_y_px = 0.0;
            return;
        }

        if paused {
            // Keep camera frozen while paused, and sync baseline depth so resume
            // does not apply stale/queued impulses.
            self.last_depth_rows = depth_rows;
            return;
        }

        if depth_rows > self.last_depth_rows {
            let delta = depth_rows - self.last_depth_rows;
            self.offset_y_px += delta as f32 * self.config.step_px_per_row;
        }
        self.last_depth_rows = depth_rows;
        self.offset_y_px = self.offset_y_px.clamp(0.0, self.config.max_offset_px);

        let settle_amount = self.config.return_px_per_s * dt.as_secs_f32();
        if settle_amount > 0.0 {
            self.offset_y_px = (self.offset_y_px - settle_amount).max(0.0);
        }
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f32>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_approx_eq(actual: f32, expected: f32) {
        let delta = (actual - expected).abs();
        assert!(
            delta < 0.001,
            "expected ~{expected}, got {actual} (delta={delta})"
        );
    }

    #[test]
    fn depth_delta_creates_impulse() {
        let mut controller = DigCameraController::new_with_config(
            true,
            DigCameraConfig {
                step_px_per_row: 10.0,
                max_offset_px: 200.0,
                return_px_per_s: 0.0,
            },
        );
        controller.reset(5);

        controller.update(7, Duration::ZERO, false);

        assert_eq!(controller.last_depth_rows(), 7);
        assert_approx_eq(controller.offset_y_px(), 20.0);
    }

    #[test]
    fn no_depth_delta_creates_no_impulse() {
        let mut controller = DigCameraController::new_with_config(
            true,
            DigCameraConfig {
                step_px_per_row: 10.0,
                max_offset_px: 200.0,
                return_px_per_s: 0.0,
            },
        );
        controller.reset(7);

        controller.update(7, Duration::from_millis(16), false);

        assert_eq!(controller.last_depth_rows(), 7);
        assert_approx_eq(controller.offset_y_px(), 0.0);
    }

    #[test]
    fn settle_rate_approaches_zero() {
        let mut controller = DigCameraController::new_with_config(
            true,
            DigCameraConfig {
                step_px_per_row: 10.0,
                max_offset_px: 200.0,
                return_px_per_s: 20.0,
            },
        );
        controller.reset(0);
        controller.update(3, Duration::ZERO, false);
        assert_approx_eq(controller.offset_y_px(), 30.0);

        controller.update(3, Duration::from_millis(500), false);
        assert_approx_eq(controller.offset_y_px(), 20.0);

        controller.update(3, Duration::from_secs(2), false);
        assert_approx_eq(controller.offset_y_px(), 0.0);
    }

    #[test]
    fn max_clamp_prevents_runaway_offset() {
        let mut controller = DigCameraController::new_with_config(
            true,
            DigCameraConfig {
                step_px_per_row: 100.0,
                max_offset_px: 40.0,
                return_px_per_s: 0.0,
            },
        );
        controller.reset(0);

        controller.update(1, Duration::ZERO, false);

        assert_approx_eq(controller.offset_y_px(), 40.0);
    }

    #[test]
    fn reset_clears_camera_state() {
        let mut controller = DigCameraController::new_with_config(
            true,
            DigCameraConfig {
                step_px_per_row: 10.0,
                max_offset_px: 200.0,
                return_px_per_s: 0.0,
            },
        );
        controller.reset(0);
        controller.update(2, Duration::ZERO, false);
        assert!(controller.offset_y_px() > 0.0);

        controller.reset(12);

        assert_eq!(controller.last_depth_rows(), 12);
        assert_approx_eq(controller.offset_y_px(), 0.0);
    }
}
