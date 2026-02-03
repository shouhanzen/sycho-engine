use engine::render::{color_for_cell, draw_board, CELL_SIZE};
use engine::ui as ui;

use crate::debug::{draw_text, draw_text_scaled};
use crate::tetris_core::{piece_board_offset, piece_grid, piece_type, Piece, TetrisCore, Vec2i};

const COLOR_PANEL_BG: [u8; 4] = [16, 16, 22, 255];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];
const COLOR_PANEL_BORDER_DISABLED: [u8; 4] = [28, 28, 38, 255];
const BUTTON_HOVER_BRIGHTEN: f32 = 0.12;

pub const MAIN_MENU_TITLE: &str = "UNTITLED";

const PAUSE_BUTTON_SIZE: u32 = 44;
const PAUSE_BUTTON_MARGIN: u32 = 12;
const COLOR_PAUSE_ICON: [u8; 4] = [235, 235, 245, 255];

const COLOR_PAUSE_MENU_TEXT: [u8; 4] = [235, 235, 245, 255];
const COLOR_PAUSE_MENU_DIM: [u8; 4] = [0, 0, 0, 255];
const PAUSE_MENU_DIM_ALPHA: u8 = 170;
const COLOR_PAUSE_MENU_BG: [u8; 4] = [10, 10, 14, 255];
const COLOR_PAUSE_MENU_BORDER: [u8; 4] = [40, 40, 55, 255];

const PANEL_MARGIN: u32 = 16;
const PANEL_PADDING: u32 = 12;

const PREVIEW_GRID: u32 = 4;
const PREVIEW_CELL: u32 = 16;
const PREVIEW_SIZE: u32 = PREVIEW_GRID * PREVIEW_CELL;
const PREVIEW_GAP_Y: u32 = 10;

const GHOST_ALPHA: u8 = 80;

