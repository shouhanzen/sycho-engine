use engine::ui_tree::{UiAction, UiId};

pub const UI_CANVAS: UiId = UiId(1);

pub const UI_MAIN_MENU_CONTAINER: UiId = UiId(100);
pub const UI_MAIN_MENU_START: UiId = UiId(101);
pub const UI_MAIN_MENU_SKILLTREE_EDITOR: UiId = UiId(102);
pub const UI_MAIN_MENU_QUIT: UiId = UiId(103);
pub const UI_MAIN_MENU_SETTINGS: UiId = UiId(104);

pub const UI_TETRIS_HUD_CONTAINER: UiId = UiId(200);
pub const UI_TETRIS_PAUSE: UiId = UiId(201);
pub const UI_TETRIS_HOLD: UiId = UiId(202);

pub const UI_PAUSE_MENU_CONTAINER: UiId = UiId(300);
pub const UI_PAUSE_RESUME: UiId = UiId(301);
pub const UI_PAUSE_END_RUN: UiId = UiId(302);
pub const UI_PAUSE_SETTINGS: UiId = UiId(303);

pub const UI_GAME_OVER_CONTAINER: UiId = UiId(400);
pub const UI_GAME_OVER_RESTART: UiId = UiId(401);
pub const UI_GAME_OVER_SKILLTREE: UiId = UiId(402);
pub const UI_GAME_OVER_QUIT: UiId = UiId(403);

pub const UI_SKILLTREE_CONTAINER: UiId = UiId(500);
pub const UI_SKILLTREE_TOOLBAR: UiId = UiId(510);
pub const UI_SKILLTREE_TOOL_SELECT: UiId = UiId(511);
pub const UI_SKILLTREE_TOOL_MOVE: UiId = UiId(512);
pub const UI_SKILLTREE_TOOL_ADD_CELL: UiId = UiId(513);
pub const UI_SKILLTREE_TOOL_REMOVE_CELL: UiId = UiId(514);
pub const UI_SKILLTREE_TOOL_LINK: UiId = UiId(515);
pub const UI_SKILLTREE_START_RUN: UiId = UiId(516);

pub const UI_SETTINGS_MENU_CONTAINER: UiId = UiId(600);
pub const UI_SETTINGS_BACK: UiId = UiId(601);
pub const UI_SETTINGS_RESET: UiId = UiId(602);
pub const UI_SETTINGS_TOGGLE_MUTE: UiId = UiId(603);
pub const UI_SETTINGS_TOGGLE_MUSIC: UiId = UiId(604);
pub const UI_SETTINGS_TOGGLE_TIMER: UiId = UiId(605);
pub const UI_SETTINGS_TOGGLE_AUTO_PAUSE: UiId = UiId(606);
pub const UI_SETTINGS_TOGGLE_HIGH_CONTRAST: UiId = UiId(607);
pub const UI_SETTINGS_TOGGLE_REDUCE_MOTION: UiId = UiId(608);

pub const ACTION_MAIN_MENU_START: UiAction = UiAction(1);
pub const ACTION_MAIN_MENU_SKILLTREE_EDITOR: UiAction = UiAction(2);
pub const ACTION_MAIN_MENU_QUIT: UiAction = UiAction(3);
pub const ACTION_TETRIS_TOGGLE_PAUSE: UiAction = UiAction(4);
pub const ACTION_TETRIS_HOLD: UiAction = UiAction(5);
pub const ACTION_PAUSE_RESUME: UiAction = UiAction(6);
pub const ACTION_PAUSE_END_RUN: UiAction = UiAction(7);
pub const ACTION_GAME_OVER_RESTART: UiAction = UiAction(8);
pub const ACTION_GAME_OVER_SKILLTREE: UiAction = UiAction(9);
pub const ACTION_GAME_OVER_QUIT: UiAction = UiAction(10);
pub const ACTION_SKILLTREE_START_RUN: UiAction = UiAction(11);
pub const ACTION_SKILLTREE_TOOL_SELECT: UiAction = UiAction(12);
pub const ACTION_SKILLTREE_TOOL_MOVE: UiAction = UiAction(13);
pub const ACTION_SKILLTREE_TOOL_ADD_CELL: UiAction = UiAction(14);
pub const ACTION_SKILLTREE_TOOL_REMOVE_CELL: UiAction = UiAction(15);
pub const ACTION_SKILLTREE_TOOL_LINK: UiAction = UiAction(16);
