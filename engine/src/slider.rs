use crate::ui::Rect;

/// Lightweight slider element: geometry + value mapping.
///
/// Rendering and input orchestration stay in callers, while this type provides
/// deterministic math for converting pointer positions into values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slider {
    pub track: Rect,
    pub min: f32,
    pub max: f32,
    pub value: f32,
}

impl Slider {
    pub fn new(track: Rect, min: f32, max: f32, value: f32) -> Self {
        let (min, max) = ordered_range(min, max);
        let value = clamp_f32(value, min, max);
        Self {
            track,
            min,
            max,
            value,
        }
    }

    pub fn normalized_value(&self) -> f32 {
        if self.track.w == 0 || (self.max - self.min).abs() <= f32::EPSILON {
            0.0
        } else {
            ((self.value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        }
    }

    pub fn set_value(&mut self, value: f32) {
        self.value = clamp_f32(value, self.min, self.max);
    }

    pub fn value_from_normalized(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        self.min + (self.max - self.min) * t
    }

    pub fn value_from_x(&self, x: u32) -> f32 {
        if self.track.w <= 1 {
            return self.min;
        }
        let left = self.track.x;
        let right = self.track.x.saturating_add(self.track.w.saturating_sub(1));
        let clamped_x = x.clamp(left, right);
        let local = clamped_x.saturating_sub(left);
        let t = local as f32 / (self.track.w.saturating_sub(1)) as f32;
        self.value_from_normalized(t)
    }

    pub fn set_value_from_x(&mut self, x: u32) {
        self.value = self.value_from_x(x);
    }

    pub fn thumb_center_x(&self) -> u32 {
        if self.track.w == 0 {
            return self.track.x;
        }
        let t = self.normalized_value();
        self.track
            .x
            .saturating_add(((self.track.w.saturating_sub(1)) as f32 * t).round() as u32)
    }

    pub fn thumb_rect(&self, thumb_w: u32, thumb_h: u32) -> Rect {
        let thumb_w = thumb_w.max(1).min(self.track.w.max(1));
        let thumb_h = thumb_h.max(1);
        let cx = self.thumb_center_x();
        let half = thumb_w / 2;
        let x = cx.saturating_sub(half).min(
            self.track
                .x
                .saturating_add(self.track.w.saturating_sub(thumb_w)),
        );
        let y = if thumb_h > self.track.h {
            self.track.y.saturating_sub((thumb_h - self.track.h) / 2)
        } else {
            self.track.y.saturating_add((self.track.h - thumb_h) / 2)
        };
        Rect::new(x, y, thumb_w, thumb_h)
    }
}

fn ordered_range(a: f32, b: f32) -> (f32, f32) {
    if a <= b { (a, b) } else { (b, a) }
}

fn clamp_f32(v: f32, min: f32, max: f32) -> f32 {
    if v < min {
        min
    } else if v > max {
        max
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_from_x_clamps_to_range() {
        let slider = Slider::new(Rect::new(10, 20, 100, 8), 0.0, 1.0, 0.5);
        assert!((slider.value_from_x(0) - 0.0).abs() < 1e-6);
        assert!((slider.value_from_x(999) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn value_from_x_maps_linearly() {
        let slider = Slider::new(Rect::new(100, 0, 101, 8), 0.0, 1.0, 0.0);
        let mid = slider.value_from_x(150);
        assert!((mid - 0.5).abs() < 0.01, "expected ~0.5, got {mid}");
    }

    #[test]
    fn thumb_rect_tracks_value() {
        let mut slider = Slider::new(Rect::new(0, 0, 100, 6), 0.0, 1.0, 0.0);
        let left = slider.thumb_rect(10, 14).x;
        slider.set_value(1.0);
        let right = slider.thumb_rect(10, 14).x;
        assert!(right > left);
    }
}