pub type Rect = ui::Rect;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiLayout {
    pub board: Rect,
    pub hold_panel: Rect,
    pub next_panel: Rect,
    pub pause_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuLayout {
    pub panel: Rect,
    pub resume_button: Rect,
    pub end_run_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MainMenuLayout {
    pub panel: Rect,
    pub start_button: Rect,
    pub quit_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GameOverMenuLayout {
    pub panel: Rect,
    pub restart_button: Rect,
    pub skilltree_button: Rect,
    pub quit_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkillTreeLayout {
    pub panel: Rect,
    pub start_new_game_button: Rect,
}

pub fn compute_layout(width: u32, height: u32, board_w: u32, board_h: u32, next_len: usize) -> UiLayout {
    let board_pixel_width = board_w.saturating_mul(CELL_SIZE);
    let board_pixel_height = board_h.saturating_mul(CELL_SIZE);
    let board_x = width.saturating_sub(board_pixel_width) / 2;
    let board_y = height.saturating_sub(board_pixel_height) / 2;

    let board = Rect {
        x: board_x,
        y: board_y,
        w: board_pixel_width,
        h: board_pixel_height,
    };

    let panel_w = (PREVIEW_SIZE + PANEL_PADDING * 2).min(width);
    let hold_h = (PREVIEW_SIZE + PANEL_PADDING * 2).min(height);

    // Next panel height depends on queue length.
    let next_h_content = (next_len as u32)
        .saturating_mul(PREVIEW_SIZE)
        .saturating_add((next_len as u32).saturating_sub(1).saturating_mul(PREVIEW_GAP_Y));
    let next_h = (next_h_content + PANEL_PADDING * 2).min(height);

    // Prefer hold on the left of the board, next on the right. If there isn't space,
    // fall back to the opposite side.
    let space_left = board_x;
    let space_right = width.saturating_sub(board_x.saturating_add(board_pixel_width));

    let mut hold_x = 0;
    if space_left >= panel_w.saturating_add(PANEL_MARGIN) {
        hold_x = board_x.saturating_sub(PANEL_MARGIN + panel_w);
    } else if space_right >= panel_w.saturating_add(PANEL_MARGIN) {
        hold_x = board_x.saturating_add(board_pixel_width + PANEL_MARGIN);
    }

    let mut next_x = 0;
    if space_right >= panel_w.saturating_add(PANEL_MARGIN) {
        next_x = board_x.saturating_add(board_pixel_width + PANEL_MARGIN);
    } else if space_left >= panel_w.saturating_add(PANEL_MARGIN) {
        next_x = board_x.saturating_sub(PANEL_MARGIN + panel_w);
    }

    let hold_panel = Rect {
        x: hold_x,
        y: board_y,
        w: panel_w,
        h: hold_h,
    };

    let next_panel = Rect {
        x: next_x,
        y: board_y,
        w: panel_w,
        h: next_h.min(board_pixel_height),
    };

    let pause_size = PAUSE_BUTTON_SIZE.min(width).min(height);
    let pause_button = Rect {
        x: width.saturating_sub(PAUSE_BUTTON_MARGIN.saturating_add(pause_size)),
        y: PAUSE_BUTTON_MARGIN.min(height.saturating_sub(pause_size)),
        w: pause_size,
        h: pause_size,
    };

    UiLayout {
        board,
        hold_panel,
        next_panel,
        pause_button,
    }
}

pub fn draw_tetris_world(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
) -> UiLayout {
    let board = state.board();
    let board_h = board.len() as u32;
    let board_w = board.first().map(|r| r.len()).unwrap_or(0) as u32;
    let layout = compute_layout(width, height, board_w, board_h, state.next_queue().len());

    draw_board(frame, board);

    draw_ghost_and_active_piece(frame, width, height, layout.board, board_w, board_h, state);

    layout
}

pub fn draw_tetris_hud(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
) {
    draw_tetris_hud_with_cursor(frame, width, height, state, layout, None);
}

pub fn draw_tetris_hud_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
    cursor: Option<(u32, u32)>,
) {
    draw_hold_panel(
        frame,
        width,
        height,
        layout.hold_panel,
        state.held_piece(),
        state.can_hold(),
    );
    draw_next_panel(frame, width, height, layout.next_panel, state.next_queue());

    let pause_hovered = cursor
        .map(|(x, y)| layout.pause_button.contains(x, y))
        .unwrap_or(false);
    draw_pause_button(frame, width, height, layout.pause_button, pause_hovered);

    // Simple HUD: score + lines, placed near the top-right (left of the pause button).
    let hud_x = layout.pause_button.x.saturating_sub(180);
    let hud_y = layout.pause_button.y.saturating_add(6);
    let score_text = format!("SCORE {}", state.score());
    let lines_text = format!("LINES {}", state.lines_cleared());
    draw_text(frame, width, height, hud_x, hud_y, &score_text, COLOR_PAUSE_ICON);
    draw_text(
        frame,
        width,
        height,
        hud_x,
        hud_y.saturating_add(14),
        &lines_text,
        COLOR_PAUSE_ICON,
    );
}

pub fn draw_tetris(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
) -> UiLayout {
    let layout = draw_tetris_world(frame, width, height, state);
    draw_tetris_hud(frame, width, height, state, layout);
    layout
}

fn draw_pause_button(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    rect: Rect,
    hovered: bool,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    if rect.x >= width || rect.y >= height {
        return;
    }

    let (fill, border) = button_colors(hovered);
    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, fill);
    draw_rect_outline(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        border,
    );

    // Draw a simple pause icon: two vertical bars.
    let bar_w = (rect.w / 6).max(3).min(rect.w);
    let bar_h = (rect.h * 2 / 3).max(6).min(rect.h);
    let gap = (rect.w / 5).max(4);

    let icon_total_w = bar_w.saturating_mul(2).saturating_add(gap);
    let icon_x0 = rect.x + rect.w.saturating_sub(icon_total_w) / 2;
    let icon_y0 = rect.y + rect.h.saturating_sub(bar_h) / 2;

    fill_rect(frame, width, height, icon_x0, icon_y0, bar_w, bar_h, COLOR_PAUSE_ICON);
    fill_rect(
        frame,
        width,
        height,
        icon_x0.saturating_add(bar_w + gap),
        icon_y0,
        bar_w,
        bar_h,
        COLOR_PAUSE_ICON,
    );
}

pub fn draw_pause_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> PauseMenuLayout {
    draw_pause_menu_with_cursor(frame, width, height, None)
}

pub fn draw_pause_menu_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
) -> PauseMenuLayout {
    // Dim the entire game view.
    blend_rect(
        frame,
        width,
        height,
        0,
        0,
        width,
        height,
        COLOR_PAUSE_MENU_DIM,
        PAUSE_MENU_DIM_ALPHA,
    );

    let margin = 32u32;
    let pad = 18u32;

    // Layout is expressed via the engine UI layout helpers, then converted to our local `Rect`
    // for hit-testing and drawing.
    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return PauseMenuLayout::default();
    }

    let panel_size = ui::Size::new(360, 260).clamp_max(safe.size());
    if panel_size.w == 0 || panel_size.h == 0 {
        return PauseMenuLayout::default();
    }

    let panel_ui = safe.place(panel_size, ui::Anchor::Center);
    let panel = Rect {
        x: panel_ui.x,
        y: panel_ui.y,
        w: panel_ui.w,
        h: panel_ui.h,
    };

    fill_rect(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BG,
    );
    draw_rect_outline(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BORDER,
    );

    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad),
        "PAUSED",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 24),
        "ESC TO RESUME",
        COLOR_PAUSE_MENU_TEXT,
    );

    let content = panel_ui.inset(ui::Insets::all(pad));
    let gap = 12u32;
    let button_size = ui::Size::new(240, 44).clamp_max(content.size());
    let resume_ui = content.place(button_size, ui::Anchor::BottomCenter);
    let resume_button = Rect {
        x: resume_ui.x,
        y: resume_ui.y,
        w: resume_ui.w,
        h: resume_ui.h,
    };
    let end_run_button = Rect {
        x: resume_button.x,
        y: resume_button.y.saturating_sub(resume_button.h.saturating_add(gap)),
        w: resume_button.w,
        h: resume_button.h,
    };

    draw_button(
        frame,
        width,
        height,
        resume_button,
        "RESUME",
        cursor
            .map(|(x, y)| resume_button.contains(x, y))
            .unwrap_or(false),
    );
    draw_button(
        frame,
        width,
        height,
        end_run_button,
        "END RUN",
        cursor
            .map(|(x, y)| end_run_button.contains(x, y))
            .unwrap_or(false),
    );

    PauseMenuLayout {
        panel,
        resume_button,
        end_run_button,
    }
}

