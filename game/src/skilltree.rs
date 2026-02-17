use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::tetris_core::Vec2i;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Vec2f {
    pub x: f32,
    pub y: f32,
}

impl Vec2f {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Static, designer-authored skilltree definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillTreeDef {
    pub version: u32,
    pub nodes: Vec<SkillNodeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillNodeDef {
    pub id: String,
    pub name: String,

    /// Anchor position on the global skilltree grid (world cells; y increases upward).
    pub pos: Vec2i,

    /// Polyblock cells relative to `pos` (disconnected shapes are allowed).
    pub shape: Vec<Vec2i>,

    /// Optional visual identifier; by default we map 0..=7 via `engine::render::color_for_cell`.
    pub color: u8,

    /// Purchase cost in meta-currency ("money").
    pub cost: u32,

    /// Node ids that must be unlocked before this node can be purchased.
    pub requires: Vec<String>,

    pub effect: SkillEffect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SkillEffect {
    None,
    AddRoundTimeSeconds { seconds: u32 },
    AddScoreBonus { bonus: u32 },
    FasterGravity { percent: u32 },
    AddDeepShaftRows { rows: u32 },
    AddOreWeight { points: u32 },
    AddCoinWeight { points: u32 },
    AddOreScoreValue { points: u32 },
    AddCoinScoreValue { points: u32 },
    AddOreMoneyValue { points: u32 },
    AddCoinMoneyValue { points: u32 },
    AddHolePatchChanceBp {
        #[serde(alias = "basisPoints")]
        basis_points: u32,
    },
    AddHoleAlignChanceBp {
        #[serde(alias = "basisPoints")]
        basis_points: u32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillTreeRunMods {
    pub extra_round_time_seconds: u32,
    pub gravity_faster_percent: u32,
    pub score_bonus_per_line: u32,
    pub deep_shaft_rows: u32,
    pub ore_weight_points: u32,
    pub coin_weight_points: u32,
    pub ore_score_bonus: u32,
    pub coin_score_bonus: u32,
    pub ore_money_bonus: u32,
    pub coin_money_bonus: u32,
    pub hole_patch_chance_bp: u32,
    pub hole_align_chance_bp: u32,
}

const MAX_DEEP_SHAFT_ROWS: u32 = 3;
const MAX_ORE_WEIGHT_POINTS: u32 = 6;
const MAX_COIN_WEIGHT_POINTS: u32 = 3;
const MAX_ORE_SCORE_BONUS: u32 = 50;
const MAX_COIN_SCORE_BONUS: u32 = 120;
const MAX_ORE_MONEY_BONUS: u32 = 2;
const MAX_COIN_MONEY_BONUS: u32 = 6;
const MAX_HOLE_CHANCE_BP: u32 = 10_000;
const SKILLTREE_LOAD_ERROR_PREFIX: &str = "SKILLTREE LOAD ERROR:";

impl Default for SkillTreeDef {
    fn default() -> Self {
        // Keep a compile-time fallback so the game still runs even if the asset file is missing.
        serde_json::from_str(include_str!("../assets/skilltree.json")).unwrap_or_else(|_| {
            SkillTreeDef {
                version: 1,
                nodes: vec![SkillNodeDef {
                    id: "start".to_string(),
                    name: "START".to_string(),
                    pos: Vec2i::new(0, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 3,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                }],
            }
        })
    }
}

/// Dynamic player progress for the skilltree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillTreeProgress {
    pub version: u32,
    pub money: u32,
    pub unlocked: Vec<String>,
}

impl Default for SkillTreeProgress {
    fn default() -> Self {
        Self {
            version: 1,
            money: 0,
            unlocked: vec!["start".to_string()],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Unlocked,
    Available,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillTreeEditorTool {
    Select,
    Move,
    AddCell,
    RemoveCell,
    ConnectPrereqs,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillTreeEditorState {
    pub enabled: bool,
    pub tool: SkillTreeEditorTool,
    pub selected: Option<String>,

    /// For `SelectMove`: the clicked cell's offset from the node's `pos` (world coords).
    pub move_grab_offset: Option<Vec2i>,

    /// For `ConnectPrereqs`: the prerequisite node id ("from").
    pub connect_from: Option<String>,

    /// Compact help overlay expansion state (`?` toggles while editor is enabled).
    #[serde(default)]
    pub help_expanded: bool,

    /// Keyboard world cursor location (used by cursor-first editor commands).
    #[serde(default = "default_editor_cursor_world")]
    pub cursor_world: Vec2i,

    /// Two-step delete guardrail: selected id waiting for a second delete confirmation.
    #[serde(default)]
    pub pending_delete_id: Option<String>,

    /// Search panel visibility/query for keyboard-first jump-to-node.
    #[serde(default)]
    pub search_open: bool,
    #[serde(default)]
    pub search_query: String,

    pub dirty: bool,
    pub status: Option<String>,
}

impl Default for SkillTreeEditorState {
    fn default() -> Self {
        Self {
            enabled: false,
            tool: SkillTreeEditorTool::Select,
            selected: None,
            move_grab_offset: None,
            connect_from: None,
            help_expanded: false,
            cursor_world: Vec2i::new(0, 0),
            pending_delete_id: None,
            search_open: false,
            search_query: String::new(),
            dirty: false,
            status: None,
        }
    }
}

fn default_editor_cursor_world() -> Vec2i {
    Vec2i::new(0, 0)
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SkillTreeCamera {
    /// Current zoom level in pixels per grid cell (lerps towards `target_cell_px`).
    pub cell_px: f32,
    /// Target zoom level in pixels per grid cell.
    pub target_cell_px: f32,
    /// Current pan offset in world grid cells (lerps towards `target_pan`).
    pub pan: Vec2f,
    /// Target pan offset in world grid cells.
    pub target_pan: Vec2f,
}

impl Default for SkillTreeCamera {
    fn default() -> Self {
        Self {
            cell_px: 20.0,
            target_cell_px: 20.0,
            pan: Vec2f::new(0.0, 0.0),
            target_pan: Vec2f::new(0.0, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillTreeWorldBounds {
    /// Inclusive cell coordinates.
    pub min: Vec2i,
    /// Inclusive cell coordinates.
    pub max: Vec2i,
}

pub fn skilltree_world_bounds(def: &SkillTreeDef) -> Option<SkillTreeWorldBounds> {
    let mut any = false;
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    for node in &def.nodes {
        for rel in &node.shape {
            any = true;
            let x = node.pos.x.saturating_add(rel.x);
            let y = node.pos.y.saturating_add(rel.y);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    any.then(|| SkillTreeWorldBounds {
        min: Vec2i::new(min_x, min_y),
        max: Vec2i::new(max_x, max_y),
    })
}

pub fn clamp_camera_min_to_bounds(
    cam_min: Vec2f,
    view_size_cells: Vec2f,
    bounds: SkillTreeWorldBounds,
    pad_cells: f32,
) -> Vec2f {
    let view_w = view_size_cells.x.max(0.0);
    let view_h = view_size_cells.y.max(0.0);

    let left = bounds.min.x as f32 - pad_cells;
    let right = (bounds.max.x.saturating_add(1)) as f32 + pad_cells;
    let bottom = bounds.min.y as f32 - pad_cells;
    let top = (bounds.max.y.saturating_add(1)) as f32 + pad_cells;

    let min_x = left;
    let max_x = right - view_w;
    let min_y = bottom;
    let max_y = top - view_h;

    let x = if min_x <= max_x {
        cam_min.x.clamp(min_x, max_x)
    } else {
        // Viewport is wider than the bounds; center it.
        (min_x + max_x) / 2.0
    };

    let y = if min_y <= max_y {
        cam_min.y.clamp(min_y, max_y)
    } else {
        // Viewport is taller than the bounds; center it.
        (min_y + max_y) / 2.0
    };

    Vec2f::new(x, y)
}

/// Runtime helper: definition + player progress + caches for fast queries/hit-testing.
#[derive(Debug, Clone)]
pub struct SkillTreeRuntime {
    pub def: SkillTreeDef,
    pub progress: SkillTreeProgress,

    pub def_path: Option<PathBuf>,
    pub progress_path: PathBuf,

    pub camera: SkillTreeCamera,
    pub editor: SkillTreeEditorState,

    editor_history: SkillTreeEditorHistory,

    // Cached indices (rebuilt on load/edit).
    id_to_index: HashMap<String, usize>,
    unlocked_set: HashSet<String>,
}

const SKILLTREE_EDITOR_HISTORY_LIMIT: usize = 128;

#[derive(Debug, Clone, Default)]
struct SkillTreeEditorHistory {
    undo_defs: Vec<SkillTreeDef>,
    redo_defs: Vec<SkillTreeDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillTreeSnapshot {
    pub def: SkillTreeDef,
    pub progress: SkillTreeProgress,
    pub camera: SkillTreeCamera,
    pub editor: SkillTreeEditorState,
}

impl SkillTreeRuntime {
    pub fn from_defaults() -> Self {
        let mut rt = Self {
            def: SkillTreeDef::default(),
            progress: SkillTreeProgress::default(),
            def_path: None,
            progress_path: default_progress_path(),
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
            editor_history: SkillTreeEditorHistory::default(),
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };
        rt.rebuild_caches();
        rt
    }

    pub fn to_snapshot(&self) -> SkillTreeSnapshot {
        SkillTreeSnapshot {
            def: self.def.clone(),
            progress: self.progress.clone(),
            camera: self.camera,
            editor: self.editor.clone(),
        }
    }

    pub fn from_snapshot(snapshot: SkillTreeSnapshot) -> Self {
        let mut rt = Self {
            def: snapshot.def,
            progress: snapshot.progress,
            def_path: None,
            progress_path: default_progress_path(),
            camera: snapshot.camera,
            editor: snapshot.editor,
            editor_history: SkillTreeEditorHistory::default(),
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };
        rt.rebuild_caches();
        rt
    }

    pub fn load_default() -> Self {
        let (def, def_path, load_warning) = load_def_from_default_path();
        let progress_path = default_progress_path();
        let progress = load_progress(&progress_path).unwrap_or_default();

        let mut rt = Self {
            def,
            progress,
            def_path,
            progress_path,
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
            editor_history: SkillTreeEditorHistory::default(),
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };
        rt.rebuild_caches();
        rt.set_load_warning_status(load_warning);
        rt
    }

    pub fn load_warning_message(&self) -> Option<&str> {
        let status = self.editor.status.as_deref()?;
        status
            .strip_prefix(SKILLTREE_LOAD_ERROR_PREFIX)
            .map(str::trim_start)
    }

    pub fn rebuild_caches(&mut self) {
        normalize_and_validate(&mut self.def);
        if self.progress.version == 0 {
            self.progress.version = 1;
        }

        // Prune unlocked ids that no longer exist (e.g. after editing/deleting nodes).
        let ids: HashSet<String> = self.def.nodes.iter().map(|n| n.id.clone()).collect();
        self.progress.unlocked.retain(|id| ids.contains(id));
        if ids.contains("start") && !self.progress.unlocked.iter().any(|id| id == "start") {
            self.progress.unlocked.push("start".to_string());
        }

        self.unlocked_set = self.progress.unlocked.iter().cloned().collect();
        self.id_to_index = self
            .def
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        self.reconcile_editor_references();
    }

    pub fn money(&self) -> u32 {
        self.progress.money
    }

    pub fn is_unlocked(&self, id: &str) -> bool {
        self.unlocked_set.contains(id)
    }

    pub fn node_state(&self, node: &SkillNodeDef) -> NodeState {
        if self.is_unlocked(&node.id) {
            return NodeState::Unlocked;
        }
        if node.requires.iter().all(|r| self.is_unlocked(r)) {
            NodeState::Available
        } else {
            NodeState::Locked
        }
    }

    pub fn run_mods(&self) -> SkillTreeRunMods {
        let mut mods = SkillTreeRunMods::default();
        for node in &self.def.nodes {
            if !self.is_unlocked(&node.id) {
                continue;
            }
            match node.effect {
                SkillEffect::None => {}
                SkillEffect::AddRoundTimeSeconds { seconds } => {
                    mods.extra_round_time_seconds =
                        mods.extra_round_time_seconds.saturating_add(seconds);
                }
                SkillEffect::AddScoreBonus { bonus } => {
                    mods.score_bonus_per_line = mods.score_bonus_per_line.saturating_add(bonus);
                }
                SkillEffect::FasterGravity { percent } => {
                    mods.gravity_faster_percent =
                        mods.gravity_faster_percent.saturating_add(percent);
                }
                SkillEffect::AddDeepShaftRows { rows } => {
                    mods.deep_shaft_rows = mods
                        .deep_shaft_rows
                        .saturating_add(rows)
                        .min(MAX_DEEP_SHAFT_ROWS);
                }
                SkillEffect::AddOreWeight { points } => {
                    mods.ore_weight_points = mods
                        .ore_weight_points
                        .saturating_add(points)
                        .min(MAX_ORE_WEIGHT_POINTS);
                }
                SkillEffect::AddCoinWeight { points } => {
                    mods.coin_weight_points = mods
                        .coin_weight_points
                        .saturating_add(points)
                        .min(MAX_COIN_WEIGHT_POINTS);
                }
                SkillEffect::AddOreScoreValue { points } => {
                    mods.ore_score_bonus = mods
                        .ore_score_bonus
                        .saturating_add(points)
                        .min(MAX_ORE_SCORE_BONUS);
                }
                SkillEffect::AddCoinScoreValue { points } => {
                    mods.coin_score_bonus = mods
                        .coin_score_bonus
                        .saturating_add(points)
                        .min(MAX_COIN_SCORE_BONUS);
                }
                SkillEffect::AddOreMoneyValue { points } => {
                    mods.ore_money_bonus = mods
                        .ore_money_bonus
                        .saturating_add(points)
                        .min(MAX_ORE_MONEY_BONUS);
                }
                SkillEffect::AddCoinMoneyValue { points } => {
                    mods.coin_money_bonus = mods
                        .coin_money_bonus
                        .saturating_add(points)
                        .min(MAX_COIN_MONEY_BONUS);
                }
                SkillEffect::AddHolePatchChanceBp { basis_points } => {
                    mods.hole_patch_chance_bp = mods
                        .hole_patch_chance_bp
                        .saturating_add(basis_points)
                        .min(MAX_HOLE_CHANCE_BP);
                }
                SkillEffect::AddHoleAlignChanceBp { basis_points } => {
                    mods.hole_align_chance_bp = mods
                        .hole_align_chance_bp
                        .saturating_add(basis_points)
                        .min(MAX_HOLE_CHANCE_BP);
                }
            }
        }
        mods
    }

    pub fn can_buy(&self, node: &SkillNodeDef) -> bool {
        matches!(self.node_state(node), NodeState::Available) && self.progress.money >= node.cost
    }

    pub fn try_buy(&mut self, id: &str) -> bool {
        let Some(idx) = self.id_to_index.get(id).copied() else {
            return false;
        };
        let node = &self.def.nodes[idx];
        if !self.can_buy(node) {
            return false;
        }

        self.progress.money = self.progress.money.saturating_sub(node.cost);
        if !self.unlocked_set.contains(id) {
            self.unlocked_set.insert(id.to_string());
            self.progress.unlocked.push(id.to_string());
        }
        let _ = save_progress(&self.progress_path, &self.progress);
        true
    }

    pub fn add_money(&mut self, amount: u32) {
        self.progress.money = self.progress.money.saturating_add(amount);
        let _ = save_progress(&self.progress_path, &self.progress);
    }

    pub fn save_def(&self) -> std::io::Result<()> {
        let Some(path) = self.def_path.as_ref() else {
            return Ok(());
        };
        let json = serde_json::to_string_pretty(&self.def).unwrap_or_else(|_| "{}".to_string());
        atomic_write(path, json.as_bytes())
    }

    pub fn reload_def(&mut self) {
        let (def, def_path, load_warning) = load_def_from_default_path();
        self.def = def;
        self.def_path = def_path;
        self.editor_history = SkillTreeEditorHistory::default();
        self.editor.pending_delete_id = None;
        self.editor.search_open = false;
        self.editor.search_query.clear();
        self.rebuild_caches();
        self.set_load_warning_status(load_warning);
    }

    fn set_load_warning_status(&mut self, warning: Option<String>) {
        if let Some(warning) = warning {
            self.editor.status = Some(format!("{SKILLTREE_LOAD_ERROR_PREFIX} {warning}"));
            return;
        }
        if self
            .editor
            .status
            .as_deref()
            .is_some_and(|status| status.starts_with(SKILLTREE_LOAD_ERROR_PREFIX))
        {
            self.editor.status = None;
        }
    }

    fn push_history_entry(history: &mut Vec<SkillTreeDef>, snapshot: SkillTreeDef) {
        history.push(snapshot);
        if history.len() > SKILLTREE_EDITOR_HISTORY_LIMIT {
            let overflow = history.len() - SKILLTREE_EDITOR_HISTORY_LIMIT;
            history.drain(0..overflow);
        }
    }

    fn reconcile_editor_references(&mut self) {
        if let Some(selected) = self.editor.selected.as_deref() {
            if !self.id_to_index.contains_key(selected) {
                self.editor.selected = None;
                self.editor.move_grab_offset = None;
            }
        }
        if let Some(connect_from) = self.editor.connect_from.as_deref() {
            if !self.id_to_index.contains_key(connect_from) {
                self.editor.connect_from = None;
            }
        }
        if self.editor.pending_delete_id.as_deref() != self.editor.selected.as_deref() {
            self.editor.pending_delete_id = None;
        }
    }

    fn mark_editor_mutation(&mut self, before: SkillTreeDef, status: String) {
        Self::push_history_entry(&mut self.editor_history.undo_defs, before);
        self.editor_history.redo_defs.clear();
        self.editor.pending_delete_id = None;
        self.editor.dirty = true;
        self.editor.status = Some(status);
    }

    fn next_duplicate_id(&self, base_id: &str) -> String {
        let first = format!("{base_id}_copy");
        if !self.id_to_index.contains_key(&first) {
            return first;
        }
        let mut i = 2usize;
        loop {
            let candidate = format!("{base_id}_copy{i}");
            if !self.id_to_index.contains_key(&candidate) {
                return candidate;
            }
            i = i.saturating_add(1);
        }
    }

    pub fn editor_toggle(&mut self) {
        self.editor.enabled = !self.editor.enabled;
        self.editor.status = Some(if self.editor.enabled {
            "EDITOR ON".to_string()
        } else {
            "EDITOR OFF".to_string()
        });
        if !self.editor.enabled {
            self.editor.connect_from = None;
            self.editor.move_grab_offset = None;
            self.editor.pending_delete_id = None;
            self.editor.search_open = false;
            self.editor.search_query.clear();
        }
    }

    pub fn editor_cycle_tool(&mut self) {
        let next_tool = match self.editor.tool {
            SkillTreeEditorTool::Select => SkillTreeEditorTool::Move,
            SkillTreeEditorTool::Move => SkillTreeEditorTool::AddCell,
            SkillTreeEditorTool::AddCell => SkillTreeEditorTool::RemoveCell,
            SkillTreeEditorTool::RemoveCell => SkillTreeEditorTool::ConnectPrereqs,
            SkillTreeEditorTool::ConnectPrereqs => SkillTreeEditorTool::Select,
        };
        self.editor_set_tool(next_tool);
    }

    pub fn editor_set_tool(&mut self, tool: SkillTreeEditorTool) {
        self.editor.tool = tool;
        if !matches!(tool, SkillTreeEditorTool::ConnectPrereqs) {
            self.editor.connect_from = None;
        }
        self.editor.pending_delete_id = None;
        self.editor.status = Some(format!("TOOL {:?}", self.editor.tool));
    }

    pub fn editor_select(&mut self, id: &str, grab_offset: Option<Vec2i>) {
        self.editor.selected = Some(id.to_string());
        self.editor.move_grab_offset = grab_offset;
        self.editor.pending_delete_id = None;
        if let Some(idx) = self.node_index(id) {
            self.editor.cursor_world = self.def.nodes[idx].pos;
        }
        self.editor.status = Some(format!("SELECT {id}"));
    }

    pub fn editor_selected_id(&self) -> Option<&str> {
        self.editor.selected.as_deref()
    }

    pub fn editor_clear_selection(&mut self) {
        self.editor.selected = None;
        self.editor.move_grab_offset = None;
        self.editor.pending_delete_id = None;
        self.editor.status = Some("SELECT NONE".to_string());
    }

    pub fn editor_set_cursor_world(&mut self, world: Vec2i) {
        self.editor.cursor_world = world;
    }

    pub fn editor_toggle_help_overlay(&mut self) {
        self.editor.help_expanded = !self.editor.help_expanded;
        self.editor.status = Some(if self.editor.help_expanded {
            "HELP EXPANDED".to_string()
        } else {
            "HELP COMPACT".to_string()
        });
    }

    pub fn editor_open_search(&mut self) {
        self.editor.search_open = true;
        self.editor.search_query.clear();
        self.editor.status = Some("SEARCH".to_string());
    }

    pub fn editor_close_search(&mut self) {
        self.editor.search_open = false;
        self.editor.search_query.clear();
        self.editor.status = Some("SEARCH CLOSED".to_string());
    }

    pub fn editor_append_search_char(&mut self, c: char) {
        self.editor.search_query.push(c);
    }

    pub fn editor_pop_search_char(&mut self) {
        let _ = self.editor.search_query.pop();
    }

    pub fn editor_find_first_matching(&self, query: &str) -> Option<String> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return None;
        }
        let mut nodes: Vec<&SkillNodeDef> = self.def.nodes.iter().collect();
        nodes.sort_by(|a, b| a.id.cmp(&b.id));

        for node in &nodes {
            if node.id.to_ascii_lowercase() == q {
                return Some(node.id.clone());
            }
        }
        for node in &nodes {
            if node.name.to_ascii_lowercase() == q {
                return Some(node.id.clone());
            }
        }
        for node in &nodes {
            if node.id.to_ascii_lowercase().starts_with(&q) {
                return Some(node.id.clone());
            }
        }
        for node in &nodes {
            if node.name.to_ascii_lowercase().starts_with(&q) {
                return Some(node.id.clone());
            }
        }
        for node in &nodes {
            if node.id.to_ascii_lowercase().contains(&q) {
                return Some(node.id.clone());
            }
        }
        for node in &nodes {
            if node.name.to_ascii_lowercase().contains(&q) {
                return Some(node.id.clone());
            }
        }
        None
    }

    pub fn editor_select_matching(&mut self, query: &str) -> Option<String> {
        let hit = self.editor_find_first_matching(query)?;
        self.editor_select(&hit, None);
        self.editor.status = Some(format!("JUMP {hit}"));
        Some(hit)
    }

    pub fn editor_can_undo(&self) -> bool {
        !self.editor_history.undo_defs.is_empty()
    }

    pub fn editor_can_redo(&self) -> bool {
        !self.editor_history.redo_defs.is_empty()
    }

    pub fn editor_undo(&mut self) -> bool {
        let Some(prev) = self.editor_history.undo_defs.pop() else {
            return false;
        };
        Self::push_history_entry(&mut self.editor_history.redo_defs, self.def.clone());
        self.def = prev;
        self.rebuild_caches();
        self.editor.pending_delete_id = None;
        self.editor.dirty = true;
        self.editor.status = Some("UNDO".to_string());
        true
    }

    pub fn editor_redo(&mut self) -> bool {
        let Some(next) = self.editor_history.redo_defs.pop() else {
            return false;
        };
        Self::push_history_entry(&mut self.editor_history.undo_defs, self.def.clone());
        self.def = next;
        self.rebuild_caches();
        self.editor.pending_delete_id = None;
        self.editor.dirty = true;
        self.editor.status = Some("REDO".to_string());
        true
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.id_to_index.get(id).copied()
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut SkillNodeDef> {
        let idx = self.node_index(id)?;
        self.def.nodes.get_mut(idx)
    }

    pub fn editor_create_node_at(&mut self, pos: Vec2i) -> String {
        let before = self.def.clone();
        let mut i = self.def.nodes.len().saturating_add(1);
        let mut id = format!("node{i}");
        while self.id_to_index.contains_key(&id) {
            i = i.saturating_add(1);
            id = format!("node{i}");
        }

        self.def.nodes.push(SkillNodeDef {
            id: id.clone(),
            name: "NODE".to_string(),
            pos,
            shape: vec![Vec2i::new(0, 0)],
            color: 7,
            cost: 10,
            requires: vec![],
            effect: SkillEffect::None,
        });
        self.rebuild_caches();
        self.editor.selected = Some(id.clone());
        self.editor.cursor_world = pos;
        self.mark_editor_mutation(before, format!("NEW {id}"));
        id
    }

    pub fn editor_duplicate_selected(&mut self) -> Option<String> {
        let selected_id = self.editor.selected.clone()?;
        let idx = self.node_index(&selected_id)?;
        let before = self.def.clone();

        let mut dup = self.def.nodes[idx].clone();
        let dup_id = self.next_duplicate_id(&selected_id);
        dup.id = dup_id.clone();
        dup.pos = Vec2i::new(dup.pos.x.saturating_add(1), dup.pos.y);
        self.def.nodes.push(dup);
        self.rebuild_caches();
        self.editor.selected = Some(dup_id.clone());
        if let Some(new_idx) = self.node_index(&dup_id) {
            self.editor.cursor_world = self.def.nodes[new_idx].pos;
        }
        self.mark_editor_mutation(before, format!("DUP {selected_id} -> {dup_id}"));
        Some(dup_id)
    }

    pub fn editor_request_delete_selected(&mut self) -> bool {
        let Some(selected) = self.editor.selected.clone() else {
            self.editor.pending_delete_id = None;
            return false;
        };
        if self.editor.pending_delete_id.as_deref() != Some(selected.as_str()) {
            self.editor.pending_delete_id = Some(selected.clone());
            self.editor.status = Some(format!("DELETE {selected}? PRESS DEL AGAIN"));
            return false;
        }
        self.editor.pending_delete_id = None;
        self.editor_delete_selected()
    }

    pub fn editor_delete_selected(&mut self) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let Some(idx) = self.node_index(&id) else {
            self.editor.selected = None;
            return false;
        };
        let before = self.def.clone();

        self.def.nodes.remove(idx);
        for n in &mut self.def.nodes {
            n.requires.retain(|r| r != &id);
        }
        self.editor.selected = None;
        self.editor.connect_from = None;
        self.editor.move_grab_offset = None;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("DEL {id}"));
        true
    }

    pub fn editor_nudge_selected_by(&mut self, delta: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let Some(idx) = self.node_index(&id) else {
            return false;
        };
        let pos = self.def.nodes[idx].pos;
        let next = Vec2i::new(pos.x.saturating_add(delta.x), pos.y.saturating_add(delta.y));
        self.editor_move_selected_to(next)
    }

    pub fn editor_move_selected_to(&mut self, new_pos: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let before = self.def.clone();
        let Some(node) = self.node_mut(&id) else {
            return false;
        };
        if node.pos == new_pos {
            return false;
        }
        node.pos = new_pos;
        let _ = node;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("MOVE {id}"));
        true
    }

    pub fn editor_toggle_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let before = self.def.clone();
        let Some(node) = self.node_mut(&id) else {
            return false;
        };

        let rel = Vec2i::new(world.x - node.pos.x, world.y - node.pos.y);
        if let Some(i) = node.shape.iter().position(|c| c.x == rel.x && c.y == rel.y) {
            if node.shape.len() <= 1 {
                // Never allow an empty shape.
                return false;
            }
            node.shape.remove(i);
        } else {
            node.shape.push(rel);
        }
        let _ = node;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("PAINT {id}"));
        true
    }

    pub fn editor_add_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let before = self.def.clone();
        let Some(node) = self.node_mut(&id) else {
            return false;
        };

        let rel = Vec2i::new(world.x - node.pos.x, world.y - node.pos.y);
        if node.shape.iter().any(|c| c.x == rel.x && c.y == rel.y) {
            return false;
        }
        node.shape.push(rel);
        let _ = node;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("ADD {id}"));
        true
    }

    pub fn editor_remove_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let before = self.def.clone();
        let Some(node) = self.node_mut(&id) else {
            return false;
        };

        let rel = Vec2i::new(world.x - node.pos.x, world.y - node.pos.y);
        let Some(i) = node.shape.iter().position(|c| c.x == rel.x && c.y == rel.y) else {
            return false;
        };
        if node.shape.len() <= 1 {
            return false;
        }
        node.shape.remove(i);
        let _ = node;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("REMOVE {id}"));
        true
    }

    pub fn editor_toggle_prereq(&mut self, prereq: &str, node_id: &str) -> bool {
        let before = self.def.clone();
        let Some(node) = self.node_mut(node_id) else {
            return false;
        };
        if prereq == node_id {
            return false;
        }

        if let Some(i) = node.requires.iter().position(|r| r == prereq) {
            node.requires.remove(i);
        } else {
            node.requires.push(prereq.to_string());
        }
        let _ = node;
        self.rebuild_caches();
        self.mark_editor_mutation(before, format!("REQ {prereq} -> {node_id}"));
        true
    }
}

impl Serialize for SkillTreeRuntime {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.to_snapshot().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SkillTreeRuntime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let snapshot = SkillTreeSnapshot::deserialize(deserializer)?;
        Ok(SkillTreeRuntime::from_snapshot(snapshot))
    }
}

fn load_def_from_default_path() -> (SkillTreeDef, Option<PathBuf>, Option<String>) {
    let mut warnings = Vec::new();

    if let Ok(p) = std::env::var("ROLLOUT_SKILLTREE_PATH") {
        let path = PathBuf::from(p);
        match load_def(&path) {
            Ok(def) => return (def, Some(path), None),
            Err(err) => warnings.push(format!(
                "failed to load ROLLOUT_SKILLTREE_PATH {}: {}",
                path.display(),
                err
            )),
        }
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("skilltree.json");
    match load_def(&path) {
        Ok(def) => {
            let warning = if warnings.is_empty() {
                None
            } else {
                Some(warnings.join(" | "))
            };
            return (def, Some(path), warning);
        }
        Err(err) => warnings.push(format!(
            "failed to load default skilltree {}: {}",
            path.display(),
            err
        )),
    }

    let warning = Some(format!(
        "{} | using built-in fallback tree",
        warnings.join(" | ")
    ));
    (SkillTreeDef::default(), None, warning)
}

fn load_def(path: &Path) -> Result<SkillTreeDef, std::io::Error> {
    let bytes = fs::read(path)?;
    let mut def: SkillTreeDef = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    normalize_and_validate(&mut def);
    Ok(def)
}

fn default_progress_path() -> PathBuf {
    if let Ok(p) = std::env::var("ROLLOUT_SKILLTREE_PROGRESS_PATH") {
        return PathBuf::from(p);
    }

    // `CARGO_MANIFEST_DIR` is `.../rollout_engine/game`; the workspace `target/` lives at `..`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("skilltree_progress.json")
}

fn load_progress(path: &Path) -> Result<SkillTreeProgress, std::io::Error> {
    let bytes = fs::read(path)?;
    let p: SkillTreeProgress = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(p)
}

fn save_progress(path: &Path, progress: &SkillTreeProgress) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(progress).unwrap_or_else(|_| "{}".to_string());
    atomic_write(path, json.as_bytes())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Best-effort fallback for Windows rename semantics (rename over an existing file can fail).
            fs::copy(&tmp, path)?;
            let _ = fs::remove_file(&tmp);
            Ok(())
        }
    }
}

fn normalize_and_validate(def: &mut SkillTreeDef) {
    if def.version == 0 {
        def.version = 1;
    }

    // Normalize per-node: remove duplicate cells; shift shape so min(rel) is (0,0) while keeping
    // world cell coverage identical by translating `pos` accordingly.
    for node in &mut def.nodes {
        if node.id.trim().is_empty() {
            node.id = "unnamed".to_string();
        }
        if node.name.trim().is_empty() {
            node.name = node.id.clone();
        }
        if node.shape.is_empty() {
            node.shape.push(Vec2i::new(0, 0));
        }

        // Dedup.
        let mut seen = HashSet::<(i32, i32)>::new();
        node.shape.retain(|c| seen.insert((c.x, c.y)));

        let (min_x, min_y) = node.shape.iter().fold((i32::MAX, i32::MAX), |(mx, my), c| {
            (mx.min(c.x), my.min(c.y))
        });
        if min_x != 0 || min_y != 0 {
            node.pos = Vec2i::new(node.pos.x + min_x, node.pos.y + min_y);
            for c in &mut node.shape {
                c.x -= min_x;
                c.y -= min_y;
            }
        }
    }

    // Ensure stable, unique ids by de-duping with a suffix if needed.
    let mut used = HashSet::<String>::new();
    for node in &mut def.nodes {
        if used.insert(node.id.clone()) {
            continue;
        }
        let base = node.id.clone();
        for i in 2.. {
            let cand = format!("{base}_{i}");
            if used.insert(cand.clone()) {
                node.id = cand;
                break;
            }
        }
    }

    // Prune invalid `requires`.
    let ids: HashSet<String> = def.nodes.iter().map(|n| n.id.clone()).collect();
    for node in &mut def.nodes {
        node.requires.retain(|r| ids.contains(r));
        node.requires.sort();
        node.requires.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_editor_runtime() -> SkillTreeRuntime {
        let def = SkillTreeDef {
            version: 1,
            nodes: vec![SkillNodeDef {
                id: "start".to_string(),
                name: "START".to_string(),
                pos: Vec2i::new(0, 0),
                shape: vec![Vec2i::new(0, 0)],
                color: 3,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            }],
        };
        let progress = SkillTreeProgress {
            version: 1,
            money: 0,
            unlocked: vec!["start".to_string()],
        };
        let mut rt = SkillTreeRuntime::from_snapshot(SkillTreeSnapshot {
            def,
            progress,
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
        });
        rt.editor.enabled = true;
        rt
    }

    #[test]
    fn normalize_shifts_negative_rel_cells_into_shape_origin_and_adjusts_pos() {
        let mut def = SkillTreeDef {
            version: 1,
            nodes: vec![SkillNodeDef {
                id: "a".to_string(),
                name: "A".to_string(),
                pos: Vec2i::new(10, 10),
                shape: vec![Vec2i::new(-1, 0), Vec2i::new(0, 0)],
                color: 1,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            }],
        };

        normalize_and_validate(&mut def);
        let n = &def.nodes[0];

        assert_eq!(n.pos, Vec2i::new(9, 10));
        assert!(n.shape.contains(&Vec2i::new(0, 0)));
        assert!(n.shape.contains(&Vec2i::new(1, 0)));
        assert_eq!(n.shape.len(), 2);
    }

    #[test]
    fn normalize_dedups_shape_cells() {
        let mut def = SkillTreeDef {
            version: 1,
            nodes: vec![SkillNodeDef {
                id: "a".to_string(),
                name: "A".to_string(),
                pos: Vec2i::new(0, 0),
                shape: vec![Vec2i::new(0, 0), Vec2i::new(0, 0), Vec2i::new(1, 0)],
                color: 1,
                cost: 0,
                requires: vec![],
                effect: SkillEffect::None,
            }],
        };

        normalize_and_validate(&mut def);
        assert_eq!(def.nodes[0].shape.len(), 2);
    }

    #[test]
    fn rebuild_caches_prunes_unknown_unlocks_and_keeps_start_when_present() {
        let def = SkillTreeDef {
            version: 1,
            nodes: vec![
                SkillNodeDef {
                    id: "start".to_string(),
                    name: "START".to_string(),
                    pos: Vec2i::new(0, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 3,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                },
                SkillNodeDef {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    pos: Vec2i::new(1, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 4,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::None,
                },
            ],
        };
        let progress = SkillTreeProgress {
            version: 1,
            money: 0,
            unlocked: vec!["missing".to_string()],
        };

        let mut rt = SkillTreeRuntime {
            def,
            progress,
            def_path: None,
            progress_path: PathBuf::new(),
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
            editor_history: SkillTreeEditorHistory::default(),
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };

        rt.rebuild_caches();

        assert!(rt.progress.unlocked.iter().any(|id| id == "start"));
        assert!(!rt.progress.unlocked.iter().any(|id| id == "missing"));
        assert!(rt.is_unlocked("start"));
        assert!(!rt.is_unlocked("missing"));
    }

    #[test]
    fn skilltree_world_bounds_includes_all_node_cells() {
        let def = SkillTreeDef {
            version: 1,
            nodes: vec![
                SkillNodeDef {
                    id: "a".to_string(),
                    name: "A".to_string(),
                    pos: Vec2i::new(0, 0),
                    shape: vec![Vec2i::new(0, 0), Vec2i::new(1, 0)],
                    color: 1,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                },
                SkillNodeDef {
                    id: "b".to_string(),
                    name: "B".to_string(),
                    pos: Vec2i::new(-2, 3),
                    shape: vec![Vec2i::new(0, 0), Vec2i::new(0, 1)],
                    color: 2,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                },
            ],
        };

        let bounds = skilltree_world_bounds(&def).expect("expected bounds for non-empty def");
        assert_eq!(bounds.min, Vec2i::new(-2, 0));
        assert_eq!(bounds.max, Vec2i::new(1, 4));
    }

    #[test]
    fn clamp_camera_min_clamps_when_viewport_is_smaller_than_bounds() {
        let bounds = SkillTreeWorldBounds {
            min: Vec2i::new(0, 0),
            max: Vec2i::new(9, 9),
        };
        let view = Vec2f::new(5.0, 5.0);

        let clamped = clamp_camera_min_to_bounds(Vec2f::new(-5.0, -5.0), view, bounds, 0.0);
        assert_eq!(clamped, Vec2f::new(0.0, 0.0));

        let clamped = clamp_camera_min_to_bounds(Vec2f::new(10.0, 10.0), view, bounds, 0.0);
        assert_eq!(clamped, Vec2f::new(5.0, 5.0));
    }

    #[test]
    fn clamp_camera_min_centers_when_viewport_is_larger_than_bounds() {
        let bounds = SkillTreeWorldBounds {
            min: Vec2i::new(0, 0),
            max: Vec2i::new(9, 9),
        };
        let view = Vec2f::new(20.0, 20.0);

        // Bounds edges are [0,10) in each axis; centering a 20x20 viewport => cam_min = -5.
        let clamped = clamp_camera_min_to_bounds(Vec2f::new(123.0, 456.0), view, bounds, 0.0);
        assert_eq!(clamped, Vec2f::new(-5.0, -5.0));
    }

    #[test]
    fn editor_delete_guardrail_requires_confirm_press() {
        let mut rt = make_editor_runtime();
        let id = rt.editor_create_node_at(Vec2i::new(3, 0));
        assert!(rt.node_index(&id).is_some());

        assert!(!rt.editor_request_delete_selected());
        assert_eq!(rt.editor.pending_delete_id.as_deref(), Some(id.as_str()));
        assert!(rt.node_index(&id).is_some());

        assert!(rt.editor_request_delete_selected());
        assert!(rt.node_index(&id).is_none());
    }

    #[test]
    fn editor_undo_redo_roundtrips_create_mutation() {
        let mut rt = make_editor_runtime();
        let id = rt.editor_create_node_at(Vec2i::new(2, 2));
        assert!(rt.node_index(&id).is_some());
        assert!(rt.editor_can_undo());

        assert!(rt.editor_undo());
        assert!(rt.node_index(&id).is_none());
        assert!(rt.editor_can_redo());

        assert!(rt.editor_redo());
        assert!(rt.node_index(&id).is_some());
    }

    #[test]
    fn editor_duplicate_selected_uses_stable_copy_suffix() {
        let mut rt = make_editor_runtime();
        let id = rt.editor_create_node_at(Vec2i::new(4, 1));

        let dup_id = rt
            .editor_duplicate_selected()
            .expect("duplicate should succeed for selected node");
        assert_eq!(dup_id, format!("{id}_copy"));
        assert!(rt.node_index(&dup_id).is_some());
    }

    #[test]
    fn editor_select_matching_finds_by_name_prefix() {
        let mut rt = make_editor_runtime();
        let id = rt.editor_create_node_at(Vec2i::new(5, 0));
        if let Some(node) = rt.node_mut(&id) {
            node.name = "Power Burst".to_string();
        }
        rt.rebuild_caches();

        let hit = rt.editor_select_matching("pow");
        assert_eq!(hit.as_deref(), Some(id.as_str()));
        assert_eq!(rt.editor.selected.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn run_mods_accumulate_bottomwell_effects() {
        let def = SkillTreeDef {
            version: 1,
            nodes: vec![
                SkillNodeDef {
                    id: "start".to_string(),
                    name: "START".to_string(),
                    pos: Vec2i::new(0, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 3,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                },
                SkillNodeDef {
                    id: "deep".to_string(),
                    name: "DEEP".to_string(),
                    pos: Vec2i::new(1, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddDeepShaftRows { rows: 1 },
                },
                SkillNodeDef {
                    id: "ore".to_string(),
                    name: "ORE".to_string(),
                    pos: Vec2i::new(2, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddOreWeight { points: 2 },
                },
                SkillNodeDef {
                    id: "coin".to_string(),
                    name: "COIN".to_string(),
                    pos: Vec2i::new(3, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddCoinMoneyValue { points: 3 },
                },
            ],
        };
        let progress = SkillTreeProgress {
            version: 1,
            money: 0,
            unlocked: vec![
                "start".to_string(),
                "deep".to_string(),
                "ore".to_string(),
                "coin".to_string(),
            ],
        };
        let rt = SkillTreeRuntime::from_snapshot(SkillTreeSnapshot {
            def,
            progress,
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
        });
        let mods = rt.run_mods();
        assert_eq!(mods.deep_shaft_rows, 1);
        assert_eq!(mods.ore_weight_points, 2);
        assert_eq!(mods.coin_money_bonus, 3);
    }

    #[test]
    fn run_mods_clamp_bottomwell_effect_caps() {
        let def = SkillTreeDef {
            version: 1,
            nodes: vec![
                SkillNodeDef {
                    id: "start".to_string(),
                    name: "START".to_string(),
                    pos: Vec2i::new(0, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 3,
                    cost: 0,
                    requires: vec![],
                    effect: SkillEffect::None,
                },
                SkillNodeDef {
                    id: "deep".to_string(),
                    name: "DEEP".to_string(),
                    pos: Vec2i::new(1, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddDeepShaftRows { rows: 99 },
                },
                SkillNodeDef {
                    id: "align".to_string(),
                    name: "ALIGN".to_string(),
                    pos: Vec2i::new(2, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddHoleAlignChanceBp { basis_points: 99_999 },
                },
                SkillNodeDef {
                    id: "money".to_string(),
                    name: "MONEY".to_string(),
                    pos: Vec2i::new(3, 0),
                    shape: vec![Vec2i::new(0, 0)],
                    color: 1,
                    cost: 1,
                    requires: vec!["start".to_string()],
                    effect: SkillEffect::AddCoinMoneyValue { points: 99 },
                },
            ],
        };
        let progress = SkillTreeProgress {
            version: 1,
            money: 0,
            unlocked: vec![
                "start".to_string(),
                "deep".to_string(),
                "align".to_string(),
                "money".to_string(),
            ],
        };
        let rt = SkillTreeRuntime::from_snapshot(SkillTreeSnapshot {
            def,
            progress,
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
        });
        let mods = rt.run_mods();
        assert_eq!(mods.deep_shaft_rows, MAX_DEEP_SHAFT_ROWS);
        assert_eq!(mods.hole_align_chance_bp, MAX_HOLE_CHANCE_BP);
        assert_eq!(mods.coin_money_bonus, MAX_COIN_MONEY_BONUS);
    }

    #[test]
    fn skill_effect_alias_basis_points_deserializes_camel_case_basis_points() {
        let json = r#"{
            "id":"a",
            "name":"A",
            "pos":{"x":0,"y":0},
            "shape":[{"x":0,"y":0}],
            "color":1,
            "cost":1,
            "requires":["start"],
            "effect":{"type":"addHolePatchChanceBp","basisPoints":3500}
        }"#;
        let node: SkillNodeDef = serde_json::from_str(json).expect("node should deserialize");
        assert_eq!(
            node.effect,
            SkillEffect::AddHolePatchChanceBp { basis_points: 3500 }
        );
    }
}
