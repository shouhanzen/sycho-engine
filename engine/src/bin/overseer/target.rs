use super::board::{Board, Bounds, Panel, Point, WindowId};
use engine::surface::SurfaceSize;

#[derive(Clone, PartialEq)]
pub(super) struct Menu {
    pub bounds: Bounds,
    pub rows: Vec<Choice>,
    pub offset: usize,
    pub visible: usize,
    pub selected: Option<WindowId>,
}

#[derive(Clone, PartialEq)]
pub(super) struct Choice {
    pub target: Option<WindowId>,
    pub label: String,
}

impl Menu {
    const ROW_HEIGHT: f64 = 27.0;

    pub fn new(board: &Board, source: WindowId, size: SurfaceSize, offset: usize) -> Option<Self> {
        let window = board.window(source)?;
        let anchor = board.project(window.target_button()?);
        let mut rows = vec![Choice {
            target: None,
            label: "Detach".to_owned(),
        }];
        rows.extend(
            board
                .windows()
                .rev()
                .filter(|window| window.kind() == Panel::Game)
                .map(|window| {
                    let title = window.title();
                    let label = if window.project().is_none() {
                        format!("{title} · {}", window.id)
                    } else {
                        board.window_label(window)
                    };
                    Choice {
                        target: Some(window.id),
                        label,
                    }
                }),
        );
        let screen_width = f64::from(size.width);
        let screen_height = f64::from(size.height);
        let width = anchor
            .w
            .clamp(176.0, 320.0)
            .min((screen_width - 12.0).max(1.0));
        let below = (screen_height - anchor.bottom() - 12.0).max(0.0);
        let above = (anchor.y - 12.0).max(0.0);
        let wanted = rows.len().min(8);
        let opens_below = below >= wanted as f64 * Self::ROW_HEIGHT || below >= above;
        let available = if opens_below { below } else { above };
        let visible = wanted.min((available / Self::ROW_HEIGHT).floor().max(1.0) as usize);
        let height = visible as f64 * Self::ROW_HEIGHT + 8.0;
        let y = if opens_below {
            anchor.bottom() + 4.0
        } else {
            anchor.y - height - 4.0
        };
        Some(Self {
            bounds: Bounds::new(
                anchor.x.clamp(6.0, (screen_width - width - 6.0).max(6.0)),
                y.clamp(6.0, (screen_height - height - 6.0).max(6.0)),
                width,
                height,
            ),
            offset: offset.min(rows.len() - visible),
            rows,
            visible,
            selected: window.attached_game(),
        })
    }

    pub fn row(&self, index: usize) -> Bounds {
        Bounds::new(
            self.bounds.x + 4.0,
            self.bounds.y + 4.0 + index as f64 * Self::ROW_HEIGHT,
            self.bounds.w - 8.0,
            Self::ROW_HEIGHT,
        )
    }

    pub fn choice_at(&self, point: Point) -> Option<Option<WindowId>> {
        (0..self.visible)
            .find(|&index| self.row(index).contains(point))
            .map(|index| self.rows[self.offset + index].target)
    }

    pub fn max_offset(&self) -> usize {
        self.rows.len() - self.visible
    }
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    pub source: WindowId,
    pub pointer: Point,
    pub hovered: Option<WindowId>,
    pub phase: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_stays_near_the_header_and_selects_by_window_id() {
        let mut board = Board::default();
        let size = SurfaceSize::new(480, 390);
        let view = super::super::board::viewport(size);
        let game = board.add(Panel::Game, view);
        let profiler = board.add(Panel::Profiler, view);
        board.set_bounds(profiler, Bounds::new(310.0, 340.0, 500.0, 390.0));
        board.attach_target(profiler, Some(game));
        let menu = Menu::new(&board, profiler, size, 0).unwrap();
        let button = board.project(board.window(profiler).unwrap().target_button().unwrap());
        assert!(menu.bounds.bottom() <= button.y);
        assert!(menu.bounds.right() <= view.right());
        assert_eq!(menu.selected, Some(game));
        let pick = |index| {
            let row = menu.row(index);
            menu.choice_at(Point::new(row.x + 8.0, row.y + 8.0))
        };
        assert_eq!(pick(0), Some(None));
        assert_eq!(pick(1), Some(Some(game)));
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct HeaderMenu {
    pub source: WindowId,
    pub bounds: Bounds,
    pub pending: bool,
}

impl HeaderMenu {
    pub fn new(source: WindowId, point: Point, size: SurfaceSize) -> Self {
        let w = 144.0_f64.min(f64::from(size.width));
        let h = 34.0_f64.min(f64::from(size.height));
        let x = point.x.clamp(0.0, (f64::from(size.width) - w).max(0.0));
        let y = point.y.clamp(0.0, (f64::from(size.height) - h).max(0.0));
        Self {
            source,
            bounds: Bounds::new(x, y, w, h),
            pending: false,
        }
    }
}