pub fn draw_main_menu(frame: &mut dyn Renderer2d, width: u32, height: u32) -> MainMenuLayout {
    draw_main_menu_with_cursor(frame, width, height, None)
}

pub fn draw_main_menu_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
) -> MainMenuLayout {
    // Main menu is its own scene: clear the frame so the Tetris board is not visible underneath.
    fill_rect(frame, width, height, 0, 0, width, height, color_for_cell(0));

    let margin = 32u32;
    let pad = 18u32;

    // "Scene bounds" used for layout/hit-testing (not a modal panel).
    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return MainMenuLayout::default();
    }

    let panel = Rect {
        x: safe.x,
        y: safe.y,
        w: safe.w,
        h: safe.h,
    };

    // Layout: a vertical stack (title, start, quit), centered in the safe region.
    let title = MAIN_MENU_TITLE;
    let title_chars = title.chars().count() as u32;
    let glyph_cols = 4u32; // 3 glyph columns + 1 column spacing (matches `draw_text` advances).
    let denom = title_chars.saturating_mul(glyph_cols).max(1);
    let max_scale = 12u32;
    let title_scale = (safe.w / denom).clamp(2, max_scale);
    let title_w = denom.saturating_mul(title_scale).min(safe.w);
    let title_h = (5u32).saturating_mul(title_scale).min(safe.h);

    let content = safe.inset(ui::Insets::all(pad));
    let button_size = ui::Size::new(240, 44).clamp_max(content.size());
    let button_gap = 12u32;
    let title_button_gap = 28u32;
    let stack_h = title_h
        .saturating_add(title_button_gap)
        .saturating_add(button_size.h.saturating_mul(2))
        .saturating_add(button_gap);
    let top_y = content
        .y
        .saturating_add(content.h.saturating_sub(stack_h) / 2);

    let title_x = content.x.saturating_add(content.w.saturating_sub(title_w) / 2);
    let title_y = top_y;
    draw_text_scaled(
        frame,
        width,
        height,
        title_x,
        title_y,
        title,
        COLOR_PAUSE_MENU_TEXT,
        title_scale,
    );

    let buttons_y = title_y
        .saturating_add(title_h)
        .saturating_add(title_button_gap);
    let start_button = Rect {
        x: content.x.saturating_add(content.w.saturating_sub(button_size.w) / 2),
        y: buttons_y,
        w: button_size.w,
        h: button_size.h,
    };
    let quit_button = Rect {
        x: start_button.x,
        y: start_button.y.saturating_add(start_button.h).saturating_add(button_gap),
        w: start_button.w,
        h: start_button.h,
    };

    for (rect, label) in [(start_button, "START"), (quit_button, "QUIT")] {
        let hovered = cursor.map(|(x, y)| rect.contains(x, y)).unwrap_or(false);
        draw_button(frame, width, height, rect, label, hovered);
    }

    MainMenuLayout {
        panel,
        start_button,
        quit_button,
    }
}

