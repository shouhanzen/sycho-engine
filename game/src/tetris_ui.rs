use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

use engine::graphics::Renderer2d;
use engine::render::{
    CELL_SIZE, clip_rect_to_viewport, color_for_cell, draw_board_cells_in_rect_clipped_with_owners,
};
use engine::ui;
use engine::ui_tree::UiTree;

use crate::background::draw_tile_background_in_viewport;
use crate::skilltree::{
    NodeState, SkillTreeDef, SkillTreeEditorTool, SkillTreeProgress, SkillTreeRuntime,
    skilltree_world_bounds,
};
use crate::tetris_core::{Piece, TetrisCore, Vec2i, piece_board_offset, piece_grid, piece_type};
use crate::ui_ids::*;

mod menus;
pub use menus::{
    GameOverMenuLayout, GameOverMenuView, MainMenuLayout, MainMenuView, PauseMenuLayout,
    PauseMenuView, SettingsMenuLayout, SettingsMenuView, draw_game_over_menu,
    draw_game_over_menu_with_ui, draw_main_menu, draw_main_menu_with_ui, draw_pause_menu,
    draw_pause_menu_with_ui, draw_settings_menu, draw_settings_menu_with_ui,
};

const COLOR_PANEL_BG: [u8; 4] = [16, 16, 22, 255];
const COLOR_PANEL_BORDER: [u8; 4] = [40, 40, 55, 255];
const COLOR_PANEL_BORDER_DISABLED: [u8; 4] = [28, 28, 38, 255];
const BUTTON_HOVER_BRIGHTEN: f32 = 0.12;
const COLOR_SKILLTREE_LINK: [u8; 4] = [110, 110, 150, 255];
const SKILLTREE_LINK_THICKNESS: u32 = 2;
const SKILLTREE_ARROW_CAP_LENGTH: i32 = 8;
const SKILLTREE_ARROW_CAP_SPREAD: i32 = 4;
const SKILLTREE_ROUTE_BOUNDS_PAD_CELLS: i32 = 8;
const SKILLTREE_ROUTE_STEP_COST: i32 = 10;
const SKILLTREE_ROUTE_TURN_PENALTY: i32 = 8;
const SKILLTREE_ROUTE_OVERLAP_PENALTY: i32 = 2;
const COLOR_SKILLTREE_CURSOR: [u8; 4] = [255, 220, 120, 255];

pub const MAIN_MENU_TITLE: &str = "UNTITLED";

const PAUSE_BUTTON_SIZE: u32 = 44;
const PAUSE_BUTTON_MARGIN: u32 = 12;
const COLOR_PAUSE_ICON: [u8; 4] = [235, 235, 245, 255];
const COLOR_DEPTH_LOCKED: [u8; 4] = [255, 188, 112, 255];
const COLOR_DEPTH_WALL_FILL: [u8; 4] = [24, 20, 16, 255];
const COLOR_DEPTH_WALL_BORDER: [u8; 4] = [120, 92, 62, 255];
const DEPTH_WALL_OVERLAY_ROWS: u32 = 2;

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
const LINE_CLEAR_FLASH_COLOR: [u8; 4] = [255, 255, 255, 255];
const COLOR_TIP_MARKER: [u8; 4] = [245, 235, 170, 255];

