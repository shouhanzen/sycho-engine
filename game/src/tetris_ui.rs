use engine::graphics::Renderer2d;
use engine::render::{color_for_cell, draw_board, CELL_SIZE};
use engine::ui as ui;

use crate::skilltree::{NodeState, SkillTreeDef, SkillTreeProgress, SkillTreeRuntime};
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
    pub skilltree_editor_button: Rect,
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

    // Grid viewport + mapping (for hit-testing / editor interactions).
    pub grid: Rect,
    pub grid_origin_x: u32,
    pub grid_origin_y: u32,
    pub grid_cell: u32,
    pub grid_cols: u32,
    pub grid_rows: u32,
    pub grid_cam_min_x: i32,
    pub grid_cam_min_y: i32,
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

    // Layout: a vertical stack (title, start, skilltree editor, quit), centered in the safe region.
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
        .saturating_add(button_size.h.saturating_mul(3))
        .saturating_add(button_gap.saturating_mul(2));
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
    let skilltree_editor_button = Rect {
        x: start_button.x,
        y: start_button
            .y
            .saturating_add(start_button.h)
            .saturating_add(button_gap),
        w: start_button.w,
        h: start_button.h,
    };
    let quit_button = Rect {
        x: start_button.x,
        y: skilltree_editor_button
            .y
            .saturating_add(skilltree_editor_button.h)
            .saturating_add(button_gap),
        w: start_button.w,
        h: start_button.h,
    };

    for (rect, label) in [
        (start_button, "START"),
        (skilltree_editor_button, "SKILLTREE EDITOR"),
        (quit_button, "QUIT"),
    ] {
        let hovered = cursor.map(|(x, y)| rect.contains(x, y)).unwrap_or(false);
        draw_button(frame, width, height, rect, label, hovered);
    }

    MainMenuLayout {
        panel,
        start_button,
        skilltree_editor_button,
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

pub fn draw_skilltree_runtime_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
    runtime: &SkillTreeRuntime,
) -> SkillTreeLayout {
    draw_skilltree_impl(
        frame,
        width,
        height,
        cursor,
        Some(runtime),
        &runtime.def,
        &runtime.progress,
    )
}

pub fn draw_skilltree_with_cursor(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
) -> SkillTreeLayout {
    // Keep this function deterministic + I/O-free for tests; the headful game uses
    // `draw_skilltree_runtime_with_cursor` instead.
    let def = SkillTreeDef::default();
    let progress = SkillTreeProgress::default();
    draw_skilltree_impl(frame, width, height, cursor, None, &def, &progress)
}