pub fn draw_game_over_menu(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
) -> GameOverMenuLayout {
    draw_game_over_menu_with_cursor(frame, width, height, None)
}

pub fn draw_game_over_menu_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
) -> GameOverMenuLayout {
    // Dim the entire game view.
    blend_rect(
        frame,
        width,
        height,
        0,
        0,
        width,
        height,
        COLOR_PAUSE_MENU_DIM,
        PAUSE_MENU_DIM_ALPHA,
    );

    let margin = 32u32;
    let pad = 18u32;

    let panel_w = 420u32.min(width.saturating_sub(margin.saturating_mul(2)));
    let panel_h = 280u32.min(height.saturating_sub(margin.saturating_mul(2)));
    if panel_w == 0 || panel_h == 0 {
        return GameOverMenuLayout::default();
    }

    let panel = Rect {
        x: width.saturating_sub(panel_w) / 2,
        y: height.saturating_sub(panel_h) / 2,
        w: panel_w,
        h: panel_h,
    };

    fill_rect(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BG,
    );
    draw_rect_outline(
        frame,
        width,
        height,
        panel.x,
        panel.y,
        panel.w,
        panel.h,
        COLOR_PAUSE_MENU_BORDER,
    );

    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad),
        "GAME OVER",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 24),
        "RUN ENDED",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 48),
        "ENTER TO RESTART",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 72),
        "K: SKILL TREE",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        panel.x.saturating_add(pad),
        panel.y.saturating_add(pad + 96),
        "ESC: MAIN MENU",
        COLOR_PAUSE_MENU_TEXT,
    );

    let button_h = 44u32.min(panel.h.saturating_sub(pad.saturating_mul(2)));
    let button_w = 240u32.min(panel.w.saturating_sub(pad.saturating_mul(2)));
    let gap = 12u32;
    let buttons_total_h = button_h
        .saturating_mul(3)
        .saturating_add(gap.saturating_mul(2));
    let top_y = panel
        .y
        .saturating_add(panel.h.saturating_sub(pad.saturating_add(buttons_total_h)));

    let restart_button = Rect {
        x: panel.x.saturating_add(panel.w.saturating_sub(button_w) / 2),
        y: top_y,
        w: button_w,
        h: button_h,
    };
    let skilltree_button = Rect {
        x: restart_button.x,
        y: restart_button.y.saturating_add(button_h + gap),
        w: button_w,
        h: button_h,
    };
    let quit_button = Rect {
        x: restart_button.x,
        y: skilltree_button.y.saturating_add(button_h + gap),
        w: button_w,
        h: button_h,
    };

    for (rect, label) in [
        (restart_button, "RESTART"),
        (skilltree_button, "SKILL TREE"),
        (quit_button, "QUIT"),
    ] {
        let hovered = cursor.map(|(x, y)| rect.contains(x, y)).unwrap_or(false);
        draw_button(frame, width, height, rect, label, hovered);
    }

    GameOverMenuLayout {
        panel,
        restart_button,
        skilltree_button,
        quit_button,
    }
}

pub fn draw_skilltree(frame: &mut dyn Renderer2d, width: u32, height: u32) -> SkillTreeLayout {
    draw_skilltree_with_cursor(frame, width, height, None)
}

