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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillTreeRunMods {
    pub extra_round_time_seconds: u32,
    pub gravity_faster_percent: u32,
    pub score_bonus_per_line: u32,
}

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
            dirty: false,
            status: None,
        }
    }
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

    // Cached indices (rebuilt on load/edit).
    id_to_index: HashMap<String, usize>,
    unlocked_set: HashSet<String>,
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
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };
        rt.rebuild_caches();
        rt
    }

    pub fn load_default() -> Self {
        let (def, def_path) = load_def_from_default_path();
        let progress_path = default_progress_path();
        let progress = load_progress(&progress_path).unwrap_or_default();

        let mut rt = Self {
            def,
            progress,
            def_path,
            progress_path,
            camera: SkillTreeCamera::default(),
            editor: SkillTreeEditorState::default(),
            id_to_index: HashMap::new(),
            unlocked_set: HashSet::new(),
        };
        rt.rebuild_caches();
        rt
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
        let (def, def_path) = load_def_from_default_path();
        self.def = def;
        self.def_path = def_path;
        self.rebuild_caches();
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
        }
    }

    pub fn editor_cycle_tool(&mut self) {
        self.editor.tool = match self.editor.tool {
            SkillTreeEditorTool::Select => SkillTreeEditorTool::Move,
            SkillTreeEditorTool::Move => SkillTreeEditorTool::AddCell,
            SkillTreeEditorTool::AddCell => SkillTreeEditorTool::RemoveCell,
            SkillTreeEditorTool::RemoveCell => SkillTreeEditorTool::ConnectPrereqs,
            SkillTreeEditorTool::ConnectPrereqs => SkillTreeEditorTool::Select,
        };
        self.editor.status = Some(format!("TOOL {:?}", self.editor.tool));
    }

    pub fn editor_set_tool(&mut self, tool: SkillTreeEditorTool) {
        self.editor.tool = tool;
        self.editor.status = Some(format!("TOOL {:?}", self.editor.tool));
    }

    pub fn editor_select(&mut self, id: &str, grab_offset: Option<Vec2i>) {
        self.editor.selected = Some(id.to_string());
        self.editor.move_grab_offset = grab_offset;
        self.editor.status = Some(format!("SELECT {id}"));
    }

    pub fn editor_selected_id(&self) -> Option<&str> {
        self.editor.selected.as_deref()
    }

    pub fn editor_clear_selection(&mut self) {
        self.editor.selected = None;
        self.editor.move_grab_offset = None;
        self.editor.status = Some("SELECT NONE".to_string());
    }

    pub fn node_index(&self, id: &str) -> Option<usize> {
        self.id_to_index.get(id).copied()
    }

    pub fn node_mut(&mut self, id: &str) -> Option<&mut SkillNodeDef> {
        let idx = self.node_index(id)?;
        self.def.nodes.get_mut(idx)
    }

    pub fn editor_create_node_at(&mut self, pos: Vec2i) -> String {
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
        self.editor.dirty = true;
        self.editor.status = Some(format!("NEW {id}"));
        id
    }

    pub fn editor_delete_selected(&mut self) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let Some(idx) = self.node_index(&id) else {
            self.editor.selected = None;
            return false;
        };

        self.def.nodes.remove(idx);
        for n in &mut self.def.nodes {
            n.requires.retain(|r| r != &id);
        }
        self.editor.selected = None;
        self.editor.connect_from = None;
        self.editor.move_grab_offset = None;
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("DEL {id}"));
        true
    }

    pub fn editor_move_selected_to(&mut self, new_pos: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let Some(node) = self.node_mut(&id) else {
            return false;
        };
        node.pos = new_pos;
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("MOVE {id}"));
        true
    }

    pub fn editor_toggle_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
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
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("PAINT {id}"));
        true
    }

    pub fn editor_add_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
        let Some(node) = self.node_mut(&id) else {
            return false;
        };

        let rel = Vec2i::new(world.x - node.pos.x, world.y - node.pos.y);
        if node.shape.iter().any(|c| c.x == rel.x && c.y == rel.y) {
            return false;
        }
        node.shape.push(rel);
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("ADD {id}"));
        true
    }

    pub fn editor_remove_cell_at_world(&mut self, world: Vec2i) -> bool {
        let Some(id) = self.editor.selected.clone() else {
            return false;
        };
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
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("REMOVE {id}"));
        true
    }

    pub fn editor_toggle_prereq(&mut self, prereq: &str, node_id: &str) -> bool {
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
        self.rebuild_caches();
        self.editor.dirty = true;
        self.editor.status = Some(format!("REQ {prereq} -> {node_id}"));
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

fn load_def_from_default_path() -> (SkillTreeDef, Option<PathBuf>) {
    if let Ok(p) = std::env::var("ROLLOUT_SKILLTREE_PATH") {
        let path = PathBuf::from(p);
        if let Ok(def) = load_def(&path) {
            return (def, Some(path));
        }
    }

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("skilltree.json");
    if let Ok(def) = load_def(&path) {
        return (def, Some(path));
    }

    (SkillTreeDef::default(), None)
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
}