fn draw_skilltree_impl(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cursor: Option<(u32, u32)>,
    runtime: Option<&SkillTreeRuntime>,
    def: &SkillTreeDef,
    progress: &SkillTreeProgress,
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

    let header_h = 120u32.min(safe.h);
    let footer_h = 80u32.min(safe.h.saturating_sub(header_h));

    let content = safe.inset(ui::Insets::all(pad));
    let grid = Rect {
        x: content.x,
        y: content.y.saturating_add(header_h.saturating_sub(pad)),
        w: content.w,
        h: content
            .h
            .saturating_sub(header_h.saturating_sub(pad))
            .saturating_sub(footer_h),
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

    let money_text = format!("MONEY {}", progress.money);
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad + 24),
        &money_text,
        COLOR_PAUSE_MENU_TEXT,
    );

    let editor_enabled = runtime.map(|rt| rt.editor.enabled).unwrap_or(false);
    if editor_enabled {
        let tool = runtime.map(|rt| format!("{:?}", rt.editor.tool)).unwrap_or_default();
        let tool_text = format!("EDITOR ON  TOOL {tool}  (TAB CYCLE, S SAVE, R RELOAD)");
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 48),
            &tool_text,
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 72),
            "ARROWS PAN  +/- ZOOM  N NEW  DEL DELETE  ESC EXIT EDITOR",
            COLOR_PAUSE_MENU_TEXT,
        );
    } else {
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 48),
            "CLICK: BUY  (F4: TOGGLE EDITOR)",
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 72),
            "ENTER: START NEW RUN   ESC: MAIN MENU",
            COLOR_PAUSE_MENU_TEXT,
        );
    }
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        safe.y.saturating_add(pad + 96),
        "TIP: EDITOR CHANGES SAVE TO game/assets/skilltree.json",
        COLOR_PAUSE_MENU_TEXT,
    );

    // Grid rendering (world coords; y increases upward).
    //
    // Camera:
    // - `grid_cam_min_*` are integer world cell coordinates (used for stable indexing / hit-testing).
    // - `grid_pan_px_*` are sub-cell pixel offsets derived from the camera's fractional pan, so panning
    //   can be smooth even though the world is cell-based.
    let grid_cell = runtime
        .map(|rt| rt.camera.cell_px.round().clamp(8.0, 64.0) as u32)
        .unwrap_or(20u32);
    let grid_cols = if grid_cell > 0 { grid.w / grid_cell } else { 0 };
    let grid_rows = if grid_cell > 0 { grid.h / grid_cell } else { 0 };

    let grid_pixel_w = grid_cols.saturating_mul(grid_cell).min(grid.w);
    let grid_pixel_h = grid_rows.saturating_mul(grid_cell).min(grid.h);
    let grid_origin_x = grid.x.saturating_add(grid.w.saturating_sub(grid_pixel_w) / 2);
    let grid_origin_y = grid.y.saturating_add(grid.h.saturating_sub(grid_pixel_h) / 2);

    let default_cam_min_x = -(grid_cols as i32) / 2;
    let default_cam_min_y = 0i32;

    let mut cam_min_x = default_cam_min_x as f32;
    let mut cam_min_y = default_cam_min_y as f32;
    if let Some(rt) = runtime {
        cam_min_x += rt.camera.pan.x;
        cam_min_y += rt.camera.pan.y;
    }

    let grid_cam_min_x = cam_min_x.floor() as i32;
    let grid_cam_min_y = cam_min_y.floor() as i32;

    let frac_x = cam_min_x - grid_cam_min_x as f32;
    let frac_y = cam_min_y - grid_cam_min_y as f32;

    let grid_pan_px_x = -((frac_x * grid_cell as f32).round() as i32);
    let grid_pan_px_y = (frac_y * grid_cell as f32).round() as i32;

    let grid_cell_i32 = grid_cell as i32;
    let grid_pixel_w_i32 = grid_pixel_w as i32;
    let grid_pixel_h_i32 = grid_pixel_h as i32;
    let grid_view_x0 = grid_origin_x as i32;
    let grid_view_y0 = grid_origin_y as i32;
    let grid_view_x1 = grid_view_x0.saturating_add(grid_pixel_w_i32);
    let grid_view_y1 = grid_view_y0.saturating_add(grid_pixel_h_i32);

    // Subtle dot grid (scrolls with the camera).
    if grid_cols > 0 && grid_rows > 0 {
        let dot = 2u32;
        let dot_color = [18, 18, 24, 255];
        for row in 0..grid_rows {
            for col in 0..grid_cols {
                let x0 = grid_view_x0
                    .saturating_add(col as i32 * grid_cell_i32)
                    .saturating_add(grid_pan_px_x);
                let y0 = grid_view_y0
                    .saturating_add(row as i32 * grid_cell_i32)
                    .saturating_add(grid_pan_px_y);
                let cx = x0 + grid_cell_i32 / 2 - dot as i32 / 2;
                let cy = y0 + grid_cell_i32 / 2 - dot as i32 / 2;

                if cx >= grid_view_x0
                    && cy >= grid_view_y0
                    && cx < grid_view_x1
                    && cy < grid_view_y1
                {
                    fill_rect_i32(frame, width, height, cx, cy, dot, dot, dot_color);
                }
            }
        }
    }

    // Draw nodes as polyblocks.
    let unlocked: std::collections::HashSet<&str> =
        progress.unlocked.iter().map(|s| s.as_str()).collect();
    let selected = runtime.and_then(|rt| rt.editor.selected.as_deref());
    let connect_from = runtime.and_then(|rt| rt.editor.connect_from.as_deref());
    for node in &def.nodes {
        let (state, can_buy) = if let Some(rt) = runtime {
            (rt.node_state(node), rt.can_buy(node))
        } else if unlocked.contains(node.id.as_str()) {
            (NodeState::Unlocked, false)
        } else if node.requires.iter().all(|r| unlocked.contains(r.as_str())) {
            (NodeState::Available, progress.money >= node.cost)
        } else {
            (NodeState::Locked, false)
        };

        let mut fill = color_for_cell(node.color);
        let mut border = COLOR_PANEL_BORDER;
        let is_selected = selected == Some(node.id.as_str());
        let is_connect_from = connect_from == Some(node.id.as_str());
        match state {
            NodeState::Unlocked => {
                border = COLOR_PANEL_BORDER;
            }
            NodeState::Available => {
                if can_buy {
                    fill = brighten_color(fill, 0.12);
                    border = brighten_color(border, 0.12);
                } else {
                    fill = dim_color(fill, 0.55);
                    border = dim_color(border, 0.65);
                }
            }
            NodeState::Locked => {
                fill = dim_color(fill, 0.25);
                border = dim_color(border, 0.55);
            }
        }
        if is_connect_from {
            border = brighten_color(border, 0.22);
        }
        if is_selected {
            border = [245, 245, 255, 255];
        }

        let mut bbox_min_x: Option<u32> = None;
        let mut bbox_min_y: Option<u32> = None;
        let mut bbox_max_x: u32 = 0;
        let mut bbox_max_y: u32 = 0;

        for rel in &node.shape {
            let wx = node.pos.x + rel.x;
            let wy = node.pos.y + rel.y;
            let col = wx - grid_cam_min_x;
            let row_from_bottom = wy - grid_cam_min_y;
            let row_from_top = grid_rows as i32 - 1 - row_from_bottom;

            // Convert to pixel coords (allowing an extra col/row for fractional panning).
            let px = grid_view_x0
                .saturating_add(col.saturating_mul(grid_cell_i32))
                .saturating_add(grid_pan_px_x);
            let py = grid_view_y0
                .saturating_add(row_from_top.saturating_mul(grid_cell_i32))
                .saturating_add(grid_pan_px_y);

            let cell_x1 = px.saturating_add(grid_cell_i32);
            let cell_y1 = py.saturating_add(grid_cell_i32);
            let overlaps = cell_x1 > grid_view_x0
                && px < grid_view_x1
                && cell_y1 > grid_view_y0
                && py < grid_view_y1;
            if !overlaps {
                continue;
            }

            fill_rect_i32(frame, width, height, px, py, grid_cell, grid_cell, fill);
            if px >= 0
                && py >= 0
                && (px as u32).saturating_add(grid_cell) <= width
                && (py as u32).saturating_add(grid_cell) <= height
            {
                draw_rect_outline(frame, width, height, px as u32, py as u32, grid_cell, grid_cell, border);
            }

            let px0 = px.max(0) as u32;
            let py0 = py.max(0) as u32;
            bbox_min_x = Some(bbox_min_x.map(|v| v.min(px0)).unwrap_or(px0));
            bbox_min_y = Some(bbox_min_y.map(|v| v.min(py0)).unwrap_or(py0));
            bbox_max_x = bbox_max_x.max(px0.saturating_add(grid_cell));
            bbox_max_y = bbox_max_y.max(py0.saturating_add(grid_cell));
        }

        // Label + cost.
        if let (Some(min_x), Some(min_y)) = (bbox_min_x, bbox_min_y) {
            let label_x = min_x.saturating_add(6);
            let label_y = min_y.saturating_add(6);
            draw_text(frame, width, height, label_x, label_y, &node.name, COLOR_PAUSE_MENU_TEXT);
            if node.cost > 0 {
                let cost = format!("${}", node.cost);
                draw_text(frame, width, height, label_x, label_y.saturating_add(18), &cost, COLOR_PAUSE_MENU_TEXT);
            }
        }
    }

    if let Some(rt) = runtime {
        if let Some(status) = rt.editor.status.as_deref() {
            draw_text(
                frame,
                width,
                height,
                safe.x.saturating_add(pad),
                safe.y.saturating_add(safe.h.saturating_sub(pad + 16)),
                status,
                COLOR_PAUSE_MENU_TEXT,
            );
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
        grid,
        grid_origin_x,
        grid_origin_y,
        grid_cell,
        grid_cols,
        grid_rows,
        grid_cam_min_x,
        grid_cam_min_y,
    }
}