pub fn draw_skilltree_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
) -> SkillTreeLayout {
    // Skilltree is its own scene: clear the frame so the Tetris board is not visible.
    fill_rect(frame, width, height, 0, 0, width, height, color_for_cell(0));

    let margin = 32u32;
    let pad = 18u32;

    let screen = ui::Rect::from_size(width, height);
    let safe = screen.inset(ui::Insets::all(margin));
    if safe.w == 0 || safe.h == 0 {
        return SkillTreeLayout::default();
    }

    // Use the safe region as our "world bounds" for this scene (not a floating modal panel).
    let panel = Rect {
        x: safe.x,
        y: safe.y,
        w: safe.w,
        h: safe.h,
    };

    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad),
        "SKILL TREE",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad + 24),
        "TODO: add progression nodes",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad + 48),
        "ENTER: START NEW RUN",
        COLOR_PAUSE_MENU_TEXT,
    );
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad + 72),
        "ESC: MAIN MENU",
        COLOR_PAUSE_MENU_TEXT,
    );

    // Placeholder "nodes" so the skilltree reads like an in-world scene rather than a modal panel.
    let content = safe.inset(ui::Insets::all(pad));
    let node_size = ui::Size::new(140, 64).clamp_max(content.size());
    if node_size.w > 0 && node_size.h > 0 {
        let nodes_band = ui::Rect::new(content.x, content.y.saturating_add(120), content.w, content.h);
        let mut x = nodes_band
            .x
            .saturating_add(nodes_band.w.saturating_sub(node_size.w.saturating_mul(3)) / 2);
        let y = nodes_band.y;
        let gap = 24u32;

        for label in ["+SCORE", "+TIME", "+SPEED"] {
            let r = Rect {
                x,
                y,
                w: node_size.w,
                h: node_size.h,
            };
            fill_rect(frame, width, height, r.x, r.y, r.w, r.h, COLOR_PANEL_BG);
            draw_rect_outline(frame, width, height, r.x, r.y, r.w, r.h, COLOR_PANEL_BORDER);
            draw_text(
                frame,
                width,
                height,
                r.x.saturating_add(16),
                r.y.saturating_add(r.h / 2).saturating_sub(6),
                label,
                COLOR_PAUSE_MENU_TEXT,
            );
            x = x.saturating_add(node_size.w.saturating_add(gap));
        }
    }

    let button_size = ui::Size::new(240, 44).clamp_max(content.size());
    let start_ui = content.place(button_size, ui::Anchor::BottomCenter);
    let start_new_game_button = Rect {
        x: start_ui.x,
        y: start_ui.y,
        w: start_ui.w,
        h: start_ui.h,
    };

    let hovered = cursor
        .map(|(x, y)| start_new_game_button.contains(x, y))
        .unwrap_or(false);
    draw_button(
        frame,
        width,
        height,
        start_new_game_button,
        "START NEW RUN",
        hovered,
    );

    SkillTreeLayout {
        panel,
        start_new_game_button,
    }
}

#[derive(Debug, Clone, Copy)]
enum PieceDrawStyle {
    Solid,
    Ghost,
}

fn draw_ghost_and_active_piece(
    frame: &mut [u8],
    width: u32,
    height: u32,
    board_rect: Rect,
    board_w: u32,
    board_h: u32,
    state: &TetrisCore,
) {
    let Some(piece) = state.current_piece() else {
        return;
    };

    let rotation = state.current_piece_rotation();

    // Ghost should render behind the active piece.
    if let Some(ghost_pos) = state.ghost_piece_pos() {
        draw_piece_on_board(
            frame,
            width,
            height,
            board_rect,
            board_w,
            board_h,
            piece,
            ghost_pos,
            rotation,
            PieceDrawStyle::Ghost,
        );
    }

    draw_piece_on_board(
        frame,
        width,
        height,
        board_rect,
        board_w,
        board_h,
        piece,
        state.current_piece_pos(),
        rotation,
        PieceDrawStyle::Solid,
    );
}

