use std::time::{Duration, Instant};

use engine::HeadlessRunner;
use engine::app::InputFrame;
use engine::ui_tree::UiAction;
use winit::event::VirtualKeyCode;

use super::skilltree_camera as headful_camera;
use super::view_transitions as headful_view;
use crate::playtest::{InputAction, TetrisLogic};
use crate::skilltree::{SkillTreeEditorTool, SkillTreeRuntime};
use crate::tetris_core::Vec2i;
use crate::tetris_ui::SkillTreeLayout;
use crate::ui_ids::{
    ACTION_GAME_OVER_QUIT, ACTION_GAME_OVER_RESTART, ACTION_GAME_OVER_SKILLTREE,
    ACTION_MAIN_MENU_QUIT, ACTION_MAIN_MENU_SKILLTREE_EDITOR, ACTION_MAIN_MENU_START,
    ACTION_PAUSE_END_RUN, ACTION_PAUSE_RESUME, ACTION_SKILLTREE_START_RUN,
    ACTION_SKILLTREE_TOOL_ADD_CELL, ACTION_SKILLTREE_TOOL_LINK, ACTION_SKILLTREE_TOOL_MOVE,
    ACTION_SKILLTREE_TOOL_REMOVE_CELL, ACTION_SKILLTREE_TOOL_SELECT,
};
use crate::view::GameView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalDir {
    Left,
    Right,
}

#[derive(Debug, Default)]
pub struct HorizontalRepeat {
    pub left_down: bool,
    pub right_down: bool,
    pub active: Option<HorizontalDir>,
    pub next_repeat_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadfulInputCommand {
    ToggleDebugHud,
    ToggleMusic,
    ExitRequested,
    PlayClick,
    ResetRun,
    ApplyAction(InputAction),
}

#[derive(Debug, Default)]
pub struct UiActionResult {
    pub handled: bool,
    pub commands: Vec<HeadfulInputCommand>,
}

impl HorizontalRepeat {
    // Roughly "DAS/ARR"-ish defaults, but the key property is that repeating is driven by our
    // own timer, not the OS key-repeat (so it won't get interrupted by other keypresses).
    pub const REPEAT_DELAY: Duration = Duration::from_millis(170);
    pub const REPEAT_INTERVAL: Duration = Duration::from_millis(50);

    pub fn clear(&mut self) {
        self.left_down = false;
        self.right_down = false;
        self.active = None;
        self.next_repeat_at = None;
    }

    pub fn on_press(&mut self, dir: HorizontalDir, now: Instant) -> bool {
        let was_down = match dir {
            HorizontalDir::Left => self.left_down,
            HorizontalDir::Right => self.right_down,
        };
        if was_down {
            // Ignore OS key-repeat "Pressed" events; repeating is handled by `next_repeat_action`.
            return false;
        }

        match dir {
            HorizontalDir::Left => self.left_down = true,
            HorizontalDir::Right => self.right_down = true,
        }

        self.active = Some(dir);
        self.next_repeat_at = Some(now + Self::REPEAT_DELAY);
        true
    }

    pub fn on_release(&mut self, dir: HorizontalDir, now: Instant) {
        match dir {
            HorizontalDir::Left => self.left_down = false,
            HorizontalDir::Right => self.right_down = false,
        }

        if self.active != Some(dir) {
            return;
        }

        // If the active direction was released, fall back to the other one if still held.
        let new_active = match dir {
            HorizontalDir::Left if self.right_down => Some(HorizontalDir::Right),
            HorizontalDir::Right if self.left_down => Some(HorizontalDir::Left),
            _ => None,
        };

        self.active = new_active;
        self.next_repeat_at = new_active.map(|_| now + Self::REPEAT_DELAY);
    }

