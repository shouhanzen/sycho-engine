use std::path::Path;

use engine::embedded::TimelineStatus;
use engine::surface::SurfaceSize;
use engine::timeline::{Bookmark, BookmarkLedger};

use super::board::{Bounds, WindowId};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Popup {
    pub window: WindowId,
    pub game: WindowId,
    pub name: String,
    pub offset: usize,
    pub entries: Vec<Bookmark>,
    pub error: Option<String>,
    pub selected: Option<u64>,
    pub connected: bool,
    pub can_save: bool,
    revision: Option<u64>,
}

impl Popup {
    pub fn new(window: WindowId, game: WindowId) -> Self {
        Self {
            window,
            game,
            name: String::new(),
            offset: 0,
            entries: Vec::new(),
            error: None,
            selected: None,
            connected: false,
            can_save: false,
            revision: None,
        }
    }

    pub fn sync(&mut self, directory: &Path, status: TimelineStatus, connected: bool) {
        self.selected = status.detached_bookmark;
        self.connected = connected;
        self.can_save = connected && status.cursor.is_some() && self.selected.is_none();
        if self.revision.is_none() || connected && self.revision != Some(status.bookmark_revision) {
            match BookmarkLedger::read(directory) {
                Ok(ledger) => {
                    self.revision = Some(ledger.revision());
                    self.entries = ledger.entries().iter().rev().cloned().collect();
                    self.error = None;
                }
                Err(err) => {
                    self.revision = Some(status.bookmark_revision);
                    self.error = Some(err.to_string());
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layout {
    pub bounds: Bounds,
    pub name: Bounds,
    pub save: Bounds,
    pub visible: usize,
}

impl Layout {
    const ROW: f64 = 29.0;

    pub fn new(anchor: Bounds, size: SurfaceSize, count: usize) -> Self {
        let screen_width = f64::from(size.width);
        let screen_height = f64::from(size.height);
        let width = 324.0_f64.min((screen_width - 12.0).max(1.0));
        let above = (anchor.y - 12.0).max(0.0);
        let below = (screen_height - anchor.bottom() - 12.0).max(0.0);
        let desired = count.clamp(1, 7);
        let opens_above = above >= 42.0 + desired as f64 * Self::ROW || above >= below;
        let room = if opens_above { above } else { below };
        let rows = desired.min(((room - 42.0) / Self::ROW).floor().max(1.0) as usize);
        let height = 42.0 + rows as f64 * Self::ROW;
        let x = anchor.x.clamp(6.0, (screen_width - width - 6.0).max(6.0));
        let y = if opens_above {
            anchor.y - height - 4.0
        } else {
            anchor.bottom() + 4.0
        };
        let y = y.clamp(6.0, (screen_height - height - 6.0).max(6.0));
        Self {
            bounds: Bounds::new(x, y, width, height),
            name: Bounds::new(x + 7.0, y + 7.0, (width - 77.0).max(0.0), 27.0),
            save: Bounds::new(x + width - 64.0, y + 7.0, 57.0, 27.0),
            visible: rows,
        }
    }

    pub fn row(self, index: usize) -> Bounds {
        Bounds::new(
            self.bounds.x + 7.0,
            self.bounds.y + 36.0 + index as f64 * Self::ROW,
            self.bounds.w - 14.0,
            Self::ROW,
        )
    }

    pub fn remove(self, index: usize) -> Bounds {
        let row = self.row(index);
        Bounds::new(row.right() - 25.0, row.y + 3.0, 24.0, 23.0)
    }

    pub fn offset(self, popup: &Popup) -> usize {
        popup
            .offset
            .min(popup.entries.len().saturating_sub(self.visible))
    }
}