fn draw_piece_on_board(
    frame: &mut [u8],
    width: u32,
    height: u32,
    board_rect: Rect,
    board_w: u32,
    board_h: u32,
    piece: Piece,
    pos: Vec2i,
    rotation: u8,
    style: PieceDrawStyle,
) {
    if board_w == 0 || board_h == 0 {
        return;
    }

    let grid = piece_grid(piece, rotation);
    let offset = piece_board_offset(piece);
    let color = color_for_cell(piece_type(piece));

    for gy in 0..grid.size() {
        for gx in 0..grid.size() {
            if grid.cell(gx, gy) != 1 {
                continue;
            }

            let board_x = pos.x + gx as i32 - offset;
            let board_y = pos.y - gy as i32 + offset;

            if board_x < 0 || board_x >= board_w as i32 {
                continue;
            }
            if board_y < 0 || board_y >= board_h as i32 {
                continue;
            }

            let pixel_x = board_rect.x + (board_x as u32) * CELL_SIZE;
            let inverted_y = (board_h - 1).saturating_sub(board_y as u32);
            let pixel_y = board_rect.y + inverted_y * CELL_SIZE;

            match style {
                PieceDrawStyle::Solid => {
                    fill_rect(frame, width, height, pixel_x, pixel_y, CELL_SIZE, CELL_SIZE, color);
                }
                PieceDrawStyle::Ghost => {
                    blend_rect(frame, width, height, pixel_x, pixel_y, CELL_SIZE, CELL_SIZE, color, GHOST_ALPHA);
                }
            }
        }
    }
}

fn draw_hold_panel(
    frame: &mut [u8],
    width: u32,
    height: u32,
    rect: Rect,
    held_piece: Option<Piece>,
    can_hold: bool,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }

    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG);
    let border = if can_hold {
        COLOR_PANEL_BORDER
    } else {
        COLOR_PANEL_BORDER_DISABLED
    };
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);

    let preview_x = rect.x + PANEL_PADDING;
    let preview_y = rect.y + PANEL_PADDING;
    draw_piece_preview(frame, width, height, preview_x, preview_y, held_piece, can_hold);
}

fn draw_next_panel(frame: &mut [u8], width: u32, height: u32, rect: Rect, next_queue: &[Piece]) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }

    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, COLOR_PANEL_BG);
    draw_rect_outline(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        COLOR_PANEL_BORDER,
    );

    let mut y = rect.y + PANEL_PADDING;
    let x = rect.x + PANEL_PADDING;

    for &piece in next_queue {
        if y.saturating_add(PREVIEW_SIZE) > rect.y.saturating_add(rect.h) {
            break;
        }
        draw_piece_preview(frame, width, height, x, y, Some(piece), true);
        y = y.saturating_add(PREVIEW_SIZE + PREVIEW_GAP_Y);
    }
}

fn draw_piece_preview(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    piece: Option<Piece>,
    enabled: bool,
) {
    if x >= width || y >= height {
        return;
    }

    // Preview background area.
    fill_rect(frame, width, height, x, y, PREVIEW_SIZE, PREVIEW_SIZE, [10, 10, 14, 255]);

    let Some(piece) = piece else {
        return;
    };

    let grid = piece_grid(piece, 0);
    let grid_h = grid.size() as u32;
    let grid_w = grid.size() as u32;

    let offset_x = (PREVIEW_GRID.saturating_sub(grid_w)) / 2;
    let offset_y = (PREVIEW_GRID.saturating_sub(grid_h)) / 2;

    let mut color = color_for_cell(piece_type(piece));
    if !enabled {
        color = dim_color(color, 0.55);
    }

    for gy in 0..grid.size() {
        for gx in 0..grid.size() {
            if grid.cell(gx, gy) != 1 {
                continue;
            }

            let px = x + (offset_x + gx as u32) * PREVIEW_CELL;
            let py = y + (offset_y + gy as u32) * PREVIEW_CELL;
            fill_rect(frame, width, height, px, py, PREVIEW_CELL, PREVIEW_CELL, color);
        }
    }
}

fn dim_color(mut c: [u8; 4], factor: f32) -> [u8; 4] {
    let f = factor.clamp(0.0, 1.0);
    c[0] = ((c[0] as f32) * f) as u8;
    c[1] = ((c[1] as f32) * f) as u8;
    c[2] = ((c[2] as f32) * f) as u8;
    c
}

fn brighten_color(mut c: [u8; 4], amount: f32) -> [u8; 4] {
    let t = amount.clamp(0.0, 1.0);
    for i in 0..3 {
        let v = c[i] as f32;
        c[i] = (v + (255.0 - v) * t).round().clamp(0.0, 255.0) as u8;
    }
    c
}