    pub fn next_repeat_action(&mut self, now: Instant) -> Option<InputAction> {
        let dir = self.active?;
        let next_at = self.next_repeat_at?;
        if now < next_at {
            return None;
        }

        self.next_repeat_at = Some(now + Self::REPEAT_INTERVAL);
        Some(match dir {
            HorizontalDir::Left => InputAction::MoveLeft,
            HorizontalDir::Right => InputAction::MoveRight,
        })
    }
}

pub fn sync_horizontal_repeat_from_frame<F>(
    input: &InputFrame,
    repeat: &mut HorizontalRepeat,
    now: Instant,
    mut on_initial_action: F,
) where
    F: FnMut(InputAction),
{
    let left_down_now = input.keys_down.contains(&VirtualKeyCode::Left);
    let right_down_now = input.keys_down.contains(&VirtualKeyCode::Right)
        || input.keys_down.contains(&VirtualKeyCode::D);
    let right_released = input.keys_released.contains(&VirtualKeyCode::Right)
        || input.keys_released.contains(&VirtualKeyCode::D);

    if input.keys_released.contains(&VirtualKeyCode::Left) && !left_down_now && repeat.left_down {
        repeat.on_release(HorizontalDir::Left, now);
    }
    if right_released && !right_down_now && repeat.right_down {
        repeat.on_release(HorizontalDir::Right, now);
    }

    if left_down_now && !repeat.left_down {
        if repeat.on_press(HorizontalDir::Left, now) {
            on_initial_action(InputAction::MoveLeft);
        }
    } else if !left_down_now && repeat.left_down {
        repeat.on_release(HorizontalDir::Left, now);
    }

    if right_down_now && !repeat.right_down {
        if repeat.on_press(HorizontalDir::Right, now) {
            on_initial_action(InputAction::MoveRight);
        }
    } else if !right_down_now && repeat.right_down {
        repeat.on_release(HorizontalDir::Right, now);
    }
}

pub fn map_key_to_action(key: VirtualKeyCode) -> Option<InputAction> {
    match key {
        VirtualKeyCode::Left => Some(InputAction::MoveLeft),
        VirtualKeyCode::Right | VirtualKeyCode::D => Some(InputAction::MoveRight),
        VirtualKeyCode::Down | VirtualKeyCode::S => Some(InputAction::SoftDrop),
        VirtualKeyCode::Up | VirtualKeyCode::W => Some(InputAction::RotateCw),
        VirtualKeyCode::Z => Some(InputAction::RotateCcw),
        VirtualKeyCode::X => Some(InputAction::RotateCw),
        VirtualKeyCode::A => Some(InputAction::Rotate180),
        VirtualKeyCode::Space => Some(InputAction::HardDrop),
        VirtualKeyCode::C => Some(InputAction::Hold),
        _ => None,
    }
}

pub fn should_play_action_sfx(action: InputAction) -> bool {
    // Gameplay actions happen very frequently; only hard drop gets a click SFX.
    matches!(action, InputAction::HardDrop)
}

fn ctrl_down(input: &InputFrame) -> bool {
    input.keys_down.contains(&VirtualKeyCode::LControl)
        || input.keys_down.contains(&VirtualKeyCode::RControl)
}

fn shift_down(input: &InputFrame) -> bool {
    input.keys_down.contains(&VirtualKeyCode::LShift)
        || input.keys_down.contains(&VirtualKeyCode::RShift)
}

fn sorted_pressed_keys(input: &InputFrame) -> Vec<VirtualKeyCode> {
    let mut keys: Vec<VirtualKeyCode> = input.keys_pressed.iter().copied().collect();
    keys.sort_by_key(|k| *k as u32);
    keys
}

fn search_char_for_key(key: VirtualKeyCode, shift: bool) -> Option<char> {
    let alpha = |lower: char| {
        Some(if shift {
            lower.to_ascii_uppercase()
        } else {
            lower
        })
    };
    match key {
        VirtualKeyCode::A => alpha('a'),
        VirtualKeyCode::B => alpha('b'),
        VirtualKeyCode::C => alpha('c'),
        VirtualKeyCode::D => alpha('d'),
        VirtualKeyCode::E => alpha('e'),
        VirtualKeyCode::F => alpha('f'),
        VirtualKeyCode::G => alpha('g'),
        VirtualKeyCode::H => alpha('h'),
        VirtualKeyCode::I => alpha('i'),
        VirtualKeyCode::J => alpha('j'),
        VirtualKeyCode::K => alpha('k'),
        VirtualKeyCode::L => alpha('l'),
        VirtualKeyCode::M => alpha('m'),
        VirtualKeyCode::N => alpha('n'),
        VirtualKeyCode::O => alpha('o'),
        VirtualKeyCode::P => alpha('p'),
        VirtualKeyCode::Q => alpha('q'),
        VirtualKeyCode::R => alpha('r'),
        VirtualKeyCode::S => alpha('s'),
        VirtualKeyCode::T => alpha('t'),
        VirtualKeyCode::U => alpha('u'),
        VirtualKeyCode::V => alpha('v'),
        VirtualKeyCode::W => alpha('w'),
        VirtualKeyCode::X => alpha('x'),
        VirtualKeyCode::Y => alpha('y'),
        VirtualKeyCode::Z => alpha('z'),
        VirtualKeyCode::Key0 => Some('0'),
        VirtualKeyCode::Key1 => Some('1'),
        VirtualKeyCode::Key2 => Some('2'),
        VirtualKeyCode::Key3 => Some('3'),
        VirtualKeyCode::Key4 => Some('4'),
        VirtualKeyCode::Key5 => Some('5'),
        VirtualKeyCode::Key6 => Some('6'),
        VirtualKeyCode::Key7 => Some('7'),
        VirtualKeyCode::Key8 => Some('8'),
        VirtualKeyCode::Key9 => Some('9'),
        VirtualKeyCode::Space => Some(' '),
        VirtualKeyCode::Minus => Some(if shift { '_' } else { '-' }),
        _ => None,
    }
}

fn pan_skilltree_camera(skilltree: &mut SkillTreeRuntime, dx_cells: f32, dy_cells: f32) {
    skilltree.camera.pan.x += dx_cells;
    skilltree.camera.pan.y += dy_cells;
    skilltree.camera.target_pan = skilltree.camera.pan;
}

fn reset_skilltree_camera(skilltree: &mut SkillTreeRuntime, layout: SkillTreeLayout) {
    skilltree.camera.pan.x = 0.0;
    skilltree.camera.pan.y = 0.0;
    skilltree.camera.target_pan = skilltree.camera.pan;
    skilltree.camera.cell_px = 20.0;
    skilltree.camera.target_cell_px = 20.0;
    headful_camera::clamp_skilltree_camera_to_bounds(skilltree, layout.grid_cols, layout.grid_rows);
    skilltree.editor.status = Some("CAMERA RESET".to_string());
}

fn center_skilltree_camera_on_world(
    skilltree: &mut SkillTreeRuntime,
    layout: SkillTreeLayout,
    world: Vec2i,
) {
    if layout.grid_cols == 0 || layout.grid_rows == 0 {
        return;
    }
    let cam_min_x = world.x as f32 - (layout.grid_cols as f32 * 0.5);
    let cam_min_y = world.y as f32 - (layout.grid_rows as f32 * 0.5);
    let default_cam_min_x = (-(layout.grid_cols as i32) / 2) as f32;
    let default_cam_min_y = 0.0;
    skilltree.camera.pan.x = cam_min_x - default_cam_min_x;
    skilltree.camera.pan.y = cam_min_y - default_cam_min_y;
    skilltree.camera.target_pan = skilltree.camera.pan;
    headful_camera::clamp_skilltree_camera_to_bounds(skilltree, layout.grid_cols, layout.grid_rows);
}

fn focus_skilltree_camera_on_selected(
    skilltree: &mut SkillTreeRuntime,
    layout: SkillTreeLayout,
) -> bool {
    let Some(selected) = skilltree.editor.selected.clone() else {
        return false;
    };
    let Some(idx) = skilltree.node_index(&selected) else {
        return false;
    };
    let node = &skilltree.def.nodes[idx];
    let mut sum_x: i64 = 0;
    let mut sum_y: i64 = 0;
    let mut count: i64 = 0;
    for rel in &node.shape {
        sum_x += (node.pos.x + rel.x) as i64;
        sum_y += (node.pos.y + rel.y) as i64;
        count += 1;
    }
    let world = if count > 0 {
        Vec2i::new((sum_x / count) as i32, (sum_y / count) as i32)
    } else {
        node.pos
    };
    center_skilltree_camera_on_world(skilltree, layout, world);
    skilltree.editor.status = Some(format!("FOCUS {selected}"));
    true
}

fn apply_editor_tool_at_world(skilltree: &mut SkillTreeRuntime, world: Vec2i) -> bool {
    skilltree.editor_set_cursor_world(world);
    let hit_id = headful_camera::skilltree_node_at_world(skilltree, world).map(|s| s.to_string());
    match skilltree.editor.tool {
        SkillTreeEditorTool::Select => {
            if let Some(id) = hit_id {
                skilltree.editor_select(&id, None);
                true
            } else {
                skilltree.editor_clear_selection();
                false
            }
        }
        SkillTreeEditorTool::Move => {
            if let Some(id) = hit_id {
                if let Some(idx) = skilltree.node_index(&id) {
                    let pos = skilltree.def.nodes[idx].pos;
                    let grab = Vec2i::new(world.x - pos.x, world.y - pos.y);
                    skilltree.editor_select(&id, Some(grab));
                } else {
                    skilltree.editor_select(&id, None);
                }
                true
            } else {
                let grab = skilltree
                    .editor
                    .move_grab_offset
                    .unwrap_or(Vec2i::new(0, 0));
                let new_pos = Vec2i::new(world.x - grab.x, world.y - grab.y);
                skilltree.editor_move_selected_to(new_pos)
            }
        }
        SkillTreeEditorTool::AddCell => {
            if let Some(id) = hit_id {
                let already = skilltree.editor.selected.as_deref() == Some(id.as_str());
                if !already {
                    skilltree.editor_select(&id, None);
                    return true;
                }
            }
            skilltree.editor_add_cell_at_world(world)
        }
        SkillTreeEditorTool::RemoveCell => {
            if let Some(id) = hit_id {
                if skilltree.editor.selected.as_deref() != Some(id.as_str()) {
                    skilltree.editor_select(&id, None);
                    return true;
                }
            }
            skilltree.editor_remove_cell_at_world(world)
        }
        SkillTreeEditorTool::ConnectPrereqs => {
            if let Some(id) = hit_id {
                if let Some(from) = skilltree.editor.connect_from.clone() {
                    if from == id {
                        skilltree.editor.connect_from = None;
                        skilltree.editor.status = Some("LINK SOURCE CLEARED".to_string());
                        return false;
                    }
                    if skilltree.editor_toggle_prereq(&from, &id) {
                        skilltree.editor.status =
                            Some(format!("LINK {from} -> {id} (SOURCE {from})"));
                        return true;
                    }
                } else {
                    skilltree.editor.connect_from = Some(id.clone());
                    skilltree.editor.status = Some(format!("LINK SOURCE {id}"));
                    return true;
                }
            }
            false
        }
    }
}

pub fn process_keyboard_frame(
    runner: &mut HeadlessRunner<TetrisLogic>,
    input: &InputFrame,
    now: Instant,
    horizontal_repeat: &mut HorizontalRepeat,
    last_skilltree: SkillTreeLayout,
    _mouse_x: u32,
    _mouse_y: u32,
) -> Vec<HeadfulInputCommand> {
    let pressed = |key| input.keys_pressed.contains(&key);
    let mut commands = Vec::new();

    if pressed(VirtualKeyCode::F3) {
        commands.push(HeadfulInputCommand::ToggleDebugHud);
    }
    if pressed(VirtualKeyCode::M) {
        commands.push(HeadfulInputCommand::ToggleMusic);
    }

    let mut view = runner.state().view;
    match view {
        GameView::MainMenu => {
            if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                let transition = headful_view::start_game(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                if transition.reset_tetris {
                    commands.push(HeadfulInputCommand::ResetRun);
                }
                commands.push(HeadfulInputCommand::PlayClick);
            } else if pressed(VirtualKeyCode::K) {
                let transition = headful_view::open_skilltree_editor(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                horizontal_repeat.clear();
                let skilltree = &mut runner.state_mut().skilltree;
                if !skilltree.editor.enabled {
                    skilltree.editor_toggle();
                }
                commands.push(HeadfulInputCommand::PlayClick);
            } else if pressed(VirtualKeyCode::Escape) {
                commands.push(HeadfulInputCommand::ExitRequested);
            }
        }
        GameView::SkillTree => {
            if pressed(VirtualKeyCode::F4) {
                let skilltree = &mut runner.state_mut().skilltree;
                skilltree.editor_toggle();
                commands.push(HeadfulInputCommand::PlayClick);
                return commands;
            }

            let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
            if skilltree_editor_enabled {
                if pressed(VirtualKeyCode::Escape) {
                    {
                        let skilltree = &mut runner.state_mut().skilltree;
                        if skilltree.editor.search_open {
                            skilltree.editor_close_search();
                            commands.push(HeadfulInputCommand::PlayClick);
                            return commands;
                        }
                        skilltree.editor_toggle();
                    }
                    let transition = headful_view::back(view);
                    view = transition.next_view;
                    runner.state_mut().view = view;
                    horizontal_repeat.clear();
                    commands.push(HeadfulInputCommand::PlayClick);
                    return commands;
                }

                let skilltree = &mut runner.state_mut().skilltree;
                let shift = shift_down(input);
                let ctrl = ctrl_down(input);

                if pressed(VirtualKeyCode::Slash) && shift {
                    skilltree.editor_toggle_help_overlay();
                    commands.push(HeadfulInputCommand::PlayClick);
                }

                if skilltree.editor.search_open {
                    if pressed(VirtualKeyCode::Escape) {
                        skilltree.editor_close_search();
                        commands.push(HeadfulInputCommand::PlayClick);
                        return commands;
                    }
                    if pressed(VirtualKeyCode::Back) {
                        skilltree.editor_pop_search_char();
                    }
                    for key in sorted_pressed_keys(input) {
                        if let Some(c) = search_char_for_key(key, shift) {
                            skilltree.editor_append_search_char(c);
                        }
                    }
                    let query = skilltree.editor.search_query.clone();
                    if pressed(VirtualKeyCode::Return) {
                        if let Some(id) = skilltree.editor_select_matching(&query) {
                            let _ = focus_skilltree_camera_on_selected(skilltree, last_skilltree);
                            skilltree.editor.search_open = false;
                            skilltree.editor.search_query.clear();
                            skilltree.editor.status = Some(format!("JUMP {id}"));
                            commands.push(HeadfulInputCommand::PlayClick);
                        } else {
                            skilltree.editor.status = Some(format!("NO MATCH: {query}"));
                        }
                    } else {
                        skilltree.editor.status =
                            Some(format!("SEARCH {}", skilltree.editor.search_query));
                    }
                    return commands;
                }

                if pressed(VirtualKeyCode::Slash) && !shift {
                    skilltree.editor_open_search();
                    commands.push(HeadfulInputCommand::PlayClick);
                    return commands;
                }

                let mut cursor = skilltree.editor.cursor_world;
                let mut cursor_moved = false;

                if ctrl && pressed(VirtualKeyCode::Z) {
                    if skilltree.editor_undo() {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                }
                if ctrl && pressed(VirtualKeyCode::Y) {
                    if skilltree.editor_redo() {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                }

                if ctrl && pressed(VirtualKeyCode::D) {
                    if skilltree.editor_duplicate_selected().is_some() {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                }

                if shift {
                    if pressed(VirtualKeyCode::J)
                        && skilltree.editor_nudge_selected_by(Vec2i::new(-1, 0))
                    {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                    if pressed(VirtualKeyCode::L)
                        && skilltree.editor_nudge_selected_by(Vec2i::new(1, 0))
                    {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                    if pressed(VirtualKeyCode::I)
                        && skilltree.editor_nudge_selected_by(Vec2i::new(0, 1))
                    {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                    if pressed(VirtualKeyCode::K)
                        && skilltree.editor_nudge_selected_by(Vec2i::new(0, -1))
                    {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                } else {
                    if pressed(VirtualKeyCode::J) {
                        cursor.x = cursor.x.saturating_sub(1);
                        cursor_moved = true;
                    }
                    if pressed(VirtualKeyCode::L) {
                        cursor.x = cursor.x.saturating_add(1);
                        cursor_moved = true;
                    }
                    if pressed(VirtualKeyCode::I) {
                        cursor.y = cursor.y.saturating_add(1);
                        cursor_moved = true;
                    }
                    if pressed(VirtualKeyCode::K) {
                        cursor.y = cursor.y.saturating_sub(1);
                        cursor_moved = true;
                    }
                }

                if cursor_moved {
                    skilltree.editor_set_cursor_world(cursor);
                }

                let mut set_tool = |tool: SkillTreeEditorTool| {
                    if skilltree.editor.tool != tool {
                        skilltree.editor_set_tool(tool);
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                };
                if pressed(VirtualKeyCode::Key1) {
                    set_tool(SkillTreeEditorTool::Select);
                }
                if pressed(VirtualKeyCode::Key2) {
                    set_tool(SkillTreeEditorTool::Move);
                }
                if pressed(VirtualKeyCode::Key3) {
                    set_tool(SkillTreeEditorTool::AddCell);
                }
                if pressed(VirtualKeyCode::Key4) {
                    set_tool(SkillTreeEditorTool::RemoveCell);
                }
                if pressed(VirtualKeyCode::Key5) {
                    set_tool(SkillTreeEditorTool::ConnectPrereqs);
                }
                if pressed(VirtualKeyCode::Tab) {
                    skilltree.editor_cycle_tool();
                    commands.push(HeadfulInputCommand::PlayClick);
                }

                let pan_step = if shift { 4.0 } else { 1.0 };
                if pressed(VirtualKeyCode::Left) {
                    pan_skilltree_camera(skilltree, -pan_step, 0.0);
                }
                if pressed(VirtualKeyCode::Right) {
                    pan_skilltree_camera(skilltree, pan_step, 0.0);
                }
                if pressed(VirtualKeyCode::Up) {
                    pan_skilltree_camera(skilltree, 0.0, pan_step);
                }
                if pressed(VirtualKeyCode::Down) {
                    pan_skilltree_camera(skilltree, 0.0, -pan_step);
                }
                if pressed(VirtualKeyCode::Key0) {
                    reset_skilltree_camera(skilltree, last_skilltree);
                    commands.push(HeadfulInputCommand::PlayClick);
                }
                if pressed(VirtualKeyCode::F)
                    && focus_skilltree_camera_on_selected(skilltree, last_skilltree)
                {
                    commands.push(HeadfulInputCommand::PlayClick);
                }
                if pressed(VirtualKeyCode::Minus) {
                    skilltree.camera.cell_px = (skilltree.camera.cell_px - 2.0)
                        .max(headful_camera::SKILLTREE_CAMERA_MIN_CELL_PX);
                    skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                }
                if pressed(VirtualKeyCode::Equals) {
                    skilltree.camera.cell_px = (skilltree.camera.cell_px + 2.0)
                        .min(headful_camera::SKILLTREE_CAMERA_MAX_CELL_PX);
                    skilltree.camera.target_cell_px = skilltree.camera.cell_px;
                }
                if pressed(VirtualKeyCode::N) {
                    let world = skilltree.editor.cursor_world;
                    let _ = skilltree.editor_create_node_at(world);
                    commands.push(HeadfulInputCommand::PlayClick);
                }
                if pressed(VirtualKeyCode::Delete) {
                    if skilltree.editor_request_delete_selected() {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                }
                if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                    if apply_editor_tool_at_world(skilltree, skilltree.editor.cursor_world) {
                        commands.push(HeadfulInputCommand::PlayClick);
                    }
                }
                if pressed(VirtualKeyCode::S) {
                    match skilltree.save_def() {
                        Ok(()) => {
                            skilltree.editor.dirty = false;
                            skilltree.editor.status = Some("SAVED".to_string());
                        }
                        Err(e) => {
                            skilltree.editor.status = Some(format!("SAVE FAILED: {e}"));
                        }
                    }
                }
                if pressed(VirtualKeyCode::R) {
                    skilltree.reload_def();
                    skilltree.editor.dirty = false;
                    if skilltree.load_warning_message().is_none() {
                        skilltree.editor.status = Some("RELOADED".to_string());
                    }
                }
            } else if pressed(VirtualKeyCode::Escape) {
                let transition = headful_view::back(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                horizontal_repeat.clear();
                commands.push(HeadfulInputCommand::PlayClick);
            } else if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                let transition = headful_view::start_game(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                if transition.reset_tetris {
                    commands.push(HeadfulInputCommand::ResetRun);
                }
                commands.push(HeadfulInputCommand::PlayClick);
            }
        }
        GameView::GameOver => {
            if pressed(VirtualKeyCode::Escape) {
                let transition = headful_view::back(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                horizontal_repeat.clear();
                commands.push(HeadfulInputCommand::PlayClick);
            } else if pressed(VirtualKeyCode::Return) || pressed(VirtualKeyCode::Space) {
                let transition = headful_view::start_game(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                if transition.reset_tetris {
                    commands.push(HeadfulInputCommand::ResetRun);
                }
                commands.push(HeadfulInputCommand::PlayClick);
            } else if pressed(VirtualKeyCode::K) {
                let transition = headful_view::open_skilltree(view);
                view = transition.next_view;
                runner.state_mut().view = view;
                horizontal_repeat.clear();
                commands.push(HeadfulInputCommand::PlayClick);
            }
        }
        GameView::Tetris { paused } => {
            if pressed(VirtualKeyCode::Escape) {
                let transition = headful_view::toggle_pause(view);
                view = transition.next_view;
                {
                    let state = runner.state_mut();
                    state.view = view;
                    state.gravity_elapsed = Duration::ZERO;
                }
                horizontal_repeat.clear();
                commands.push(HeadfulInputCommand::PlayClick);
                return commands;
            }

            if paused {
                return commands;
            }

            sync_horizontal_repeat_from_frame(input, horizontal_repeat, now, |action| {
                commands.push(HeadfulInputCommand::ApplyAction(action));
            });
            let soft_drop_down = input.keys_down.contains(&VirtualKeyCode::Down)
                || input.keys_down.contains(&VirtualKeyCode::S);
            if soft_drop_down {
                commands.push(HeadfulInputCommand::ApplyAction(InputAction::SoftDrop));
            }
            for key in [
                VirtualKeyCode::Up,
                VirtualKeyCode::W,
                VirtualKeyCode::Z,
                VirtualKeyCode::X,
                VirtualKeyCode::A,
                VirtualKeyCode::Space,
                VirtualKeyCode::C,
            ] {
                if pressed(key) {
                    if let Some(action) = map_key_to_action(key) {
                        commands.push(HeadfulInputCommand::ApplyAction(action));
                    }
                }
            }
        }
    }

    commands
}

pub fn handle_ui_tree_click_action(
    runner: &mut HeadlessRunner<TetrisLogic>,
    action: UiAction,
) -> UiActionResult {
    let mut result = UiActionResult::default();
    match action {
        ACTION_MAIN_MENU_START => {
            let view = runner.state().view;
            if matches!(view, GameView::MainMenu) {
                let transition = headful_view::start_game(view);
                runner.state_mut().view = transition.next_view;
                if transition.reset_tetris {
                    result.commands.push(HeadfulInputCommand::ResetRun);
                }
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_MAIN_MENU_SKILLTREE_EDITOR => {
            let view = runner.state().view;
            if matches!(view, GameView::MainMenu) {
                let transition = headful_view::open_skilltree_editor(view);
                runner.state_mut().view = transition.next_view;
                let skilltree = &mut runner.state_mut().skilltree;
                if !skilltree.editor.enabled {
                    skilltree.editor_toggle();
                }
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_MAIN_MENU_QUIT => {
            if matches!(runner.state().view, GameView::MainMenu) {
                result.commands.push(HeadfulInputCommand::ExitRequested);
                result.handled = true;
            }
        }
        ACTION_PAUSE_RESUME => {
            let view = runner.state().view;
            if matches!(view, GameView::Tetris { paused: true }) {
                let transition = headful_view::toggle_pause(view);
                let state = runner.state_mut();
                state.view = transition.next_view;
                state.gravity_elapsed = Duration::ZERO;
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_PAUSE_END_RUN => {
            let view = runner.state().view;
            if matches!(view, GameView::Tetris { paused: true }) {
                let transition = headful_view::game_over(view);
                runner.state_mut().view = transition.next_view;
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_GAME_OVER_RESTART => {
            let view = runner.state().view;
            if matches!(view, GameView::GameOver) {
                let transition = headful_view::start_game(view);
                runner.state_mut().view = transition.next_view;
                if transition.reset_tetris {
                    result.commands.push(HeadfulInputCommand::ResetRun);
                }
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_GAME_OVER_SKILLTREE => {
            let view = runner.state().view;
            if matches!(view, GameView::GameOver) {
                let transition = headful_view::open_skilltree(view);
                runner.state_mut().view = transition.next_view;
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_GAME_OVER_QUIT => {
            if matches!(runner.state().view, GameView::GameOver) {
                result.commands.push(HeadfulInputCommand::ExitRequested);
                result.handled = true;
            }
        }
        ACTION_SKILLTREE_START_RUN => {
            let view = runner.state().view;
            let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
            if matches!(view, GameView::SkillTree) && !skilltree_editor_enabled {
                let transition = headful_view::start_game(view);
                runner.state_mut().view = transition.next_view;
                if transition.reset_tetris {
                    result.commands.push(HeadfulInputCommand::ResetRun);
                }
                result.commands.push(HeadfulInputCommand::PlayClick);
                result.handled = true;
            }
        }
        ACTION_SKILLTREE_TOOL_SELECT
        | ACTION_SKILLTREE_TOOL_MOVE
        | ACTION_SKILLTREE_TOOL_ADD_CELL
        | ACTION_SKILLTREE_TOOL_REMOVE_CELL
        | ACTION_SKILLTREE_TOOL_LINK => {}
        _ => {}
    }
    result
}

pub fn handle_skilltree_world_click(
    runner: &mut HeadlessRunner<TetrisLogic>,
    last_skilltree: SkillTreeLayout,
    mouse_x: u32,
    mouse_y: u32,
    mouse_release_was_drag: bool,
) -> Vec<HeadfulInputCommand> {
    let mut commands = Vec::new();
    let view = runner.state().view;
    if !matches!(view, GameView::SkillTree) || mouse_release_was_drag {
        return commands;
    }

    let skilltree_editor_enabled = runner.state().skilltree.editor.enabled;
    if skilltree_editor_enabled {
        if let Some(world) = headful_camera::skilltree_world_cell_at_screen(
            &runner.state().skilltree,
            last_skilltree,
            mouse_x,
            mouse_y,
        ) {
            let skilltree = &mut runner.state_mut().skilltree;
            if apply_editor_tool_at_world(skilltree, world) {
                commands.push(HeadfulInputCommand::PlayClick);
            }
        }
        return commands;
    }

    if let Some(world) = headful_camera::skilltree_world_cell_at_screen(
        &runner.state().skilltree,
        last_skilltree,
        mouse_x,
        mouse_y,
    ) {
        let hit_id = runner.state().skilltree.def.nodes.iter().find_map(|node| {
            node.shape.iter().find_map(|rel| {
                let wx = node.pos.x + rel.x;
                let wy = node.pos.y + rel.y;
                if wx == world.x && wy == world.y {
                    Some(node.id.clone())
                } else {
                    None
                }
            })
        });
        if let Some(id) = hit_id {
            let transition = headful_view::open_skilltree(runner.state().view);
            let state = runner.state_mut();
            state.view = transition.next_view;
            state.skilltree.try_buy(&id);
            commands.push(HeadfulInputCommand::PlayClick);
        }
    }

    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skilltree::SkillTreeRuntime;
    use crate::tetris_core::Piece;

    fn input_frame_for_keys(
        pressed: &[VirtualKeyCode],
        down: &[VirtualKeyCode],
        released: &[VirtualKeyCode],
    ) -> InputFrame {
        let mut input = InputFrame::default();
        for &key in down {
            input.keys_down.insert(key);
        }
        for &key in pressed {
            input.keys_pressed.insert(key);
        }
        for &key in released {
            input.keys_released.insert(key);
        }
        input
    }

    fn make_runner(view: GameView) -> HeadlessRunner<TetrisLogic> {
        let mut runner =
            HeadlessRunner::new(TetrisLogic::new(0, Piece::all()).with_bottomwell(true));
        let state = runner.state_mut();
        state.view = view;
        state.skilltree = SkillTreeRuntime::load_default();
        runner
    }

    fn editor_layout() -> SkillTreeLayout {
        SkillTreeLayout {
            grid_cell: 20,
            grid_cols: 20,
            grid_rows: 12,
            ..SkillTreeLayout::default()
        }
    }

    #[test]
    fn map_key_a_to_rotate_180() {
        assert_eq!(
            map_key_to_action(VirtualKeyCode::A),
            Some(InputAction::Rotate180)
        );
    }

    #[test]
    fn hard_drop_is_the_only_gameplay_sfx_trigger() {
        for action in [
            InputAction::Noop,
            InputAction::MoveLeft,
            InputAction::MoveRight,
            InputAction::SoftDrop,
            InputAction::RotateCw,
            InputAction::RotateCcw,
            InputAction::Rotate180,
            InputAction::Hold,
        ] {
            assert!(!should_play_action_sfx(action));
        }
        assert!(should_play_action_sfx(InputAction::HardDrop));
    }

    #[test]
    fn sync_horizontal_repeat_consumes_frame_sets() {
        let mut repeat = HorizontalRepeat::default();
        let now = Instant::now();
        let mut immediate = Vec::new();

        sync_horizontal_repeat_from_frame(
            &input_frame_for_keys(&[VirtualKeyCode::Left], &[VirtualKeyCode::Left], &[]),
            &mut repeat,
            now,
            |action| immediate.push(action),
        );
        assert_eq!(immediate, vec![InputAction::MoveLeft]);
        assert_eq!(repeat.active, Some(HorizontalDir::Left));

        sync_horizontal_repeat_from_frame(
            &input_frame_for_keys(&[], &[], &[VirtualKeyCode::Left]),
            &mut repeat,
            now + Duration::from_millis(10),
            |_| {},
        );
        assert!(!repeat.left_down);
        assert_eq!(repeat.active, None);
    }

    #[test]
    fn keyboard_start_game_emits_reset_and_click_commands() {
        let mut runner = make_runner(GameView::MainMenu);
        let mut repeat = HorizontalRepeat::default();
        let commands = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Return], &[VirtualKeyCode::Return], &[]),
            Instant::now(),
            &mut repeat,
            SkillTreeLayout::default(),
            0,
            0,
        );

        assert_eq!(
            commands,
            vec![
                HeadfulInputCommand::ResetRun,
                HeadfulInputCommand::PlayClick
            ]
        );
        assert!(matches!(
            runner.state().view,
            GameView::Tetris { paused: false }
        ));
    }

    #[test]
    fn keyboard_soft_drop_repeats_while_down_key_is_held() {
        let mut runner = make_runner(GameView::Tetris { paused: false });
        let mut repeat = HorizontalRepeat::default();
        let now = Instant::now();

        let first = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Down], &[VirtualKeyCode::Down], &[]),
            now,
            &mut repeat,
            SkillTreeLayout::default(),
            0,
            0,
        );
        let held = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[], &[VirtualKeyCode::Down], &[]),
            now + Duration::from_millis(16),
            &mut repeat,
            SkillTreeLayout::default(),
            0,
            0,
        );

        assert_eq!(
            first,
            vec![HeadfulInputCommand::ApplyAction(InputAction::SoftDrop)]
        );
        assert_eq!(
            held,
            vec![HeadfulInputCommand::ApplyAction(InputAction::SoftDrop)]
        );
    }

    #[test]
    fn skilltree_start_run_ui_action_requires_editor_disabled() {
        let mut runner = make_runner(GameView::SkillTree);
        runner.state_mut().skilltree.editor.enabled = false;
        let result = handle_ui_tree_click_action(&mut runner, ACTION_SKILLTREE_START_RUN);
        assert!(result.handled);
        assert_eq!(
            result.commands,
            vec![
                HeadfulInputCommand::ResetRun,
                HeadfulInputCommand::PlayClick
            ]
        );

        runner.state_mut().view = GameView::SkillTree;
        runner.state_mut().skilltree.editor.enabled = true;
        let blocked = handle_ui_tree_click_action(&mut runner, ACTION_SKILLTREE_START_RUN);
        assert!(!blocked.handled);
        assert!(blocked.commands.is_empty());
    }

    #[test]
    fn skilltree_editor_direct_tool_hotkeys_select_expected_tool() {
        let mut runner = make_runner(GameView::SkillTree);
        runner.state_mut().skilltree.editor.enabled = true;
        let mut repeat = HorizontalRepeat::default();

        let commands = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Key3], &[VirtualKeyCode::Key3], &[]),
            Instant::now(),
            &mut repeat,
            editor_layout(),
            0,
            0,
        );

        assert_eq!(
            runner.state().skilltree.editor.tool,
            SkillTreeEditorTool::AddCell
        );
        assert!(commands.contains(&HeadfulInputCommand::PlayClick));
    }

    #[test]
    fn skilltree_editor_delete_key_uses_two_step_guardrail() {
        let mut runner = make_runner(GameView::SkillTree);
        {
            let skilltree = &mut runner.state_mut().skilltree;
            skilltree.editor.enabled = true;
            let _ = skilltree.editor_create_node_at(Vec2i::new(5, 5));
        }
        let target_id = runner
            .state()
            .skilltree
            .editor
            .selected
            .clone()
            .expect("new node should be selected");
        let mut repeat = HorizontalRepeat::default();

        let first = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Delete], &[VirtualKeyCode::Delete], &[]),
            Instant::now(),
            &mut repeat,
            editor_layout(),
            0,
            0,
        );
        assert!(first.is_empty());
        assert!(runner.state().skilltree.node_index(&target_id).is_some());

        let second = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Delete], &[VirtualKeyCode::Delete], &[]),
            Instant::now() + Duration::from_millis(1),
            &mut repeat,
            editor_layout(),
            0,
            0,
        );
        assert!(second.contains(&HeadfulInputCommand::PlayClick));
        assert!(runner.state().skilltree.node_index(&target_id).is_none());
    }

    #[test]
    fn skilltree_editor_keyboard_cursor_apply_adds_cell_without_mouse() {
        let mut runner = make_runner(GameView::SkillTree);
        {
            let skilltree = &mut runner.state_mut().skilltree;
            skilltree.editor.enabled = true;
            skilltree.editor_set_tool(SkillTreeEditorTool::AddCell);
            skilltree.editor_select("start", None);
            skilltree.editor_set_cursor_world(Vec2i::new(1, 0));
        }
        let mut repeat = HorizontalRepeat::default();

        let commands = process_keyboard_frame(
            &mut runner,
            &input_frame_for_keys(&[VirtualKeyCode::Return], &[VirtualKeyCode::Return], &[]),
            Instant::now(),
            &mut repeat,
            editor_layout(),
            0,
            0,
        );
        assert!(commands.contains(&HeadfulInputCommand::PlayClick));

        let start_idx = runner
            .state()
            .skilltree
            .node_index("start")
            .expect("start node should exist");
        let start = &runner.state().skilltree.def.nodes[start_idx];
        assert!(start.shape.iter().any(|c| *c == Vec2i::new(1, 0)));
    }
}