pub type Rect = ui::Rect;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiLayout {
    pub board: Rect,
    pub hold_panel: Rect,
    pub next_panel: Rect,
    pub pause_button: Rect,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkillTreeLayout {
    pub panel: Rect,
    pub start_new_game_button: Rect,
    pub tool_select_button: Rect,
    pub tool_move_button: Rect,
    pub tool_add_cell_button: Rect,
    pub tool_remove_cell_button: Rect,
    pub tool_connect_button: Rect,

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

pub fn compute_layout(
    width: u32,
    height: u32,
    board_w: u32,
    board_h: u32,
    next_len: usize,
) -> UiLayout {
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
        .saturating_add(
            (next_len as u32)
                .saturating_sub(1)
                .saturating_mul(PREVIEW_GAP_Y),
        );
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

/// Draw the Tetris world layers in the correct compositing order:
///
/// 1. **Background** - tile background/clear pass.
/// 2. **Board cells** – outline, grid dots, and locked cells (no full-screen clear).
/// 4. **Active piece + ghost** – drawn on top of the board.
///
/// The HUD/UI layer (#5) is handled separately by `draw_tetris_hud` so callers can
/// insert additional overlays between the world and HUD if needed.
pub fn draw_tetris_world(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
) -> UiLayout {
    draw_tetris_world_with_camera_offset(frame, width, height, state, 0)
}

pub fn draw_tetris_world_with_camera_offset(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    world_offset_y_px: i32,
) -> UiLayout {
    let board = state.board();
    let board_h = board.len() as u32;
    let board_w = board.first().map(|r| r.len()).unwrap_or(0) as u32;
    let layout = compute_layout(width, height, board_w, board_h, state.next_queue().len());
    let world_offset_y_px = clamp_world_offset_y(layout.board, height, world_offset_y_px);
    let world_board_rect = offset_rect_y(layout.board, world_offset_y_px);

    // --- Layer 1: background ---
    draw_tile_background_in_viewport(
        frame,
        width,
        height,
        layout.board,
        state.background_depth_rows(),
        state.background_seed(),
        world_offset_y_px,
    );

    // --- Layer 2: board cells ---
    draw_board_cells_in_rect_clipped_with_owners(
        frame,
        board,
        Some(state.board_piece_ids()),
        world_board_rect,
        layout.board,
    );

    draw_line_clear_overlay(
        frame,
        width,
        height,
        world_board_rect,
        layout.board,
        board_w,
        board_h,
        state,
    );

    draw_depth_wall_overlay(
        frame,
        width,
        height,
        world_board_rect,
        layout.board,
        board_w,
        board_h,
        state,
    );

    // --- Layer 4: active piece + ghost ---
    draw_ghost_and_active_piece(
        frame,
        width,
        height,
        world_board_rect,
        layout.board,
        board_w,
        board_h,
        state,
    );

    layout
}

fn draw_line_clear_overlay(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    board_rect: Rect,
    viewport_rect: Rect,
    board_w: u32,
    board_h: u32,
    state: &TetrisCore,
) {
    if !state.is_line_clear_active() || board_w == 0 || board_h == 0 {
        return;
    }
    let rows = state.line_clear_rows();
    if rows.is_empty() {
        return;
    }

    let progress = state.line_clear_progress().clamp(0.0, 1.0);
    // Pulse to white and back during the clear-delay window.
    let pulse = 1.0 - (progress * 2.0 - 1.0).abs();
    let alpha = (70.0 + pulse * 160.0).round().clamp(0.0, 255.0) as u8;

    for &row in rows {
        if row >= board_h as usize {
            continue;
        }
        let inverted_y = board_h - 1 - row as u32;
        let pixel_y = board_rect.y + inverted_y * CELL_SIZE;
        for x in 0..board_w {
            let pixel_x = board_rect.x + x * CELL_SIZE;
            let cell_rect = Rect::new(pixel_x, pixel_y, CELL_SIZE, CELL_SIZE);
            let Some(clipped) = clip_rect_to_viewport(cell_rect, viewport_rect) else {
                continue;
            };
            blend_rect(
                frame,
                width,
                height,
                clipped.x,
                clipped.y,
                clipped.w,
                clipped.h,
                LINE_CLEAR_FLASH_COLOR,
                alpha,
            );
        }
    }
}

fn draw_depth_wall_overlay(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    board_rect: Rect,
    viewport_rect: Rect,
    board_w: u32,
    board_h: u32,
    state: &TetrisCore,
) {
    if !state.depth_progress_paused() || board_w == 0 || board_h == 0 {
        return;
    }

    let wall_rows = DEPTH_WALL_OVERLAY_ROWS.min(board_h);
    if wall_rows == 0 {
        return;
    }

    let wall_h = wall_rows.saturating_mul(CELL_SIZE);
    let wall_y = board_rect
        .y
        .saturating_add(board_h.saturating_sub(wall_rows).saturating_mul(CELL_SIZE));
    let wall_rect = Rect::new(board_rect.x, wall_y, board_w.saturating_mul(CELL_SIZE), wall_h);
    let Some(clipped) = clip_rect_to_viewport(wall_rect, viewport_rect) else {
        return;
    };

    blend_rect(
        frame,
        width,
        height,
        clipped.x,
        clipped.y,
        clipped.w,
        clipped.h,
        COLOR_DEPTH_WALL_FILL,
        230,
    );
    draw_rect_outline(
        frame,
        width,
        height,
        clipped.x,
        clipped.y,
        clipped.w,
        clipped.h,
        COLOR_DEPTH_WALL_BORDER,
    );

    let hp_text = format!("WALL HP {}", state.active_wall_hp_remaining());
    let text_w = (hp_text.len() as u32).saturating_mul(8);
    let text_x = clipped.x.saturating_add(clipped.w.saturating_sub(text_w) / 2);
    let text_y = clipped.y.saturating_add(clipped.h.saturating_sub(8) / 2);
    draw_text(frame, width, height, text_x, text_y, &hp_text, COLOR_DEPTH_LOCKED);
}

pub fn draw_tetris_hud(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
) {
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    draw_tetris_hud_with_ui(frame, width, height, state, layout, &mut ui_tree);
}

pub fn draw_tetris_hud_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
    ui_tree: &mut UiTree,
) {
    let pause_hovered = ui_tree.is_hovered(UI_TETRIS_PAUSE);
    draw_tetris_hud_with_ui_and_pause_hover(
        frame,
        width,
        height,
        state,
        layout,
        ui_tree,
        pause_hovered,
    );
}

pub fn draw_tetris_hud_with_ui_and_pause_hover(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
    ui_tree: &mut UiTree,
    pause_hovered: bool,
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

    ui_tree.ensure_container(UI_TETRIS_HUD_CONTAINER, ui::Rect::from_size(width, height));
    ui_tree.add_child(UI_CANVAS, UI_TETRIS_HUD_CONTAINER);
    ui_tree.ensure_button(
        UI_TETRIS_PAUSE,
        layout.pause_button,
        Some(ACTION_TETRIS_TOGGLE_PAUSE),
    );
    ui_tree.add_child(UI_TETRIS_HUD_CONTAINER, UI_TETRIS_PAUSE);
    ui_tree.ensure_button(UI_TETRIS_HOLD, layout.hold_panel, Some(ACTION_TETRIS_HOLD));
    ui_tree.add_child(UI_TETRIS_HUD_CONTAINER, UI_TETRIS_HOLD);

    draw_pause_button(frame, width, height, layout.pause_button, pause_hovered);

    draw_tetris_status_text(frame, width, height, state, layout);
}

pub fn draw_tetris_hud_view(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
    mouse_pos: Option<(u32, u32)>,
) {
    let pause_hovered = mouse_pos
        .map(|(mx, my)| layout.pause_button.contains(mx, my))
        .unwrap_or(false);
    draw_hold_panel(
        frame,
        width,
        height,
        layout.hold_panel,
        state.held_piece(),
        state.can_hold(),
    );
    draw_next_panel(frame, width, height, layout.next_panel, state.next_queue());
    draw_pause_button(frame, width, height, layout.pause_button, pause_hovered);

    draw_tetris_status_text(frame, width, height, state, layout);
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

fn draw_tetris_status_text(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    state: &TetrisCore,
    layout: UiLayout,
) {
    let hud_x = layout.pause_button.x.saturating_sub(220);
    let mut y = layout.pause_button.y.saturating_add(6);
    let score_text = format!("SCORE {}", state.score());
    let lines_text = format!("LINES {}", state.lines_cleared());
    let depth_text = format!("DEPTH {}", state.background_depth_rows());

    draw_text(
        frame,
        width,
        height,
        hud_x,
        y,
        &score_text,
        COLOR_PAUSE_ICON,
    );
    y = y.saturating_add(14);
    draw_text(
        frame,
        width,
        height,
        hud_x,
        y,
        &lines_text,
        COLOR_PAUSE_ICON,
    );
    y = y.saturating_add(14);
    draw_text(
        frame,
        width,
        height,
        hud_x,
        y,
        &depth_text,
        COLOR_PAUSE_ICON,
    );

    if state.depth_progress_paused() {
        let lock_text = "DEPTH LOCKED";
        let wall_text = state
            .active_wall_label()
            .unwrap_or_else(|| "MILESTONE WALL".to_string());
        let hp_text = format!("WALL HP {}", state.active_wall_hp_remaining());
        y = y.saturating_add(14);
        draw_text(
            frame,
            width,
            height,
            hud_x,
            y,
            lock_text,
            COLOR_DEPTH_LOCKED,
        );
        y = y.saturating_add(14);
        draw_text(
            frame,
            width,
            height,
            hud_x,
            y,
            &wall_text,
            COLOR_DEPTH_LOCKED,
        );
        y = y.saturating_add(14);
        draw_text(frame, width, height, hud_x, y, &hp_text, COLOR_DEPTH_LOCKED);
    }
}

fn offset_rect_y(rect: Rect, delta_y_px: i32) -> Rect {
    let y = if delta_y_px >= 0 {
        rect.y.saturating_add(delta_y_px as u32)
    } else {
        rect.y.saturating_sub(delta_y_px.saturating_abs() as u32)
    };
    Rect { y, ..rect }
}

fn clamp_world_offset_y(board: Rect, frame_height: u32, requested_offset_y_px: i32) -> i32 {
    let max_up = -(board.y.min(i32::MAX as u32) as i32);
    let max_down = frame_height
        .saturating_sub(board.y.saturating_add(board.h))
        .min(i32::MAX as u32) as i32;
    requested_offset_y_px.clamp(max_up, max_down)
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
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);

    // Draw a simple pause icon: two vertical bars.
    let bar_w = (rect.w / 6).max(3).min(rect.w);
    let bar_h = (rect.h * 2 / 3).max(6).min(rect.h);
    let gap = (rect.w / 5).max(4);

    let icon_total_w = bar_w.saturating_mul(2).saturating_add(gap);
    let icon_x0 = rect.x + rect.w.saturating_sub(icon_total_w) / 2;
    let icon_y0 = rect.y + rect.h.saturating_sub(bar_h) / 2;

    fill_rect(
        frame,
        width,
        height,
        icon_x0,
        icon_y0,
        bar_w,
        bar_h,
        COLOR_PAUSE_ICON,
    );
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

pub fn draw_skilltree(frame: &mut dyn Renderer2d, width: u32, height: u32) -> SkillTreeLayout {
    let mut ui_tree = UiTree::new();
    ui_tree.ensure_canvas(UI_CANVAS, ui::Rect::from_size(width, height));
    ui_tree.add_root(UI_CANVAS);
    draw_skilltree_with_ui(frame, width, height, &mut ui_tree)
}

pub fn draw_skilltree_runtime_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
    runtime: &SkillTreeRuntime,
) -> SkillTreeLayout {
    draw_skilltree_runtime_with_ui_and_mouse(frame, width, height, ui_tree, runtime, None)
}

pub fn draw_skilltree_runtime_with_ui_and_mouse(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
    runtime: &SkillTreeRuntime,
    mouse_pos: Option<(u32, u32)>,
) -> SkillTreeLayout {
    draw_skilltree_impl(
        frame,
        width,
        height,
        ui_tree,
        Some(runtime),
        &runtime.def,
        &runtime.progress,
        mouse_pos,
    )
}

pub fn draw_skilltree_with_ui(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
) -> SkillTreeLayout {
    // Keep this function deterministic + I/O-free for tests; the headful game uses
    // `draw_skilltree_runtime_with_ui` instead.
    let def = SkillTreeDef::default();
    let progress = SkillTreeProgress::default();
    draw_skilltree_impl(frame, width, height, ui_tree, None, &def, &progress, None)
}

fn draw_skilltree_impl(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    ui_tree: &mut UiTree,
    runtime: Option<&SkillTreeRuntime>,
    def: &SkillTreeDef,
    progress: &SkillTreeProgress,
    mouse_pos: Option<(u32, u32)>,
) -> SkillTreeLayout {
    // Skilltree is its own scene: clear the frame so the Tetris board is not visible.
    fill_rect(frame, width, height, 0, 0, width, height, color_for_cell(0));

    let margin = 0u32;
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

    let content = safe.inset(ui::Insets::all(pad));
    let grid = Rect {
        x: panel.x,
        y: panel.y,
        w: panel.w,
        h: panel.h,
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
    let mut tool_select_button = Rect::default();
    let mut tool_move_button = Rect::default();
    let mut tool_add_cell_button = Rect::default();
    let mut tool_remove_cell_button = Rect::default();
    let mut tool_connect_button = Rect::default();
    let mut tip_y;
    if editor_enabled {
        let tool = runtime
            .map(|rt| rt.editor.tool)
            .unwrap_or(SkillTreeEditorTool::Select);
        let mut context = format!(
            "TOOL {}  SEL NONE  SAVED  LINK -",
            skilltree_tool_label(tool)
        );
        let mut help_expanded = false;
        let mut search_line: Option<String> = None;
        if let Some(rt) = runtime {
            let selected_text = rt
                .editor
                .selected
                .as_deref()
                .map(|id| {
                    if let Some(idx) = rt.node_index(id) {
                        format!("{id}/{}", rt.def.nodes[idx].name)
                    } else {
                        id.to_string()
                    }
                })
                .unwrap_or_else(|| "NONE".to_string());
            let dirty = if rt.editor.dirty { "UNSAVED*" } else { "SAVED" };
            let link = rt.editor.connect_from.as_deref().unwrap_or("-");
            context = format!(
                "TOOL {}  SEL {}  {}  LINK {}",
                skilltree_tool_label(tool),
                selected_text,
                dirty,
                link
            );
            if let Some(pending) = rt.editor.pending_delete_id.as_deref() {
                context.push_str(&format!("  DEL? {pending}"));
            }
            help_expanded = rt.editor.help_expanded;
            if rt.editor.search_open {
                search_line = Some(format!("SEARCH: {}", rt.editor.search_query));
            }
        }
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 48),
            &context,
            COLOR_PAUSE_MENU_TEXT,
        );
        draw_text(
            frame,
            width,
            height,
            safe.x.saturating_add(pad),
            safe.y.saturating_add(pad + 72),
            "1..5 TOOL TAB CYCLE ? HELP / SEARCH | ARROWS PAN SHIFT+ARROWS FAST 0 RESET F FOCUS",
            COLOR_PAUSE_MENU_TEXT,
        );
        if help_expanded {
            draw_text(
                frame,
                width,
                height,
                safe.x.saturating_add(pad),
                safe.y.saturating_add(pad + 96),
                "IJKL CURSOR ENTER APPLY | N NEW DEL(confirm) CTRL+Z/Y UNDO/REDO CTRL+D DUP",
                COLOR_PAUSE_MENU_TEXT,
            );
            draw_text(
                frame,
                width,
                height,
                safe.x.saturating_add(pad),
                safe.y.saturating_add(pad + 120),
                "SHIFT+IJKL NUDGE SELECTED | S SAVE R RELOAD ESC EXIT",
                COLOR_PAUSE_MENU_TEXT,
            );
            tip_y = safe.y.saturating_add(pad + 144);
        } else {
            draw_text(
                frame,
                width,
                height,
                safe.x.saturating_add(pad),
                safe.y.saturating_add(pad + 96),
                "IJKL CURSOR ENTER APPLY | N NEW DEL(confirm) CTRL+Z/Y UNDO/REDO CTRL+D DUP",
                COLOR_PAUSE_MENU_TEXT,
            );
            tip_y = safe.y.saturating_add(pad + 120);
        }
        if let Some(search_line) = search_line.as_deref() {
            draw_text(
                frame,
                width,
                height,
                safe.x.saturating_add(pad),
                tip_y,
                search_line,
                COLOR_PAUSE_MENU_TEXT,
            );
            tip_y = tip_y.saturating_add(24);
        }

        let header_h = 120u32.min(safe.h);
        let tool_gap = 8u32;
        let tool_button_h = 30u32.min(header_h.saturating_sub(pad));
        let tool_button_w = 120u32.min(content.w);
        let tool_count = 5u32;
        let total_w = tool_button_w
            .saturating_mul(tool_count)
            .saturating_add(tool_gap.saturating_mul(tool_count.saturating_sub(1)));
        let toolbar_x = safe
            .x
            .saturating_add(safe.w.saturating_sub(pad))
            .saturating_sub(total_w);
        let toolbar_y = safe.y.saturating_add(pad);

        ui_tree.ensure_container(UI_SKILLTREE_TOOLBAR, Rect::from_size(width, height));
        ui_tree.add_child(UI_SKILLTREE_CONTAINER, UI_SKILLTREE_TOOLBAR);

        let tool_buttons = [
            (SkillTreeEditorTool::Select, "SELECT"),
            (SkillTreeEditorTool::Move, "MOVE"),
            (SkillTreeEditorTool::AddCell, "ADD CELL"),
            (SkillTreeEditorTool::RemoveCell, "REMOVE CELL"),
            (SkillTreeEditorTool::ConnectPrereqs, "LINK"),
        ];

        for (idx, (tool_kind, label)) in tool_buttons.iter().enumerate() {
            let x = toolbar_x.saturating_add((tool_button_w + tool_gap).saturating_mul(idx as u32));
            let rect = Rect {
                x,
                y: toolbar_y,
                w: tool_button_w,
                h: tool_button_h,
            };
            let active = tool == *tool_kind;
            let hovered = if let Some((mx, my)) = mouse_pos {
                rect.contains(mx, my)
            } else {
                match tool_kind {
                    SkillTreeEditorTool::Select => ui_tree.is_hovered(UI_SKILLTREE_TOOL_SELECT),
                    SkillTreeEditorTool::Move => ui_tree.is_hovered(UI_SKILLTREE_TOOL_MOVE),
                    SkillTreeEditorTool::AddCell => ui_tree.is_hovered(UI_SKILLTREE_TOOL_ADD_CELL),
                    SkillTreeEditorTool::RemoveCell => {
                        ui_tree.is_hovered(UI_SKILLTREE_TOOL_REMOVE_CELL)
                    }
                    SkillTreeEditorTool::ConnectPrereqs => {
                        ui_tree.is_hovered(UI_SKILLTREE_TOOL_LINK)
                    }
                }
            };
            draw_tool_button(frame, width, height, rect, label, hovered, active);

            match tool_kind {
                SkillTreeEditorTool::Select => tool_select_button = rect,
                SkillTreeEditorTool::Move => tool_move_button = rect,
                SkillTreeEditorTool::AddCell => tool_add_cell_button = rect,
                SkillTreeEditorTool::RemoveCell => tool_remove_cell_button = rect,
                SkillTreeEditorTool::ConnectPrereqs => tool_connect_button = rect,
            }
        }

        ui_tree.ensure_button(
            UI_SKILLTREE_TOOL_SELECT,
            tool_select_button,
            Some(ACTION_SKILLTREE_TOOL_SELECT),
        );
        ui_tree.add_child(UI_SKILLTREE_TOOLBAR, UI_SKILLTREE_TOOL_SELECT);
        ui_tree.ensure_button(
            UI_SKILLTREE_TOOL_MOVE,
            tool_move_button,
            Some(ACTION_SKILLTREE_TOOL_MOVE),
        );
        ui_tree.add_child(UI_SKILLTREE_TOOLBAR, UI_SKILLTREE_TOOL_MOVE);
        ui_tree.ensure_button(
            UI_SKILLTREE_TOOL_ADD_CELL,
            tool_add_cell_button,
            Some(ACTION_SKILLTREE_TOOL_ADD_CELL),
        );
        ui_tree.add_child(UI_SKILLTREE_TOOLBAR, UI_SKILLTREE_TOOL_ADD_CELL);
        ui_tree.ensure_button(
            UI_SKILLTREE_TOOL_REMOVE_CELL,
            tool_remove_cell_button,
            Some(ACTION_SKILLTREE_TOOL_REMOVE_CELL),
        );
        ui_tree.add_child(UI_SKILLTREE_TOOLBAR, UI_SKILLTREE_TOOL_REMOVE_CELL);
        ui_tree.ensure_button(
            UI_SKILLTREE_TOOL_LINK,
            tool_connect_button,
            Some(ACTION_SKILLTREE_TOOL_LINK),
        );
        ui_tree.add_child(UI_SKILLTREE_TOOLBAR, UI_SKILLTREE_TOOL_LINK);
    } else {
        ui_tree.set_visible(UI_SKILLTREE_TOOL_SELECT, false);
        ui_tree.set_visible(UI_SKILLTREE_TOOL_MOVE, false);
        ui_tree.set_visible(UI_SKILLTREE_TOOL_ADD_CELL, false);
        ui_tree.set_visible(UI_SKILLTREE_TOOL_REMOVE_CELL, false);
        ui_tree.set_visible(UI_SKILLTREE_TOOL_LINK, false);
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
        tip_y = safe.y.saturating_add(pad + 96);
    }
    draw_text(
        frame,
        width,
        height,
        safe.x.saturating_add(pad),
        tip_y,
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
    let grid_origin_x = grid
        .x
        .saturating_add(grid.w.saturating_sub(grid_pixel_w) / 2);
    let grid_origin_y = grid
        .y
        .saturating_add(grid.h.saturating_sub(grid_pixel_h) / 2);

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

    let mut node_boxes: HashMap<&str, NodeScreenBox> = HashMap::new();
    for node in &def.nodes {
        if let Some(bbox) = node_screen_bbox(
            node,
            grid_cam_min_x,
            grid_cam_min_y,
            grid_rows,
            grid_cell_i32,
            grid_cell,
            grid_view_x0,
            grid_view_y0,
            grid_view_x1,
            grid_view_y1,
            grid_pan_px_x,
            grid_pan_px_y,
        ) {
            node_boxes.insert(node.id.as_str(), bbox);
        }
    }

    let link_transform = SkilltreeGridTransform {
        grid_cam_min_x,
        grid_cam_min_y,
        grid_rows: grid_rows as i32,
        grid_cell_i32,
        grid_view_x0,
        grid_view_y0,
        grid_pan_px_x,
        grid_pan_px_y,
    };
    draw_skilltree_links(frame, width, height, def, link_transform);

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
                draw_rect_outline(
                    frame, width, height, px as u32, py as u32, grid_cell, grid_cell, border,
                );
            }
        }

        // Label + cost.
        if let Some(bbox) = node_boxes.get(node.id.as_str()) {
            let label_x = bbox.min_x.saturating_add(6);
            let label_y = bbox.min_y.saturating_add(6);
            draw_text(
                frame,
                width,
                height,
                label_x,
                label_y,
                &node.name,
                COLOR_PAUSE_MENU_TEXT,
            );
            if node.cost > 0 {
                let cost = format!("${}", node.cost);
                draw_text(
                    frame,
                    width,
                    height,
                    label_x,
                    label_y.saturating_add(18),
                    &cost,
                    COLOR_PAUSE_MENU_TEXT,
                );
            }
        }
    }

    if let Some(rt) = runtime {
        if rt.editor.enabled {
            let wx = rt.editor.cursor_world.x;
            let wy = rt.editor.cursor_world.y;
            let col = wx - grid_cam_min_x;
            let row_from_bottom = wy - grid_cam_min_y;
            let row_from_top = grid_rows as i32 - 1 - row_from_bottom;
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
            if overlaps {
                if px >= 0
                    && py >= 0
                    && (px as u32).saturating_add(grid_cell) <= width
                    && (py as u32).saturating_add(grid_cell) <= height
                {
                    draw_rect_outline(
                        frame,
                        width,
                        height,
                        px as u32,
                        py as u32,
                        grid_cell,
                        grid_cell,
                        COLOR_SKILLTREE_CURSOR,
                    );
                }
                let center_x = px.saturating_add(grid_cell_i32 / 2);
                let center_y = py.saturating_add(grid_cell_i32 / 2);
                fill_rect_i32(
                    frame,
                    width,
                    height,
                    center_x.saturating_sub(2),
                    center_y.saturating_sub(2),
                    4,
                    4,
                    COLOR_SKILLTREE_CURSOR,
                );
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

    let start_new_game_button = if editor_enabled {
        ui_tree.set_visible(UI_SKILLTREE_START_RUN, false);
        Rect::default()
    } else {
        let button_size = ui::Size::new(240, 44).clamp_max(content.size());
        let start_ui = content.place(button_size, ui::Anchor::BottomCenter);
        let start_new_game_button = Rect {
            x: start_ui.x,
            y: start_ui.y,
            w: start_ui.w,
            h: start_ui.h,
        };

        ui_tree.ensure_button(
            UI_SKILLTREE_START_RUN,
            start_new_game_button,
            Some(ACTION_SKILLTREE_START_RUN),
        );
        ui_tree.add_child(UI_SKILLTREE_CONTAINER, UI_SKILLTREE_START_RUN);
        let hovered = ui_tree.is_hovered(UI_SKILLTREE_START_RUN);
        draw_button(
            frame,
            width,
            height,
            start_new_game_button,
            "START NEW RUN",
            hovered,
        );
        start_new_game_button
    };

    SkillTreeLayout {
        panel,
        start_new_game_button,
        tool_select_button,
        tool_move_button,
        tool_add_cell_button,
        tool_remove_cell_button,
        tool_connect_button,
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

#[derive(Clone, Copy)]
enum TipDirection {
    Right,
    Down,
}

fn draw_ghost_and_active_piece(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    board_rect: Rect,
    viewport_rect: Rect,
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
            viewport_rect,
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
        viewport_rect,
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
    viewport_rect: Rect,
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
            let cell_rect = Rect::new(pixel_x, pixel_y, CELL_SIZE, CELL_SIZE);
            let Some(clipped_cell_rect) = clip_rect_to_viewport(cell_rect, viewport_rect) else {
                continue;
            };

            match style {
                PieceDrawStyle::Solid => {
                    fill_rect(
                        frame,
                        width,
                        height,
                        clipped_cell_rect.x,
                        clipped_cell_rect.y,
                        clipped_cell_rect.w,
                        clipped_cell_rect.h,
                        color,
                    );
                    if let Some(direction) = tip_direction(piece, rotation, gx, gy) {
                        draw_tip_marker(
                            frame,
                            width,
                            height,
                            clipped_cell_rect,
                            direction,
                            COLOR_TIP_MARKER,
                        );
                    }
                }
                PieceDrawStyle::Ghost => {
                    blend_rect(
                        frame,
                        width,
                        height,
                        clipped_cell_rect.x,
                        clipped_cell_rect.y,
                        clipped_cell_rect.w,
                        clipped_cell_rect.h,
                        color,
                        GHOST_ALPHA,
                    );
                    if let Some(direction) = tip_direction(piece, rotation, gx, gy) {
                        draw_tip_marker(
                            frame,
                            width,
                            height,
                            clipped_cell_rect,
                            direction,
                            dim_color(COLOR_TIP_MARKER, 0.6),
                        );
                    }
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

    fill_rect(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        COLOR_PANEL_BG,
    );
    let border = if can_hold {
        COLOR_PANEL_BORDER
    } else {
        COLOR_PANEL_BORDER_DISABLED
    };
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);

    let preview_x = rect.x + PANEL_PADDING;
    let preview_y = rect.y + PANEL_PADDING;
    draw_piece_preview(
        frame, width, height, preview_x, preview_y, held_piece, can_hold,
    );
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

    fill_rect(
        frame,
        width,
        height,
        rect.x,
        rect.y,
        rect.w,
        rect.h,
        COLOR_PANEL_BG,
    );
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
    fill_rect(
        frame,
        width,
        height,
        x,
        y,
        PREVIEW_SIZE,
        PREVIEW_SIZE,
        [10, 10, 14, 255],
    );

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
            fill_rect(
                frame,
                width,
                height,
                px,
                py,
                PREVIEW_CELL,
                PREVIEW_CELL,
                color,
            );
            if let Some(direction) = tip_direction(piece, 0, gx, gy) {
                let tip_rect = Rect::new(px, py, PREVIEW_CELL, PREVIEW_CELL);
                draw_tip_marker(frame, width, height, tip_rect, direction, COLOR_TIP_MARKER);
            }
        }
    }
}

fn tip_direction(piece: Piece, rotation: u8, gx: usize, gy: usize) -> Option<TipDirection> {
    let grid = piece_grid(piece, rotation);
    if grid.cell(gx, gy) != 1 {
        return None;
    }
    match piece {
        Piece::I => {
            if rotation % 2 == 0 {
                let tip_x = (0..grid.size()).rev().find(|&x| grid.cell(x, gy) == 1)?;
                if gx == tip_x {
                    Some(TipDirection::Right)
                } else {
                    None
                }
            } else {
                let tip_y = (0..grid.size()).rev().find(|&y| grid.cell(gx, y) == 1)?;
                if gy == tip_y {
                    Some(TipDirection::Down)
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

fn draw_tip_marker(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    cell_rect: Rect,
    direction: TipDirection,
    color: [u8; 4],
) {
    if cell_rect.w < 4 || cell_rect.h < 4 {
        return;
    }
    let marker_w = (cell_rect.w / 3).max(2).min(cell_rect.w);
    let marker_h = (cell_rect.h / 3).max(2).min(cell_rect.h);
    let (x, y) = match direction {
        TipDirection::Right => (
            cell_rect.x.saturating_add(cell_rect.w.saturating_sub(marker_w)),
            cell_rect.y.saturating_add((cell_rect.h.saturating_sub(marker_h)) / 2),
        ),
        TipDirection::Down => (
            cell_rect.x.saturating_add((cell_rect.w.saturating_sub(marker_w)) / 2),
            cell_rect.y.saturating_add(cell_rect.h.saturating_sub(marker_h)),
        ),
    };
    fill_rect(frame, width, height, x, y, marker_w, marker_h, color);
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

fn draw_tool_button(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    rect: Rect,
    label: &str,
    hovered: bool,
    active: bool,
) {
    let (mut fill, mut border) = button_colors(hovered);
    if active {
        fill = brighten_color(fill, 0.18);
        border = [245, 245, 255, 255];
    }
    fill_rect(frame, width, height, rect.x, rect.y, rect.w, rect.h, fill);
    draw_rect_outline(frame, width, height, rect.x, rect.y, rect.w, rect.h, border);
    draw_text(
        frame,
        width,
        height,
        rect.x.saturating_add(12),
        rect.y.saturating_add(rect.h / 2).saturating_sub(6),
        label,
        COLOR_PAUSE_MENU_TEXT,
    );
}

fn skilltree_tool_label(tool: SkillTreeEditorTool) -> &'static str {
    match tool {
        SkillTreeEditorTool::Select => "SELECT",
        SkillTreeEditorTool::Move => "MOVE",
        SkillTreeEditorTool::AddCell => "ADD CELL",
        SkillTreeEditorTool::RemoveCell => "REMOVE CELL",
        SkillTreeEditorTool::ConnectPrereqs => "LINK",
    }
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

#[derive(Debug, Clone, Copy)]
struct NodeScreenBox {
    min_x: u32,
    min_y: u32,
}

fn node_screen_bbox(
    node: &crate::skilltree::SkillNodeDef,
    grid_cam_min_x: i32,
    grid_cam_min_y: i32,
    grid_rows: u32,
    grid_cell_i32: i32,
    _grid_cell: u32,
    grid_view_x0: i32,
    grid_view_y0: i32,
    grid_view_x1: i32,
    grid_view_y1: i32,
    grid_pan_px_x: i32,
    grid_pan_px_y: i32,
) -> Option<NodeScreenBox> {
    let mut bbox_min_x: Option<u32> = None;
    let mut bbox_min_y: Option<u32> = None;

    for rel in &node.shape {
        let wx = node.pos.x + rel.x;
        let wy = node.pos.y + rel.y;
        let col = wx - grid_cam_min_x;
        let row_from_bottom = wy - grid_cam_min_y;
        let row_from_top = grid_rows as i32 - 1 - row_from_bottom;

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

        let px0 = px.max(0) as u32;
        let py0 = py.max(0) as u32;
        bbox_min_x = Some(bbox_min_x.map(|v| v.min(px0)).unwrap_or(px0));
        bbox_min_y = Some(bbox_min_y.map(|v| v.min(py0)).unwrap_or(py0));
    }

    Some(NodeScreenBox {
        min_x: bbox_min_x?,
        min_y: bbox_min_y?,
    })
}

#[derive(Debug, Clone, Copy)]
struct SkilltreeGridTransform {
    grid_cam_min_x: i32,
    grid_cam_min_y: i32,
    grid_rows: i32,
    grid_cell_i32: i32,
    grid_view_x0: i32,
    grid_view_y0: i32,
    grid_pan_px_x: i32,
    grid_pan_px_y: i32,
}

impl SkilltreeGridTransform {
    fn world_cell_top_left_px(&self, world: Vec2i) -> (i32, i32) {
        let col = world.x.saturating_sub(self.grid_cam_min_x);
        let row_from_bottom = world.y.saturating_sub(self.grid_cam_min_y);
        let row_from_top = self
            .grid_rows
            .saturating_sub(1)
            .saturating_sub(row_from_bottom);
        let px = self
            .grid_view_x0
            .saturating_add(col.saturating_mul(self.grid_cell_i32))
            .saturating_add(self.grid_pan_px_x);
        let py = self
            .grid_view_y0
            .saturating_add(row_from_top.saturating_mul(self.grid_cell_i32))
            .saturating_add(self.grid_pan_px_y);
        (px, py)
    }

    fn world_cell_center_px(&self, world: Vec2i) -> (i32, i32) {
        let (px, py) = self.world_cell_top_left_px(world);
        (
            px.saturating_add(self.grid_cell_i32 / 2),
            py.saturating_add(self.grid_cell_i32 / 2),
        )
    }

    fn world_cell_edge_mid_px(&self, world: Vec2i, dir: CardinalDir) -> (i32, i32) {
        let (px, py) = self.world_cell_top_left_px(world);
        let mid = self.grid_cell_i32 / 2;
        match dir {
            CardinalDir::Right => (
                px.saturating_add(self.grid_cell_i32.saturating_sub(1)),
                py.saturating_add(mid),
            ),
            CardinalDir::Left => (px, py.saturating_add(mid)),
            CardinalDir::Up => (px.saturating_add(mid), py),
            CardinalDir::Down => (
                px.saturating_add(mid),
                py.saturating_add(self.grid_cell_i32.saturating_sub(1)),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum CardinalDir {
    Right,
    Up,
    Left,
    Down,
}

impl CardinalDir {
    const ORDER: [CardinalDir; 4] = [
        CardinalDir::Right,
        CardinalDir::Up,
        CardinalDir::Left,
        CardinalDir::Down,
    ];

    fn delta(self) -> Vec2i {
        match self {
            CardinalDir::Right => Vec2i::new(1, 0),
            CardinalDir::Up => Vec2i::new(0, 1),
            CardinalDir::Left => Vec2i::new(-1, 0),
            CardinalDir::Down => Vec2i::new(0, -1),
        }
    }

    fn opposite(self) -> Self {
        match self {
            CardinalDir::Right => CardinalDir::Left,
            CardinalDir::Up => CardinalDir::Down,
            CardinalDir::Left => CardinalDir::Right,
            CardinalDir::Down => CardinalDir::Up,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteBounds {
    min: Vec2i,
    max: Vec2i,
}

impl RouteBounds {
    fn contains(self, p: Vec2i) -> bool {
        p.x >= self.min.x && p.y >= self.min.y && p.x <= self.max.x && p.y <= self.max.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkilltreeBoundaryPort {
    /// Outside world cell where routing starts/ends.
    route_cell: Vec2i,
    /// Node world cell that owns this boundary port.
    touch_cell: Vec2i,
    /// Direction from `touch_cell` toward `route_cell`.
    outward_dir: CardinalDir,
}

#[derive(Debug, Clone)]
struct SkilltreeNodeLinkData {
    anchor: Vec2i,
    ports: Vec<SkilltreeBoundaryPort>,
}

#[derive(Debug, Clone)]
struct SkilltreeLinkGeometry {
    occupancy: HashSet<(i32, i32)>,
    nodes: HashMap<String, SkilltreeNodeLinkData>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenRouteNode {
    key: RouteNodeKey,
    g: i32,
    f: i32,
    serial: usize,
}

impl Ord for OpenRouteNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .f
            .cmp(&self.f)
            .then_with(|| other.g.cmp(&self.g))
            .then_with(|| other.serial.cmp(&self.serial))
            .then_with(|| other.key.cmp(&self.key))
    }
}

impl PartialOrd for OpenRouteNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

type RouteNodeKey = ((i32, i32), Option<CardinalDir>);

fn world_key(cell: Vec2i) -> (i32, i32) {
    (cell.x, cell.y)
}

fn manhattan_distance(a: Vec2i, b: Vec2i) -> i32 {
    a.x.saturating_sub(b.x)
        .abs()
        .saturating_add(a.y.saturating_sub(b.y).abs())
}

fn skilltree_route_bounds(def: &SkillTreeDef) -> RouteBounds {
    if let Some(bounds) = skilltree_world_bounds(def) {
        return RouteBounds {
            min: Vec2i::new(
                bounds
                    .min
                    .x
                    .saturating_sub(SKILLTREE_ROUTE_BOUNDS_PAD_CELLS),
                bounds
                    .min
                    .y
                    .saturating_sub(SKILLTREE_ROUTE_BOUNDS_PAD_CELLS),
            ),
            max: Vec2i::new(
                bounds
                    .max
                    .x
                    .saturating_add(SKILLTREE_ROUTE_BOUNDS_PAD_CELLS),
                bounds
                    .max
                    .y
                    .saturating_add(SKILLTREE_ROUTE_BOUNDS_PAD_CELLS),
            ),
        };
    }

    RouteBounds {
        min: Vec2i::new(
            -SKILLTREE_ROUTE_BOUNDS_PAD_CELLS,
            -SKILLTREE_ROUTE_BOUNDS_PAD_CELLS,
        ),
        max: Vec2i::new(
            SKILLTREE_ROUTE_BOUNDS_PAD_CELLS,
            SKILLTREE_ROUTE_BOUNDS_PAD_CELLS,
        ),
    }
}

fn build_node_boundary_ports(
    cells: &[Vec2i],
    occupancy: &HashSet<(i32, i32)>,
) -> Vec<SkilltreeBoundaryPort> {
    let mut ports = Vec::new();
    let mut seen = HashSet::new();
    let node_cells: HashSet<(i32, i32)> = cells.iter().copied().map(world_key).collect();

    for cell in cells {
        for dir in CardinalDir::ORDER {
            let delta = dir.delta();
            let route_cell = Vec2i::new(
                cell.x.saturating_add(delta.x),
                cell.y.saturating_add(delta.y),
            );
            let key = world_key(route_cell);
            if node_cells.contains(&key) || occupancy.contains(&key) {
                continue;
            }
            if seen.insert(key) {
                ports.push(SkilltreeBoundaryPort {
                    route_cell,
                    touch_cell: *cell,
                    outward_dir: dir,
                });
            }
        }
    }

    // If every boundary candidate is occupied by another node, keep a deterministic fallback
    // set so links still render.
    if ports.is_empty() {
        for cell in cells {
            for dir in CardinalDir::ORDER {
                let delta = dir.delta();
                let route_cell = Vec2i::new(
                    cell.x.saturating_add(delta.x),
                    cell.y.saturating_add(delta.y),
                );
                let key = world_key(route_cell);
                if node_cells.contains(&key) {
                    continue;
                }
                if seen.insert(key) {
                    ports.push(SkilltreeBoundaryPort {
                        route_cell,
                        touch_cell: *cell,
                        outward_dir: dir,
                    });
                }
            }
        }
    }

    if ports.is_empty() {
        let fallback = *cells.first().unwrap_or(&Vec2i::ZERO);
        ports.push(SkilltreeBoundaryPort {
            route_cell: fallback,
            touch_cell: fallback,
            outward_dir: CardinalDir::Right,
        });
    }
    ports.sort_by_key(|p| {
        (
            p.route_cell.x,
            p.route_cell.y,
            p.touch_cell.x,
            p.touch_cell.y,
            p.outward_dir,
        )
    });
    ports
}

fn build_skilltree_link_geometry(def: &SkillTreeDef) -> SkilltreeLinkGeometry {
    let mut occupancy = HashSet::new();
    let mut cells_by_node = HashMap::new();
    for node in &def.nodes {
        let mut cells = Vec::new();
        for rel in &node.shape {
            let world = Vec2i::new(
                node.pos.x.saturating_add(rel.x),
                node.pos.y.saturating_add(rel.y),
            );
            cells.push(world);
            occupancy.insert(world_key(world));
        }
        cells.sort_by_key(|p| (p.x, p.y));
        if cells.is_empty() {
            cells.push(node.pos);
            occupancy.insert(world_key(node.pos));
        }
        cells_by_node.insert(node.id.clone(), cells);
    }

    let mut nodes = HashMap::new();
    for node in &def.nodes {
        let cells = cells_by_node
            .get(node.id.as_str())
            .cloned()
            .unwrap_or_else(|| vec![node.pos]);
        let ports = build_node_boundary_ports(&cells, &occupancy);
        nodes.insert(
            node.id.clone(),
            SkilltreeNodeLinkData {
                anchor: node.pos,
                ports,
            },
        );
    }

    SkilltreeLinkGeometry { occupancy, nodes }
}

fn ordered_ports_for_target(
    ports: &[SkilltreeBoundaryPort],
    target_anchor: Vec2i,
) -> Vec<SkilltreeBoundaryPort> {
    let mut ordered = ports.to_vec();
    ordered.sort_by_key(|port| {
        (
            manhattan_distance(port.route_cell, target_anchor),
            port.route_cell.x,
            port.route_cell.y,
            port.touch_cell.x,
            port.touch_cell.y,
            port.outward_dir,
        )
    });
    ordered
}

fn choose_link_ports(
    source: &SkilltreeNodeLinkData,
    target: &SkilltreeNodeLinkData,
) -> Option<(SkilltreeBoundaryPort, SkilltreeBoundaryPort)> {
    let source_ports = ordered_ports_for_target(&source.ports, target.anchor);
    let target_ports = ordered_ports_for_target(&target.ports, source.anchor);
    let mut best: Option<(
        (
            i32,
            usize,
            usize,
            i32,
            i32,
            i32,
            i32,
            i32,
            i32,
            CardinalDir,
            CardinalDir,
        ),
        SkilltreeBoundaryPort,
        SkilltreeBoundaryPort,
    )> = None;
    for (source_idx, source_port) in source_ports.iter().copied().enumerate() {
        for (target_idx, target_port) in target_ports.iter().copied().enumerate() {
            let score = (
                manhattan_distance(source_port.route_cell, target_port.route_cell),
                source_idx,
                target_idx,
                source_port.route_cell.x,
                source_port.route_cell.y,
                target_port.route_cell.x,
                target_port.route_cell.y,
                source_port.touch_cell.x,
                source_port.touch_cell.y,
                source_port.outward_dir,
                target_port.outward_dir,
            );
            let replace = best
                .as_ref()
                .map(|(best_score, _, _)| score < *best_score)
                .unwrap_or(true);
            if replace {
                best = Some((score, source_port, target_port));
            }
        }
    }
    best.map(|(_, source_port, target_port)| (source_port, target_port))
}

fn ordered_skilltree_edge_indices(def: &SkillTreeDef) -> Vec<(usize, usize)> {
    let mut id_to_index = HashMap::new();
    for (idx, node) in def.nodes.iter().enumerate() {
        id_to_index.insert(node.id.as_str(), idx);
    }

    let mut edges = Vec::new();
    for (target_idx, node) in def.nodes.iter().enumerate() {
        let mut source_indices: Vec<usize> = node
            .requires
            .iter()
            .filter_map(|req| id_to_index.get(req.as_str()).copied())
            .collect();
        source_indices.sort_unstable_by(|a, b| def.nodes[*a].id.cmp(&def.nodes[*b].id));
        for source_idx in source_indices {
            edges.push((source_idx, target_idx));
        }
    }
    edges
}

fn route_key_pos(key: RouteNodeKey) -> Vec2i {
    Vec2i::new((key.0).0, (key.0).1)
}

fn reconstruct_route_path(
    start_key: RouteNodeKey,
    end_key: RouteNodeKey,
    came_from: &HashMap<RouteNodeKey, RouteNodeKey>,
) -> Vec<Vec2i> {
    let mut current = end_key;
    let mut path = vec![route_key_pos(current)];
    while current != start_key {
        let Some(prev) = came_from.get(&current).copied() else {
            return Vec::new();
        };
        current = prev;
        path.push(route_key_pos(current));
    }
    path.reverse();
    path
}

fn append_axis_steps(path: &mut Vec<Vec2i>, to: Vec2i) {
    let Some(mut current) = path.last().copied() else {
        return;
    };
    let dx = to.x.saturating_sub(current.x);
    let dy = to.y.saturating_sub(current.y);
    let step_x = dx.signum();
    let step_y = dy.signum();
    if step_x != 0 && step_y != 0 {
        return;
    }
    while current != to {
        current = Vec2i::new(
            current.x.saturating_add(step_x),
            current.y.saturating_add(step_y),
        );
        path.push(current);
    }
}

fn orthogonal_path_via_corner(start: Vec2i, corner: Vec2i, end: Vec2i) -> Vec<Vec2i> {
    let mut path = vec![start];
    append_axis_steps(&mut path, corner);
    append_axis_steps(&mut path, end);
    path
}

fn fallback_skill_link_route(
    start: Vec2i,
    end: Vec2i,
    occupancy: &HashSet<(i32, i32)>,
    bounds: RouteBounds,
    existing_routes: &HashMap<(i32, i32), u32>,
) -> Vec<Vec2i> {
    if start == end {
        return vec![start];
    }

    let mut corners = vec![Vec2i::new(end.x, start.y), Vec2i::new(start.x, end.y)];
    corners.dedup();
    let mut best: Option<((i32, i32, i32, usize), Vec<Vec2i>)> = None;
    for (candidate_idx, corner) in corners.into_iter().enumerate() {
        let candidate = orthogonal_path_via_corner(start, corner, end);
        if candidate.is_empty() {
            continue;
        }
        let mut out_of_bounds_count = 0i32;
        let mut blocked_count = 0i32;
        let mut overlap_cost = 0i32;
        for (idx, cell) in candidate.iter().copied().enumerate() {
            if !bounds.contains(cell) {
                out_of_bounds_count = out_of_bounds_count.saturating_add(1);
            }
            if idx > 0 && idx + 1 < candidate.len() && occupancy.contains(&world_key(cell)) {
                blocked_count = blocked_count.saturating_add(1);
            }
            overlap_cost = overlap_cost.saturating_add(
                existing_routes
                    .get(&world_key(cell))
                    .copied()
                    .unwrap_or_default() as i32,
            );
        }
        let score = (
            out_of_bounds_count,
            blocked_count,
            overlap_cost,
            candidate_idx,
        );
        let replace = best
            .as_ref()
            .map(|(best_score, _)| score < *best_score)
            .unwrap_or(true);
        if replace {
            best = Some((score, candidate));
        }
    }

    best.map(|(_, route)| route)
        .unwrap_or_else(|| orthogonal_path_via_corner(start, Vec2i::new(end.x, start.y), end))
}

fn route_skill_link(
    world_start_port: Vec2i,
    world_end_port: Vec2i,
    occupancy: &HashSet<(i32, i32)>,
    bounds: RouteBounds,
    existing_routes: &HashMap<(i32, i32), u32>,
) -> Vec<Vec2i> {
    if world_start_port == world_end_port {
        return vec![world_start_port];
    }

    let start_key: RouteNodeKey = ((world_start_port.x, world_start_port.y), None);
    let mut open = BinaryHeap::new();
    let mut g_score: HashMap<RouteNodeKey, i32> = HashMap::new();
    let mut came_from: HashMap<RouteNodeKey, RouteNodeKey> = HashMap::new();
    g_score.insert(start_key, 0);
    let mut serial_counter = 1usize;
    open.push(OpenRouteNode {
        key: start_key,
        g: 0,
        f: manhattan_distance(world_start_port, world_end_port)
            .saturating_mul(SKILLTREE_ROUTE_STEP_COST),
        serial: 0,
    });

    let width = bounds
        .max
        .x
        .saturating_sub(bounds.min.x)
        .saturating_add(1)
        .max(1) as usize;
    let height = bounds
        .max
        .y
        .saturating_sub(bounds.min.y)
        .saturating_add(1)
        .max(1) as usize;
    let max_expansions = width.saturating_mul(height).saturating_mul(8).max(256);
    let mut expansions = 0usize;

    while let Some(current) = open.pop() {
        expansions = expansions.saturating_add(1);
        if expansions > max_expansions {
            break;
        }

        let Some(best_seen_g) = g_score.get(&current.key).copied() else {
            continue;
        };
        if current.g > best_seen_g {
            continue;
        }

        let current_pos = route_key_pos(current.key);
        if current_pos == world_end_port {
            let route = reconstruct_route_path(start_key, current.key, &came_from);
            if route.len() >= 2 {
                return route;
            }
            break;
        }

        for dir in CardinalDir::ORDER {
            let delta = dir.delta();
            let next_pos = Vec2i::new(
                current_pos.x.saturating_add(delta.x),
                current_pos.y.saturating_add(delta.y),
            );
            if !bounds.contains(next_pos) {
                continue;
            }
            if next_pos != world_start_port
                && next_pos != world_end_port
                && occupancy.contains(&world_key(next_pos))
            {
                continue;
            }

            let mut step_cost = SKILLTREE_ROUTE_STEP_COST;
            if let Some(prev_dir) = current.key.1 {
                if prev_dir != dir {
                    step_cost = step_cost.saturating_add(SKILLTREE_ROUTE_TURN_PENALTY);
                }
            }
            let overlap_penalty = existing_routes
                .get(&world_key(next_pos))
                .copied()
                .unwrap_or_default() as i32;
            step_cost = step_cost
                .saturating_add(overlap_penalty.saturating_mul(SKILLTREE_ROUTE_OVERLAP_PENALTY));
            let next_g = current.g.saturating_add(step_cost);
            let next_key: RouteNodeKey = ((next_pos.x, next_pos.y), Some(dir));
            if next_g >= g_score.get(&next_key).copied().unwrap_or(i32::MAX) {
                continue;
            }
            g_score.insert(next_key, next_g);
            came_from.insert(next_key, current.key);
            let heuristic = manhattan_distance(next_pos, world_end_port)
                .saturating_mul(SKILLTREE_ROUTE_STEP_COST);
            open.push(OpenRouteNode {
                key: next_key,
                g: next_g,
                f: next_g.saturating_add(heuristic),
                serial: serial_counter,
            });
            serial_counter = serial_counter.saturating_add(1);
        }
    }

    fallback_skill_link_route(
        world_start_port,
        world_end_port,
        occupancy,
        bounds,
        existing_routes,
    )
}

fn step_direction(from: Vec2i, to: Vec2i) -> Option<CardinalDir> {
    let dx = to.x.saturating_sub(from.x);
    let dy = to.y.saturating_sub(from.y);
    if dy == 0 && dx > 0 {
        Some(CardinalDir::Right)
    } else if dy == 0 && dx < 0 {
        Some(CardinalDir::Left)
    } else if dx == 0 && dy > 0 {
        Some(CardinalDir::Up)
    } else if dx == 0 && dy < 0 {
        Some(CardinalDir::Down)
    } else {
        None
    }
}

fn compress_route_corners(route: &[Vec2i]) -> Vec<Vec2i> {
    if route.len() <= 2 {
        return route.to_vec();
    }
    let mut corners = vec![route[0]];
    let mut prev_dir = step_direction(route[0], route[1]);
    for idx in 1..route.len().saturating_sub(1) {
        let curr_dir = step_direction(route[idx], route[idx + 1]);
        if curr_dir != prev_dir {
            corners.push(route[idx]);
            prev_dir = curr_dir;
        }
    }
    if let Some(last) = route.last().copied() {
        corners.push(last);
    }
    corners.dedup();
    corners
}

fn draw_axis_segment_i32(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    from: (i32, i32),
    to: (i32, i32),
    color: [u8; 4],
    thickness: u32,
) {
    let t = thickness.max(1);
    let half = (t / 2) as i32;
    if from.0 == to.0 {
        let y0 = from.1.min(to.1);
        let y1 = from.1.max(to.1);
        let h = y1.saturating_sub(y0).saturating_add(1) as u32;
        fill_rect_i32(
            frame,
            width,
            height,
            from.0.saturating_sub(half),
            y0,
            t,
            h,
            color,
        );
    } else if from.1 == to.1 {
        let x0 = from.0.min(to.0);
        let x1 = from.0.max(to.0);
        let w = x1.saturating_sub(x0).saturating_add(1) as u32;
        fill_rect_i32(
            frame,
            width,
            height,
            x0,
            from.1.saturating_sub(half),
            w,
            t,
            color,
        );
    }
}

fn draw_orth_arrow_cap(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    tip: (i32, i32),
    dir: CardinalDir,
    color: [u8; 4],
    thickness: u32,
) {
    let length = SKILLTREE_ARROW_CAP_LENGTH.max(thickness as i32 + 1);
    let spread = SKILLTREE_ARROW_CAP_SPREAD.max((thickness as i32 / 2).saturating_add(1));
    match dir {
        CardinalDir::Right => {
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_sub(length), tip.1.saturating_sub(spread)),
                (tip.0, tip.1.saturating_sub(spread)),
                color,
                thickness,
            );
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_sub(length), tip.1.saturating_add(spread)),
                (tip.0, tip.1.saturating_add(spread)),
                color,
                thickness,
            );
        }
        CardinalDir::Left => {
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_add(length), tip.1.saturating_sub(spread)),
                (tip.0, tip.1.saturating_sub(spread)),
                color,
                thickness,
            );
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_add(length), tip.1.saturating_add(spread)),
                (tip.0, tip.1.saturating_add(spread)),
                color,
                thickness,
            );
        }
        CardinalDir::Up => {
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_sub(spread), tip.1.saturating_add(length)),
                (tip.0.saturating_sub(spread), tip.1),
                color,
                thickness,
            );
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_add(spread), tip.1.saturating_add(length)),
                (tip.0.saturating_add(spread), tip.1),
                color,
                thickness,
            );
        }
        CardinalDir::Down => {
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_sub(spread), tip.1.saturating_sub(length)),
                (tip.0.saturating_sub(spread), tip.1),
                color,
                thickness,
            );
            draw_axis_segment_i32(
                frame,
                width,
                height,
                (tip.0.saturating_add(spread), tip.1.saturating_sub(length)),
                (tip.0.saturating_add(spread), tip.1),
                color,
                thickness,
            );
        }
    }
}

fn draw_routed_skilltree_link(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    route: &[Vec2i],
    source_port: SkilltreeBoundaryPort,
    target_port: SkilltreeBoundaryPort,
    transform: SkilltreeGridTransform,
    color: [u8; 4],
    thickness: u32,
) {
    if route.len() < 2 {
        return;
    }
    let corners = compress_route_corners(route);
    if corners.len() < 2 {
        return;
    }

    // Draw a short stub from source node edge to the routed port center so links
    // visually terminate on node geometry.
    let source_tip =
        transform.world_cell_edge_mid_px(source_port.touch_cell, source_port.outward_dir);
    let route_start_px = transform.world_cell_center_px(route[0]);
    draw_axis_segment_i32(
        frame,
        width,
        height,
        source_tip,
        route_start_px,
        color,
        thickness,
    );

    for segment in corners.windows(2) {
        let from_px = transform.world_cell_center_px(segment[0]);
        let to_px = transform.world_cell_center_px(segment[1]);
        draw_axis_segment_i32(frame, width, height, from_px, to_px, color, thickness);
    }

    let route_end_px = transform.world_cell_center_px(*route.last().unwrap_or(&route[0]));
    let target_tip =
        transform.world_cell_edge_mid_px(target_port.touch_cell, target_port.outward_dir);
    draw_axis_segment_i32(
        frame,
        width,
        height,
        route_end_px,
        target_tip,
        color,
        thickness,
    );
    draw_orth_arrow_cap(
        frame,
        width,
        height,
        target_tip,
        target_port.outward_dir.opposite(),
        color,
        thickness,
    );
}

fn draw_skilltree_links(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    def: &SkillTreeDef,
    transform: SkilltreeGridTransform,
) {
    let geometry = build_skilltree_link_geometry(def);
    let route_bounds = skilltree_route_bounds(def);
    let mut existing_routes: HashMap<(i32, i32), u32> = HashMap::new();
    for (source_idx, target_idx) in ordered_skilltree_edge_indices(def) {
        let source_node = &def.nodes[source_idx];
        let target_node = &def.nodes[target_idx];
        let Some(source_data) = geometry.nodes.get(source_node.id.as_str()) else {
            continue;
        };
        let Some(target_data) = geometry.nodes.get(target_node.id.as_str()) else {
            continue;
        };
        let Some((source_port, target_port)) = choose_link_ports(source_data, target_data) else {
            continue;
        };
        let route = route_skill_link(
            source_port.route_cell,
            target_port.route_cell,
            &geometry.occupancy,
            route_bounds,
            &existing_routes,
        );
        if route.len() < 2 {
            continue;
        }
        draw_routed_skilltree_link(
            frame,
            width,
            height,
            &route,
            source_port,
            target_port,
            transform,
            COLOR_SKILLTREE_LINK,
            SKILLTREE_LINK_THICKNESS,
        );
        for cell in route {
            let entry = existing_routes.entry(world_key(cell)).or_insert(0);
            *entry = entry.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod routing_tests {
    use super::*;

    fn assert_path_is_orthogonal_adjacent(path: &[Vec2i]) {
        assert!(
            path.len() >= 2,
            "expected path with at least start/end points"
        );
        for step in path.windows(2) {
            let dx = step[1].x.saturating_sub(step[0].x).abs();
            let dy = step[1].y.saturating_sub(step[0].y).abs();
            assert_eq!(
                dx.saturating_add(dy),
                1,
                "expected unit cardinal step between {:?} and {:?}",
                step[0],
                step[1]
            );
        }
    }

    #[test]
    fn route_skill_link_returns_orthogonal_adjacent_steps() {
        let occupancy = HashSet::new();
        let existing_routes = HashMap::new();
        let bounds = RouteBounds {
            min: Vec2i::new(-8, -8),
            max: Vec2i::new(8, 8),
        };
        let start = Vec2i::new(0, 0);
        let end = Vec2i::new(4, 3);
        let route = route_skill_link(start, end, &occupancy, bounds, &existing_routes);
        assert_eq!(route.first(), Some(&start));
        assert_eq!(route.last(), Some(&end));
        assert_path_is_orthogonal_adjacent(&route);
    }

    #[test]
    fn route_skill_link_avoids_occupied_cells() {
        let mut occupancy = HashSet::new();
        occupancy.insert((2, 0));
        occupancy.insert((3, 0));
        let existing_routes = HashMap::new();
        let bounds = RouteBounds {
            min: Vec2i::new(-8, -8),
            max: Vec2i::new(8, 8),
        };
        let start = Vec2i::new(1, 0);
        let end = Vec2i::new(5, 0);
        let route = route_skill_link(start, end, &occupancy, bounds, &existing_routes);
        assert_eq!(route.first(), Some(&start));
        assert_eq!(route.last(), Some(&end));
        for cell in route
            .iter()
            .copied()
            .skip(1)
            .take(route.len().saturating_sub(2))
        {
            assert!(
                !occupancy.contains(&world_key(cell)),
                "route should avoid occupied interior cells, found {:?}",
                cell
            );
        }
        assert_path_is_orthogonal_adjacent(&route);
    }

    #[test]
    fn route_skill_link_is_deterministic() {
        let mut occupancy = HashSet::new();
        occupancy.insert((2, 0));
        let mut existing_routes = HashMap::new();
        existing_routes.insert((1, 1), 1);
        let bounds = RouteBounds {
            min: Vec2i::new(-8, -8),
            max: Vec2i::new(8, 8),
        };
        let start = Vec2i::new(0, 0);
        let end = Vec2i::new(4, 0);
        let first = route_skill_link(start, end, &occupancy, bounds, &existing_routes);
        let second = route_skill_link(start, end, &occupancy, bounds, &existing_routes);
        assert_eq!(first, second, "router should be deterministic");
    }

    #[test]
    fn route_skill_link_falls_back_to_dogleg_when_no_route_exists() {
        let mut occupancy = HashSet::new();
        for x in 0..=4 {
            occupancy.insert((x, 2));
        }
        let existing_routes = HashMap::new();
        let bounds = RouteBounds {
            min: Vec2i::new(0, 0),
            max: Vec2i::new(4, 4),
        };
        let start = Vec2i::new(0, 0);
        let end = Vec2i::new(4, 4);
        let route = route_skill_link(start, end, &occupancy, bounds, &existing_routes);
        assert_eq!(route.first(), Some(&start));
        assert_eq!(route.last(), Some(&end));
        assert_path_is_orthogonal_adjacent(&route);
        assert!(
            route
                .iter()
                .any(|cell| occupancy.contains(&world_key(*cell))),
            "fallback path should still return an orthogonal route under full blockage"
        );
    }
}