fn button_colors(hovered: bool) -> ([u8; 4], [u8; 4]) {
    if hovered {
        (
            brighten_color(COLOR_PANEL_BG, BUTTON_HOVER_BRIGHTEN),
            brighten_color(COLOR_PANEL_BORDER, BUTTON_HOVER_BRIGHTEN),
        )
    } else {
        (COLOR_PANEL_BG, COLOR_PANEL_BORDER)
    }
}

fn draw_button(frame: &mut [u8], width: u32, height: u32, rect: Rect, label: &str, hovered: bool) {
    let (fill, border) = button_colors(hovered);
    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, fill);
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);
    draw_text(
        frame,
        width,
        height,
        rect.x.saturating_add(16),
        rect.y.saturating_add(rect.h / 2).saturating_sub(6),
        label,
        COLOR_PAUSE_MENU_TEXT,
    );
}

fn fill_rect(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    let max_x = (x + w).min(width);
    let max_y = (y + h).min(height);

    if x >= max_x || y >= max_y {
        return;
    }

    let width = width as usize;
    let height = height as usize;
    let expected_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(4))
        .unwrap_or(0);
    if expected_len == 0 || frame.len() < expected_len {
        return;
    }

    let row_pixels = (max_x - x) as usize;
    let row_bytes = row_pixels.checked_mul(4).unwrap_or(0);
    if row_bytes == 0 {
        return;
    }

    let stride = width.checked_mul(4).unwrap_or(0);
    let mut row_start = (y as usize)
        .checked_mul(stride)
        .and_then(|v| v.checked_add((x as usize).checked_mul(4)?))
        .unwrap_or(0);

    let [r, g, b, a] = color;
    for _ in y..max_y {
        let row_end = row_start + row_bytes;
        let row = &mut frame[row_start..row_end];
        for px in row.chunks_exact_mut(4) {
            px[0] = r;
            px[1] = g;
            px[2] = b;
            px[3] = a;
        }
        row_start += stride;
    }
}

fn blend_rect(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
    alpha: u8,
) {
    if alpha == 0 {
        return;
    }
    if alpha == 255 {
        fill_rect(frame, width, height, x, y, w, h, color);
        return;
    }

    let max_x = (x + w).min(width);
    let max_y = (y + h).min(height);
    if x >= max_x || y >= max_y {
        return;
    }

    let width = width as usize;
    let height = height as usize;
    let expected_len = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(4))
        .unwrap_or(0);
    if expected_len == 0 || frame.len() < expected_len {
        return;
    }

    let row_pixels = (max_x - x) as usize;
    let row_bytes = row_pixels.checked_mul(4).unwrap_or(0);
    if row_bytes == 0 {
        return;
    }

    let a = alpha as u32;
    let inv = 255u32 - a;
    let stride = width.checked_mul(4).unwrap_or(0);
    let mut row_start = (y as usize)
        .checked_mul(stride)
        .and_then(|v| v.checked_add((x as usize).checked_mul(4)?))
        .unwrap_or(0);

    for _ in y..max_y {
        let row_end = row_start + row_bytes;
        let row = &mut frame[row_start..row_end];
        for px in row.chunks_exact_mut(4) {
            let r0 = px[0] as u32;
            let g0 = px[1] as u32;
            let b0 = px[2] as u32;

            px[0] = ((r0 * inv + (color[0] as u32) * a + 127) / 255) as u8;
            px[1] = ((g0 * inv + (color[1] as u32) * a + 127) / 255) as u8;
            px[2] = ((b0 * inv + (color[2] as u32) * a + 127) / 255) as u8;
            px[3] = 255;
        }
        row_start += stride;
    }
}

fn draw_rect_outline(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    if w == 0 || h == 0 {
        return;
    }

    let x1 = x.saturating_add(w).min(width);
    let y1 = y.saturating_add(h).min(height);
    if x >= x1 || y >= y1 {
        return;
    }

    let w = x1 - x;
    let h = y1 - y;

    // Top / bottom edges.
    fill_rect(frame, width, height, x, y, w, 1, color);
    if h > 1 {
        fill_rect(frame, width, height, x, y1.saturating_sub(1), w, 1, color);
    }

    // Left / right edges.
    fill_rect(frame, width, height, x, y, 1, h, color);
    if w > 1 {
        fill_rect(frame, width, height, x1.saturating_sub(1), y, 1, h, color);
    }
}

