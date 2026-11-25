//! Minimal UI layout primitives.
//!
//! This module is intentionally small and dependency-free: it provides a `Rect` type plus
//! a few helpers for common 2D layout tasks (padding/insets + anchored placement).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_size(w: u32, h: u32) -> Self {
        Self { x: 0, y: 0, w, h }
    }

    pub fn size(&self) -> Size {
        Size {
            w: self.w,
            h: self.h,
        }
    }

    pub fn contains(&self, px: u32, py: u32) -> bool {
        px >= self.x
            && px < self.x.saturating_add(self.w)
            && py >= self.y
            && py < self.y.saturating_add(self.h)
    }

    /// Returns the rectangle inset by `insets` (i.e. the inner content area).
    ///
    /// If insets exceed the rect size, the resulting width/height will saturate to 0.
    pub fn inset(&self, insets: Insets) -> Self {
        let w = self
            .w
            .saturating_sub(insets.left.saturating_add(insets.right));
        let h = self
            .h
            .saturating_sub(insets.top.saturating_add(insets.bottom));
        Self {
            x: self.x.saturating_add(insets.left),
            y: self.y.saturating_add(insets.top),
            w,
            h,
        }
    }

    /// Places a child of `size` inside this rect using the requested `anchor`.
    ///
    /// If `size` exceeds this rect, it is clamped to fit.
    pub fn place(&self, size: Size, anchor: Anchor) -> Self {
        let w = size.w.min(self.w);
        let h = size.h.min(self.h);

        let x = match anchor {
            Anchor::TopLeft | Anchor::CenterLeft | Anchor::BottomLeft => self.x,
            Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => {
                self.x.saturating_add(self.w.saturating_sub(w) / 2)
            }
            Anchor::TopRight | Anchor::CenterRight | Anchor::BottomRight => {
                self.x.saturating_add(self.w.saturating_sub(w))
            }
        };

        let y = match anchor {
            Anchor::TopLeft | Anchor::TopCenter | Anchor::TopRight => self.y,
            Anchor::CenterLeft | Anchor::Center | Anchor::CenterRight => {
                self.y.saturating_add(self.h.saturating_sub(h) / 2)
            }
            Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => {
                self.y.saturating_add(self.h.saturating_sub(h))
            }
        };

        Self { x, y, w, h }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn clamp_max(self, max: Size) -> Self {
        Self {
            w: self.w.min(max.w),
            h: self.h.min(max.h),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Insets {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Insets {
    pub const ZERO: Insets = Insets {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    pub fn all(v: u32) -> Self {
        Self {
            left: v,
            top: v,
            right: v,
            bottom: v,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// Clip a screen-space rect to a viewport, returning the visible intersection.
pub fn clip_rect_to_viewport(rect: Rect, viewport: Rect) -> Option<Rect> {
    clip_rect_i32_to_viewport(rect.x as i32, rect.y as i32, rect.w, rect.h, viewport)
}

/// Clip an `i32`-positioned rect to a viewport, returning the visible intersection.
///
/// This is useful when world-space content can be temporarily translated outside the
/// viewport bounds (for example camera impulses) but must still render with fixed
/// viewport clipping.
pub fn clip_rect_i32_to_viewport(x: i32, y: i32, w: u32, h: u32, viewport: Rect) -> Option<Rect> {
    if w == 0 || h == 0 || viewport.w == 0 || viewport.h == 0 {
        return None;
    }

    let vx0 = viewport.x.min(i32::MAX as u32) as i32;
    let vy0 = viewport.y.min(i32::MAX as u32) as i32;
    let vx1 = viewport.x.saturating_add(viewport.w).min(i32::MAX as u32) as i32;
    let vy1 = viewport.y.saturating_add(viewport.h).min(i32::MAX as u32) as i32;

    let w_i32 = w.min(i32::MAX as u32) as i32;
    let h_i32 = h.min(i32::MAX as u32) as i32;
    let x1 = x.saturating_add(w_i32);
    let y1 = y.saturating_add(h_i32);

    let clipped_x0 = x.max(vx0);
    let clipped_y0 = y.max(vy0);
    let clipped_x1 = x1.min(vx1);
    let clipped_y1 = y1.min(vy1);
    if clipped_x1 <= clipped_x0 || clipped_y1 <= clipped_y0 {
        return None;
    }

    Some(Rect::new(
        clipped_x0 as u32,
        clipped_y0 as u32,
        (clipped_x1 - clipped_x0) as u32,
        (clipped_y1 - clipped_y0) as u32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inset_shrinks_rect_and_moves_origin() {
        let r = Rect::from_size(100, 80);
        let inner = r.inset(Insets::all(10));
        assert_eq!(inner, Rect::new(10, 10, 80, 60));
    }

    #[test]
    fn place_center_positions_child_in_parent() {
        let parent = Rect::from_size(100, 100);
        let child = parent.place(Size::new(20, 10), Anchor::Center);
        assert_eq!(child, Rect::new(40, 45, 20, 10));
    }

    #[test]
    fn place_bottom_center_positions_child_at_bottom() {
        let parent = Rect::from_size(100, 100);
        let child = parent.place(Size::new(20, 10), Anchor::BottomCenter);
        assert_eq!(child, Rect::new(40, 90, 20, 10));
    }

    #[test]
    fn place_clamps_size_to_parent() {
        let parent = Rect::from_size(50, 40);
        let child = parent.place(Size::new(999, 999), Anchor::TopLeft);
        assert_eq!(child, Rect::new(0, 0, 50, 40));
    }

    #[test]
    fn clip_rect_to_viewport_returns_intersection() {
        let rect = Rect::new(12, 20, 40, 30);
        let viewport = Rect::new(24, 10, 20, 20);
        let clipped = clip_rect_to_viewport(rect, viewport);
        assert_eq!(clipped, Some(Rect::new(24, 20, 20, 10)));
    }

    #[test]
    fn clip_rect_i32_to_viewport_handles_negative_origin() {
        let viewport = Rect::new(10, 10, 20, 20);
        let clipped = clip_rect_i32_to_viewport(-8, 18, 20, 8, viewport);
        assert_eq!(clipped, Some(Rect::new(10, 18, 2, 8)));
    }
}