#[derive(Debug, Clone, Copy)]
enum PieceDrawStyle {
    Solid,
    Ghost,
}

fn draw_ghost_and_active_piece(
    frame: &mut dyn Renderer2d,
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
    frame: &mut dyn Renderer2d,
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
    frame: &mut dyn Renderer2d,
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

fn draw_next_panel(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    rect: Rect,
    next_queue: &[Piece],
) {
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
    frame: &mut dyn Renderer2d,
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

fn draw_button(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    rect: Rect,
    label: &str,
    hovered: bool,
) {
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
    frame: &mut dyn Renderer2d,
    _width: u32,
    _height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    frame.fill_rect(Rect::new(x, y, w, h), color);
}

fn fill_rect_i32(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    if w == 0 || h == 0 {
        return;
    }

    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x.saturating_add(w as i32)).clamp(0, width as i32) as u32;
    let y1 = (y.saturating_add(h as i32)).clamp(0, height as i32) as u32;
    if x1 <= x0 || y1 <= y0 {
        return;
    }

    fill_rect(frame, width, height, x0, y0, x1 - x0, y1 - y0, color);
}

fn blend_rect(
    frame: &mut dyn Renderer2d,
    _width: u32,
    _height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
    alpha: u8,
) {
    frame.blend_rect(Rect::new(x, y, w, h), color, alpha);
}

fn draw_rect_outline(
    frame: &mut dyn Renderer2d,
    _width: u32,
    _height: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    color: [u8; 4],
) {
    frame.rect_outline(Rect::new(x, y, w, h), color);
}

fn draw_text(
    frame: &mut dyn Renderer2d,
    _width: u32,
    _height: u32,
    x: u32,
    y: u32,
    text: &str,
    color: [u8; 4],
) {
    frame.draw_text(x, y, text, color);
}

fn draw_text_scaled(
    frame: &mut dyn Renderer2d,
    _width: u32,
    _height: u32,
    x: u32,
    y: u32,
    text: &str,
    color: [u8; 4],
    scale: u32,
) {
    frame.draw_text_scaled(x, y, text, color, scale);
}

