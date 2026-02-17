use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    ops::Add,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub const BOARD_WIDTH: usize = 10;
pub const BOARD_HEIGHT: usize = 20;
pub const NEXT_QUEUE_LEN: usize = 5;
pub const LOCK_DELAY_MS_DEFAULT: u32 = 500;
pub const LOCK_DELAY_MAX_MS_DEFAULT: u32 = 2_000;
pub const LINE_CLEAR_DELAY_MS_DEFAULT: u32 = 180;

const HARD_DROP_POINTS_PER_ROW: u32 = 2;
const EMPTY_LINE_CLEAR_ROWS: [usize; 0] = [];
pub type PieceId = u32;

// Bottomwell cell-type constants (active piece IDs use 1-7).
pub const CELL_EMPTY: u8 = 0;
pub const CELL_GLASS: u8 = 3;
pub const CELL_DIRT: u8 = 4;
pub const CELL_GARBAGE: u8 = 8;
pub const CELL_STONE: u8 = 9;
pub const CELL_ORE: u8 = 10;
pub const CELL_COIN: u8 = 11;
pub const CELL_GRASS: u8 = 12;
pub const CELL_MOSS: u8 = 13;
pub const CELL_MOSS_SEED: u8 = 14;
pub const CELL_SAND: u8 = 15;
pub const BASE_ORE_SCORE_VALUE: u32 = 50;
pub const BASE_COIN_SCORE_VALUE: u32 = 200;

pub const DEFAULT_BOTTOMWELL_ROWS: usize = 3;
pub const MAX_DEEP_SHAFT_ROWS: usize = 3;
pub const DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE: u32 = 4;
pub const DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT: u32 = 125;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottomwellRunMods {
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Piece {
    I,
    O,
    Glass,
    T,
    S,
    MossSeed,
    Z,
    J,
    L,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PieceTileRole {
    Body,
    Tip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContactImpactProfile {
    attack: u8,
    impetus: u8,
}

impl Piece {
    pub const ALL: [Piece; 6] = [
        Piece::O, // Stone O
        Piece::I, // Wood I4
        Piece::Glass, // Glass I3
        Piece::T, // Sand T
        Piece::S, // Dirt I2
        Piece::MossSeed, // Moss seed 1x1
    ];

    pub const LEGACY_ALL: [Piece; 8] = [
        Piece::I,
        Piece::O,
        Piece::Glass,
        Piece::T,
        Piece::S,
        Piece::Z,
        Piece::J,
        Piece::L,
    ];

    pub fn all() -> Vec<Piece> {
        Self::ALL.to_vec()
    }

    fn default_weight(self) -> u32 {
        match self {
            Piece::O => 30,
            Piece::I => 25,
            Piece::Glass => 25,
            Piece::T => 25,
            Piece::S => 20,
            Piece::MossSeed => 15,
            // Legacy piece IDs are retained for compatibility but not part of the
            // default elemental pool.
            Piece::Z | Piece::J | Piece::L => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vec2i {
    pub x: i32,
    pub y: i32,
}

impl Vec2i {
    pub const ZERO: Vec2i = Vec2i { x: 0, y: 0 };

    pub fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

impl Add for Vec2i {
    type Output = Vec2i;

    fn add(self, rhs: Vec2i) -> Self::Output {
        Vec2i::new(self.x + rhs.x, self.y + rhs.y)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RotationDir {
    Cw,
    Ccw,
    Half,
}

impl RotationDir {
    fn apply(self, rotation: u8, states: u8) -> u8 {
        let states = states.max(1);
        match self {
            RotationDir::Cw => (rotation + 1) % states,
            RotationDir::Ccw => (rotation + states - 1) % states,
            RotationDir::Half => (rotation + 2) % states,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DepthWallDef {
    pub id: String,
    pub depth_trigger: u64,
    pub hp: u32,
    pub biome_from: String,
    pub biome_to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DepthWallProgress {
    version: u32,
    #[serde(default)]
    broken_walls: HashSet<String>,
}

impl Default for DepthWallProgress {
    fn default() -> Self {
        Self {
            version: 1,
            broken_walls: HashSet::new(),
        }
    }
}

pub fn default_depth_wall_defs() -> Vec<DepthWallDef> {
    vec![DepthWallDef {
        id: "dirt_stone_wall".to_string(),
        depth_trigger: 5,
        hp: 24,
        biome_from: "dirt".to_string(),
        biome_to: "stone".to_string(),
    }]
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TetrisSnapshot {
    pub board: Vec<Vec<u8>>,
    pub current_piece: Option<Piece>,
    pub next_piece: Option<Piece>,
    pub next_queue: Vec<Piece>,
    pub held_piece: Option<Piece>,
    pub can_hold: bool,
    pub current_piece_pos: Vec2i,
    pub current_piece_rotation: u8,
    pub lines_cleared: u32,
    pub score: u32,
    pub game_over: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GravityAdvanceResult {
    Moved,
    Grounded,
    LineClearAnimating,
    Locked,
    NoActivePiece,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
enum LineClearPhase {
    #[default]
    Idle,
    Delay {
        rows: Vec<usize>,
        elapsed_ms: u32,
        duration_ms: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TetrisCore {
    board: Vec<Vec<u8>>,
    #[serde(default)]
    board_owner: Vec<Vec<Option<PieceId>>>,
    #[serde(default)]
    next_piece_id: PieceId,
    #[serde(default)]
    placed_piece_kind: HashMap<PieceId, Piece>,
    current_piece: Option<Piece>,
    next_queue: Vec<Piece>,
    held_piece: Option<Piece>,
    can_hold: bool,
    current_piece_pos: Vec2i,
    current_piece_rotation: u8,
    available_pieces: Vec<Piece>,
    piece_bag: Vec<Piece>,
    #[serde(default)]
    background_seed: u64,
    rng: Rng,
    lines_cleared: u32,
    score: u32,
    game_over: bool,
    #[serde(default = "default_lock_delay_ms")]
    lock_delay_ms: u32,
    #[serde(default = "default_lock_delay_max_ms")]
    lock_delay_max_ms: u32,
    #[serde(default = "default_line_clear_delay_ms")]
    line_clear_delay_ms: u32,
    #[serde(default)]
    grounded_lock_ms: u32,
    #[serde(default)]
    grounded_total_lock_ms: u32,
    #[serde(default)]
    grounded_for_lock: bool,
    #[serde(default)]
    line_clear_phase: LineClearPhase,
    last_kick_offset: Vec2i,
    #[serde(default)]
    bottomwell_enabled: bool,
    #[serde(default = "default_bottomwell_rows")]
    bottomwell_rows: usize,
    #[serde(default)]
    earth_depth: u64,
    #[serde(default)]
    ore_collected: u32,
    #[serde(default)]
    coins_collected: u32,
    #[serde(default)]
    deep_shaft_rows: usize,
    #[serde(default = "default_ore_score_value")]
    ore_score_value: u32,
    #[serde(default = "default_coin_score_value")]
    coin_score_value: u32,
    #[serde(default)]
    ore_money_value: u32,
    #[serde(default)]
    coin_money_value: u32,
    #[serde(default)]
    ore_weight_points: u32,
    #[serde(default)]
    coin_weight_points: u32,
    #[serde(default)]
    hole_patch_chance_bp: u32,
    #[serde(default)]
    hole_align_chance_bp: u32,
    #[serde(default = "default_depth_wall_defs")]
    depth_walls: Vec<DepthWallDef>,
    #[serde(default)]
    active_wall_id: Option<String>,
    #[serde(default)]
    active_wall_hp_remaining: u32,
    #[serde(default)]
    depth_progress_paused: bool,
    #[serde(default = "default_depth_wall_damage_per_line")]
    depth_wall_damage_per_line: u32,
    #[serde(default = "default_depth_wall_multi_clear_bonus_percent")]
    depth_wall_multi_clear_bonus_percent: u32,
    #[serde(default)]
    broken_walls: HashSet<String>,
    #[serde(default)]
    glass_shatter_count: u32,
    #[serde(skip, default = "default_depth_wall_progress_path")]
    depth_wall_progress_path: PathBuf,
}

fn default_bottomwell_rows() -> usize {
    DEFAULT_BOTTOMWELL_ROWS
}

fn default_ore_score_value() -> u32 {
    BASE_ORE_SCORE_VALUE
}

fn default_coin_score_value() -> u32 {
    BASE_COIN_SCORE_VALUE
}

fn default_lock_delay_ms() -> u32 {
    LOCK_DELAY_MS_DEFAULT
}

fn default_lock_delay_max_ms() -> u32 {
    LOCK_DELAY_MAX_MS_DEFAULT
}

fn default_line_clear_delay_ms() -> u32 {
    LINE_CLEAR_DELAY_MS_DEFAULT
}

fn default_depth_wall_damage_per_line() -> u32 {
    DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE
}

fn default_depth_wall_multi_clear_bonus_percent() -> u32 {
    DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT
}

fn default_depth_wall_progress_path() -> PathBuf {
    if let Ok(p) = std::env::var("ROLLOUT_DEPTH_WALL_PROGRESS_PATH") {
        return PathBuf::from(p);
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join("depth_wall_progress.json")
}

impl TetrisCore {
    pub fn new(seed: u64) -> Self {
        Self {
            board: vec![vec![0; BOARD_WIDTH]; BOARD_HEIGHT],
            board_owner: vec![vec![None; BOARD_WIDTH]; BOARD_HEIGHT],
            next_piece_id: 1,
            placed_piece_kind: HashMap::new(),
            current_piece: None,
            next_queue: Vec::new(),
            held_piece: None,
            can_hold: true,
            current_piece_pos: Vec2i::new(4, BOARD_HEIGHT as i32),
            current_piece_rotation: 0,
            available_pieces: vec![Piece::O],
            piece_bag: Vec::new(),
            background_seed: seed,
            rng: Rng::new(seed),
            lines_cleared: 0,
            score: 0,
            game_over: false,
            lock_delay_ms: LOCK_DELAY_MS_DEFAULT,
            lock_delay_max_ms: LOCK_DELAY_MAX_MS_DEFAULT,
            line_clear_delay_ms: LINE_CLEAR_DELAY_MS_DEFAULT,
            grounded_lock_ms: 0,
            grounded_total_lock_ms: 0,
            grounded_for_lock: false,
            line_clear_phase: LineClearPhase::Idle,
            last_kick_offset: Vec2i::ZERO,
            bottomwell_enabled: false,
            bottomwell_rows: DEFAULT_BOTTOMWELL_ROWS,
            earth_depth: 0,
            ore_collected: 0,
            coins_collected: 0,
            deep_shaft_rows: 0,
            ore_score_value: BASE_ORE_SCORE_VALUE,
            coin_score_value: BASE_COIN_SCORE_VALUE,
            ore_money_value: 0,
            coin_money_value: 0,
            ore_weight_points: 0,
            coin_weight_points: 0,
            hole_patch_chance_bp: 0,
            hole_align_chance_bp: 0,
            depth_walls: default_depth_wall_defs(),
            active_wall_id: None,
            active_wall_hp_remaining: 0,
            depth_progress_paused: false,
            depth_wall_damage_per_line: DEFAULT_DEPTH_WALL_DAMAGE_PER_LINE,
            depth_wall_multi_clear_bonus_percent: DEFAULT_DEPTH_WALL_MULTI_CLEAR_BONUS_PERCENT,
            broken_walls: HashSet::new(),
            glass_shatter_count: 0,
            depth_wall_progress_path: default_depth_wall_progress_path(),
        }
    }

    pub fn set_bottomwell_enabled(&mut self, enabled: bool) {
        self.bottomwell_enabled = enabled;
        self.resize_board_to_effective_height();
    }

    pub fn bottomwell_enabled(&self) -> bool {
        self.bottomwell_enabled
    }

    pub fn set_bottomwell_run_mods(&mut self, mods: BottomwellRunMods) {
        self.deep_shaft_rows = (mods.deep_shaft_rows as usize).min(MAX_DEEP_SHAFT_ROWS);
        self.ore_weight_points = mods.ore_weight_points;
        self.coin_weight_points = mods.coin_weight_points;
        self.ore_score_value = BASE_ORE_SCORE_VALUE.saturating_add(mods.ore_score_bonus);
        self.coin_score_value = BASE_COIN_SCORE_VALUE.saturating_add(mods.coin_score_bonus);
        self.ore_money_value = mods.ore_money_bonus;
        self.coin_money_value = mods.coin_money_bonus;
        self.hole_patch_chance_bp = mods.hole_patch_chance_bp.min(10_000);
        self.hole_align_chance_bp = mods.hole_align_chance_bp.min(10_000);
        self.resize_board_to_effective_height();
    }

    pub fn deep_shaft_rows(&self) -> usize {
        self.deep_shaft_rows
    }

    pub fn effective_board_height(&self) -> usize {
        if self.bottomwell_enabled {
            BOARD_HEIGHT
                .saturating_add(self.bottomwell_rows)
                .saturating_add(self.deep_shaft_rows)
        } else {
            BOARD_HEIGHT
        }
    }

    pub fn refinery_money_from_collected_resources(&self) -> u32 {
        self.ore_collected
            .saturating_mul(self.ore_money_value)
            .saturating_add(self.coins_collected.saturating_mul(self.coin_money_value))
    }

    pub fn earth_depth(&self) -> u64 {
        self.earth_depth
    }

    /// Render-facing depth signal used by background/camera systems.
    ///
    /// This intentionally maps to canonical revealed-earth depth instead of
    /// gameplay line-count metrics so world-motion effects stay in sync with
    /// bottomwell reveal progression.
    ///
    /// When bottomwell is enabled, the initial prefill rows are treated as the
    /// zero point so run-start visuals begin at surface depth.
    pub fn background_depth_rows(&self) -> u32 {
        let prefill_rows = self.initial_bottomwell_fill_rows() as u64;
        self.earth_depth
            .saturating_sub(prefill_rows)
            .min(u32::MAX as u64) as u32
    }

    pub fn ore_collected(&self) -> u32 {
        self.ore_collected
    }

    pub fn coins_collected(&self) -> u32 {
        self.coins_collected
    }

    pub fn active_wall_id(&self) -> Option<&str> {
        self.active_wall_id.as_deref()
    }

    pub fn active_wall_hp_remaining(&self) -> u32 {
        self.active_wall_hp_remaining
    }

    pub fn depth_progress_paused(&self) -> bool {
        self.depth_progress_paused
    }

    pub fn active_wall_label(&self) -> Option<String> {
        let wall = self.active_wall_def()?;
        Some(format!(
            "{} TO {} WALL",
            wall.biome_from.to_ascii_uppercase(),
            wall.biome_to.to_ascii_uppercase()
        ))
    }

    pub fn set_depth_wall_defs(&mut self, mut defs: Vec<DepthWallDef>) {
        for def in &mut defs {
            if def.id.trim().is_empty() {
                def.id = "depth_wall".to_string();
            }
            if def.hp == 0 {
                def.hp = 1;
            }
        }
        defs.sort_unstable_by_key(|def| def.depth_trigger);
        self.depth_walls = defs;
    }

    pub fn set_depth_wall_damage_tuning(&mut self, per_line_damage: u32, multi_bonus_percent: u32) {
        self.depth_wall_damage_per_line = per_line_damage.max(1);
        self.depth_wall_multi_clear_bonus_percent = multi_bonus_percent.max(100);
    }

    pub fn set_depth_wall_progress_path(&mut self, path: PathBuf) {
        self.depth_wall_progress_path = path;
    }

    pub fn set_available_pieces(&mut self, pieces: Vec<Piece>) {
        if pieces.is_empty() {
            self.available_pieces = vec![Piece::O];
        } else {
            self.available_pieces = pieces;
        }
    }

    fn initial_bottomwell_fill_rows(&self) -> usize {
        if !self.bottomwell_enabled {
            return 0;
        }
        self.bottomwell_rows.saturating_add(self.deep_shaft_rows)
    }

    fn resize_board_to_effective_height(&mut self) {
        let target = self.effective_board_height();
        if self.board.len() > target {
            self.board.truncate(target);
            self.board_owner.truncate(target);
        } else {
            while self.board.len() < target {
                self.board.push(vec![0; BOARD_WIDTH]);
                self.board_owner.push(vec![None; BOARD_WIDTH]);
            }
        }
        self.current_piece_pos = Vec2i::new(4, self.board.len() as i32);
    }

    pub fn initialize_game(&mut self) {
        self.board = vec![vec![0; BOARD_WIDTH]; self.effective_board_height()];
        self.board_owner = vec![vec![None; BOARD_WIDTH]; self.effective_board_height()];
        self.next_piece_id = 1;
        self.placed_piece_kind.clear();
        self.current_piece = None;
        self.next_queue.clear();
        self.held_piece = None;
        self.can_hold = true;
        self.current_piece_pos = Vec2i::new(4, self.board.len() as i32);
        self.current_piece_rotation = 0;
        self.piece_bag.clear();
        self.lines_cleared = 0;
        self.score = 0;
        self.game_over = false;
        self.clear_lock_delay_state();
        self.line_clear_phase = LineClearPhase::Idle;
        self.last_kick_offset = Vec2i::ZERO;
        self.earth_depth = 0;
        self.ore_collected = 0;
        self.coins_collected = 0;
        self.active_wall_id = None;
        self.active_wall_hp_remaining = 0;
        self.depth_progress_paused = false;
        self.glass_shatter_count = 0;
        self.reload_depth_wall_progress();

        if self.bottomwell_enabled {
            self.prefill_bottomwell();
        }

        self.spawn_new_piece();
    }

    pub fn board(&self) -> &[Vec<u8>] {
        &self.board
    }

    pub fn board_piece_ids(&self) -> &[Vec<Option<PieceId>>] {
        &self.board_owner
    }

    pub fn placed_piece_kind(&self, piece_id: PieceId) -> Option<Piece> {
        self.placed_piece_kind.get(&piece_id).copied()
    }

    pub fn board_with_active_piece(&self) -> Vec<Vec<u8>> {
        let mut board = self.board.clone();
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return board,
        };

        let grid = piece_grid(piece, self.current_piece_rotation);
        let offset = piece_board_offset(piece);
        let piece_type = piece_type(piece);

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }

                let board_x = self.current_piece_pos.x + gx as i32 - offset;
                let board_y = self.current_piece_pos.y - gy as i32 + offset;

                if board_x >= 0
                    && board_x < BOARD_WIDTH as i32
                    && board_y >= 0
                    && board_y < self.board.len() as i32
                {
                    board[board_y as usize][board_x as usize] = piece_type;
                }
            }
        }

        board
    }

    pub fn current_piece(&self) -> Option<Piece> {
        self.current_piece
    }

    pub fn next_piece(&self) -> Option<Piece> {
        self.next_queue.first().copied()
    }

    pub fn next_queue(&self) -> &[Piece] {
        &self.next_queue
    }

    pub fn held_piece(&self) -> Option<Piece> {
        self.held_piece
    }

    pub fn can_hold(&self) -> bool {
        self.can_hold
    }

    pub fn current_piece_pos(&self) -> Vec2i {
        self.current_piece_pos
    }

    pub fn current_piece_rotation(&self) -> u8 {
        self.current_piece_rotation
    }

    pub fn active_piece_tip_cell(&self) -> Option<(i32, i32)> {
        let piece = self.current_piece?;
        let (gx, gy) = Self::tip_grid_cell(piece, self.current_piece_rotation)?;
        let offset = piece_board_offset(piece);
        Some((
            self.current_piece_pos.x + gx as i32 - offset,
            self.current_piece_pos.y - gy as i32 + offset,
        ))
    }

    /// Returns the position the current piece would occupy after a hard drop,
    /// without mutating the game state.
    pub fn ghost_piece_pos(&self) -> Option<Vec2i> {
        if self.current_piece.is_none() {
            return None;
        }

        let rotation = self.current_piece_rotation;
        let mut pos = self.current_piece_pos;

        // If the current position is invalid (e.g. game-over spawn overlap), don't
        // attempt to compute a ghost.
        if !self.is_valid_position(pos, rotation) {
            return None;
        }

        while self.is_valid_position(pos + Vec2i::new(0, -1), rotation) {
            pos = pos + Vec2i::new(0, -1);
        }

        Some(pos)
    }

    pub fn lines_cleared(&self) -> u32 {
        self.lines_cleared
    }

    pub fn background_seed(&self) -> u64 {
        self.background_seed
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn glass_shatter_count(&self) -> u32 {
        self.glass_shatter_count
    }

    pub fn lock_delay_ms(&self) -> u32 {
        self.lock_delay_ms
    }

    pub fn set_lock_delay_ms(&mut self, lock_delay_ms: u32) {
        self.lock_delay_ms = lock_delay_ms;
        self.lock_delay_max_ms = self.lock_delay_max_ms.max(self.lock_delay_ms);
    }

    pub fn lock_delay_max_ms(&self) -> u32 {
        self.lock_delay_max_ms
    }

    pub fn line_clear_delay_ms(&self) -> u32 {
        self.line_clear_delay_ms
    }

    pub fn set_line_clear_delay_ms(&mut self, line_clear_delay_ms: u32) {
        self.line_clear_delay_ms = line_clear_delay_ms;
    }

    pub fn set_lock_delay_max_ms(&mut self, lock_delay_max_ms: u32) {
        self.lock_delay_max_ms = lock_delay_max_ms.max(self.lock_delay_ms);
    }

    pub fn grounded_lock_ms(&self) -> u32 {
        self.grounded_lock_ms
    }

    pub fn grounded_total_lock_ms(&self) -> u32 {
        self.grounded_total_lock_ms
    }

    pub fn is_grounded_for_lock_delay(&self) -> bool {
        self.grounded_for_lock
    }

    pub fn is_line_clear_active(&self) -> bool {
        !matches!(self.line_clear_phase, LineClearPhase::Idle)
    }

    pub fn line_clear_rows(&self) -> &[usize] {
        match &self.line_clear_phase {
            LineClearPhase::Delay { rows, .. } => rows,
            LineClearPhase::Idle => &EMPTY_LINE_CLEAR_ROWS,
        }
    }

    pub fn line_clear_progress(&self) -> f32 {
        match self.line_clear_phase {
            LineClearPhase::Delay {
                elapsed_ms,
                duration_ms,
                ..
            } => {
                if duration_ms == 0 {
                    1.0
                } else {
                    (elapsed_ms as f32 / duration_ms as f32).clamp(0.0, 1.0)
                }
            }
            LineClearPhase::Idle => 0.0,
        }
    }

    pub fn add_score(&mut self, bonus: u32) {
        self.score = self.score.saturating_add(bonus);
    }

    pub fn is_game_over(&self) -> bool {
        self.game_over
    }

    pub fn snapshot(&self) -> TetrisSnapshot {
        TetrisSnapshot {
            board: self.board.clone(),
            current_piece: self.current_piece,
            next_piece: self.next_piece(),
            next_queue: self.next_queue.clone(),
            held_piece: self.held_piece,
            can_hold: self.can_hold,
            current_piece_pos: self.current_piece_pos,
            current_piece_rotation: self.current_piece_rotation,
            lines_cleared: self.lines_cleared,
            score: self.score,
            game_over: self.game_over,
        }
    }

    pub fn set_current_piece_for_test(&mut self, piece: Piece, pos: Vec2i, rotation: u8) {
        self.current_piece = Some(piece);
        self.current_piece_pos = pos;
        self.current_piece_rotation = rotation % 4;
        self.clear_lock_delay_state();
        self.line_clear_phase = LineClearPhase::Idle;
    }

    pub fn set_cell(&mut self, x: usize, y: usize, value: u8) {
        if y < self.board.len() && x < BOARD_WIDTH {
            self.board[y][x] = value;
            self.board_owner[y][x] = None;
        }
    }

    /// Advance board-level material simulation once per game turn.
    ///
    /// Each seed grows at most one dirt tile into moss per turn, with growth
    /// restricted to BFS distance <= 3 from that seed.
    pub fn advance_material_turn(&mut self) {
        if self.is_line_clear_active() {
            return;
        }

        let mut seeds = Vec::new();
        for y in 0..self.board.len() {
            for x in 0..BOARD_WIDTH {
                if self.board[y][x] == CELL_MOSS_SEED {
                    seeds.push((x, y));
                }
            }
        }

        for (seed_x, seed_y) in seeds {
            let _ = self.grow_moss_from_seed(seed_x, seed_y);
        }
        let _ = self.settle_sand_cells();
        let _ = self.start_line_clear_phase_if_needed();
    }

    pub fn draw_piece(&mut self) -> Piece {
        if self.available_pieces.is_empty() {
            self.available_pieces = vec![Piece::O];
        }
        let total_weight = self
            .available_pieces
            .iter()
            .map(|piece| piece.default_weight())
            .sum::<u32>()
            .max(1);
        let mut pick = self.rng.next_u32() % total_weight;
        for &piece in &self.available_pieces {
            let w = piece.default_weight();
            if pick < w {
                return piece;
            }
            pick -= w;
        }
        Piece::O
    }

    fn fill_next_queue(&mut self) {
        while self.next_queue.len() < NEXT_QUEUE_LEN {
            let piece = self.draw_piece();
            self.next_queue.push(piece);
        }
    }

    pub fn spawn_new_piece(&mut self) -> bool {
        self.fill_next_queue();
        let piece = if self.next_queue.is_empty() {
            self.draw_piece()
        } else {
            self.next_queue.remove(0)
        };
        self.current_piece = Some(piece);
        self.current_piece_pos = Vec2i::new(4, self.board.len() as i32);
        self.current_piece_rotation = 0;
        self.clear_lock_delay_state();
        self.line_clear_phase = LineClearPhase::Idle;
        self.fill_next_queue();
        self.can_hold = true;
        self.last_kick_offset = Vec2i::ZERO;

        if !self.is_valid_position(self.current_piece_pos, self.current_piece_rotation) {
            self.game_over = true;
            return false;
        }

        true
    }

    pub fn hold_piece(&mut self) -> bool {
        if self.game_over || !self.can_hold || self.is_line_clear_active() {
            return false;
        }

        let Some(current) = self.current_piece else {
            return false;
        };

        if let Some(held) = self.held_piece {
            self.held_piece = Some(current);
            self.current_piece = Some(held);
            self.current_piece_pos = Vec2i::new(4, self.board.len() as i32);
            self.current_piece_rotation = 0;
            self.clear_lock_delay_state();
            self.last_kick_offset = Vec2i::ZERO;
            self.can_hold = false;

            if !self.is_valid_position(self.current_piece_pos, self.current_piece_rotation) {
                self.game_over = true;
                return false;
            }

            return true;
        }

        // Empty hold: store the current piece and consume the next piece from the queue.
        self.held_piece = Some(current);
        let ok = self.spawn_new_piece();
        self.can_hold = false;
        ok
    }

    pub fn is_valid_position(&self, pos: Vec2i, rotation: u8) -> bool {
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return false,
        };
        let grid = piece_grid(piece, rotation);
        let offset = piece_board_offset(piece);

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }

                let board_x = pos.x + gx as i32 - offset;
                let board_y = pos.y - gy as i32 + offset;

                if board_x < 0 || board_x >= BOARD_WIDTH as i32 {
                    return false;
                }
                if board_y < 0 {
                    return false;
                }
                if board_y < self.board.len() as i32 {
                    let cell_value = self.board[board_y as usize][board_x as usize];
                    if cell_value != 0 {
                        return false;
                    }
                }
            }
        }

        true
    }

    pub fn move_piece(&mut self, dir: Vec2i) -> bool {
        if self.is_line_clear_active() {
            return false;
        }
        let old_pos = self.current_piece_pos;
        let new_pos = self.current_piece_pos + dir;
        if self.is_valid_position(new_pos, self.current_piece_rotation) {
            self.current_piece_pos = new_pos;
            self.handle_successful_adjustment(new_pos != old_pos);
            return true;
        }
        false
    }

    pub fn move_piece_down(&mut self) -> bool {
        if self.is_line_clear_active() {
            return false;
        }
        self.move_piece(Vec2i::new(0, -1))
    }

    pub fn advance_with_gravity(&mut self, dt_ms: u32) -> GravityAdvanceResult {
        if self.advance_line_clear_phase(dt_ms) {
            return GravityAdvanceResult::Locked;
        }
        if self.is_line_clear_active() {
            return GravityAdvanceResult::LineClearAnimating;
        }
        if self.game_over || self.current_piece.is_none() {
            return GravityAdvanceResult::NoActivePiece;
        }

        if self.move_piece_down() {
            return GravityAdvanceResult::Moved;
        }

        if !self.grounded_for_lock {
            self.grounded_for_lock = true;
            self.grounded_total_lock_ms = self.grounded_total_lock_ms.saturating_add(dt_ms);
            if self.grounded_total_lock_ms >= self.lock_delay_max_ms {
                self.lock_active_piece();
                return GravityAdvanceResult::Locked;
            }
            return GravityAdvanceResult::Grounded;
        }

        self.grounded_lock_ms = self.grounded_lock_ms.saturating_add(dt_ms);
        self.grounded_total_lock_ms = self.grounded_total_lock_ms.saturating_add(dt_ms);
        if self.grounded_lock_ms >= self.lock_delay_ms
            || self.grounded_total_lock_ms >= self.lock_delay_max_ms
        {
            self.lock_active_piece();
            return GravityAdvanceResult::Locked;
        }

        GravityAdvanceResult::Grounded
    }

    pub fn rotate_piece(&mut self, dir: RotationDir) -> bool {
        if self.is_line_clear_active() {
            return false;
        }
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return false,
        };
        let before_cells =
            Self::occupied_cells(piece, self.current_piece_pos, self.current_piece_rotation);
        let new_rotation = dir.apply(self.current_piece_rotation, piece_rotation_states(piece));
        if self.try_rotation_with_kicks(new_rotation) {
            self.current_piece_rotation = new_rotation;
            let after_cells =
                Self::occupied_cells(piece, self.current_piece_pos, self.current_piece_rotation);
            self.handle_successful_adjustment(after_cells != before_cells);
            return true;
        }
        false
    }

    pub fn hard_drop(&mut self) -> i32 {
        if self.is_line_clear_active() || self.current_piece.is_none() {
            return 0;
        }

        let mut drop_distance = 0u32;
        loop {
            let next_pos = self.current_piece_pos + Vec2i::new(0, -1);
            if self.is_valid_position(next_pos, self.current_piece_rotation) {
                self.current_piece_pos = next_pos;
                drop_distance = drop_distance.saturating_add(1);
                continue;
            }
            if self.apply_hard_drop_impact(next_pos) {
                continue;
            }
            break;
        }

        self.score = self
            .score
            .saturating_add(drop_distance.saturating_mul(HARD_DROP_POINTS_PER_ROW));

        self.clear_lock_delay_state();
        self.lock_active_piece();
        drop_distance as i32
    }

    pub fn clear_lines(&mut self) -> usize {
        let lines_to_clear = self.detect_full_rows();
        self.clear_specific_lines(lines_to_clear)
    }

    fn apply_hard_drop_impact(&mut self, next_pos: Vec2i) -> bool {
        let Some(piece) = self.current_piece else {
            return false;
        };
        if matches!(piece, Piece::I) && self.apply_spear_tip_impact(next_pos) {
            return true;
        }
        self.apply_collision_crush_impact(
            piece,
            next_pos,
            Vec2i::new(0, -1),
            ContactImpactProfile {
                attack: 1,
                impetus: 1,
            },
        )
    }

    fn apply_spear_tip_impact(&mut self, next_pos: Vec2i) -> bool {
        let piece = Piece::I;
        let rotation = self.current_piece_rotation;
        let Some((tip_gx, tip_gy)) = Self::tip_grid_cell(piece, rotation) else {
            return false;
        };
        let grid = piece_grid(piece, rotation);
        if grid.cell(tip_gx, tip_gy) != 1 {
            return false;
        }
        let offset = piece_board_offset(piece);
        let target_x = next_pos.x + tip_gx as i32 - offset;
        let target_y = next_pos.y - tip_gy as i32 + offset;
        if target_x < 0
            || target_x >= BOARD_WIDTH as i32
            || target_y < 0
            || target_y >= self.board.len() as i32
        {
            return false;
        }
        let impact_profile = Self::spear_tip_impact_profile(rotation, Vec2i::new(0, -1));
        self.apply_contact_chain_crush(
            target_x as usize,
            target_y as usize,
            Vec2i::new(0, -1),
            impact_profile,
        )
    }

    fn apply_collision_crush_impact(
        &mut self,
        piece: Piece,
        next_pos: Vec2i,
        impact_dir: Vec2i,
        impact_profile: ContactImpactProfile,
    ) -> bool {
        if impact_profile.attack == 0 || impact_profile.impetus == 0 {
            return false;
        }
        let grid = piece_grid(piece, self.current_piece_rotation);
        let offset = piece_board_offset(piece);
        let mut collided_cells = Vec::new();
        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }
                let target_x = next_pos.x + gx as i32 - offset;
                let target_y = next_pos.y - gy as i32 + offset;
                if target_x < 0
                    || target_x >= BOARD_WIDTH as i32
                    || target_y < 0
                    || target_y >= self.board.len() as i32
                {
                    // Board bounds are always non-crushable supports.
                    return false;
                }
                let tx = target_x as usize;
                let ty = target_y as usize;
                if self.board[ty][tx] == CELL_EMPTY {
                    continue;
                }
                if !collided_cells.contains(&(tx, ty)) {
                    collided_cells.push((tx, ty));
                }
            }
        }
        if collided_cells.is_empty() {
            return false;
        }
        let mut crushed_any = false;
        for (x, y) in collided_cells {
            crushed_any |= self.apply_contact_chain_crush(x, y, impact_dir, impact_profile);
        }
        crushed_any
    }

    fn apply_contact_chain_crush(
        &mut self,
        start_x: usize,
        start_y: usize,
        impact_dir: Vec2i,
        impact_profile: ContactImpactProfile,
    ) -> bool {
        if impact_profile.attack == 0 || impact_profile.impetus == 0 {
            return false;
        }

        let mut x = start_x as i32;
        let mut y = start_y as i32;
        let mut remaining_impetus = impact_profile.impetus;
        let mut crushed_any = false;

        loop {
            if x < 0 || x >= BOARD_WIDTH as i32 || y < 0 || y >= self.board.len() as i32 {
                break;
            }
            let tx = x as usize;
            let ty = y as usize;
            let cell = self.board[ty][tx];
            if cell == CELL_EMPTY {
                break;
            }
            let defense = Self::cell_contact_defense(cell);
            if impact_profile.attack <= defense {
                break;
            }
            if !self.apply_tile_damage(tx, ty, impact_profile.attack) {
                break;
            }
            crushed_any = true;
            remaining_impetus = remaining_impetus.saturating_sub(defense);
            if remaining_impetus == 0 {
                break;
            }
            x += impact_dir.x;
            y += impact_dir.y;
        }

        crushed_any
    }

    fn cell_contact_defense(cell: u8) -> u8 {
        match cell {
            CELL_EMPTY => 0,
            CELL_MOSS => 0,
            CELL_DIRT | CELL_GLASS | CELL_SAND => 1,
            CELL_STONE => 2,
            CELL_MOSS_SEED => 1,
            // Keep unknown cells at least minimally resistive.
            _ => 1,
        }
    }

    fn spear_tip_impact_profile(rotation: u8, impact_dir: Vec2i) -> ContactImpactProfile {
        if impact_dir == Vec2i::new(0, -1) && Self::is_i_piece_facing_down(rotation) {
            ContactImpactProfile {
                attack: 2,
                impetus: 2,
            }
        } else {
            ContactImpactProfile {
                attack: 1,
                impetus: 1,
            }
        }
    }

    fn is_i_piece_facing_down(rotation: u8) -> bool {
        rotation % piece_rotation_states(Piece::I) == 1
    }

    fn apply_tile_damage(&mut self, x: usize, y: usize, damage: u8) -> bool {
        if damage == 0 || y >= self.board.len() || x >= BOARD_WIDTH {
            return false;
        }
        let cell = self.board[y][x];
        if cell == CELL_EMPTY {
            return false;
        }

        match cell {
            // Dirt I2: break one tile.
            CELL_DIRT => {
                self.clear_board_cell(x, y);
                true
            }
            // Glass I3: break entire placed piece if possible.
            CELL_GLASS => {
                if let Some(piece_id) = self.board_owner[y][x] {
                    if self.placed_piece_kind.get(&piece_id) == Some(&Piece::Glass) {
                        self.shatter_piece(piece_id);
                        return true;
                    }
                }
                self.clear_board_cell(x, y);
                true
            }
            CELL_SAND => {
                self.clear_board_cell(x, y);
                true
            }
            CELL_MOSS => {
                self.clear_board_cell(x, y);
                true
            }
            _ => false,
        }
    }

    fn grow_moss_from_seed(&mut self, seed_x: usize, seed_y: usize) -> bool {
        const MAX_MOSS_BFS_DISTANCE: usize = 3;

        if seed_y >= self.board.len() || seed_x >= BOARD_WIDTH {
            return false;
        }

        let mut visited = vec![vec![false; BOARD_WIDTH]; self.board.len()];
        let mut queue = VecDeque::new();
        visited[seed_y][seed_x] = true;
        queue.push_back((seed_x, seed_y, 0usize));

        while let Some((x, y, dist)) = queue.pop_front() {
            if dist > 0 && self.board[y][x] == CELL_DIRT {
                self.board[y][x] = CELL_MOSS;
                self.board_owner[y][x] = None;
                return true;
            }
            if dist == MAX_MOSS_BFS_DISTANCE {
                continue;
            }

            // Stable neighbor ordering keeps growth deterministic.
            let neighbors = [
                (x.wrapping_sub(1), y, x > 0),
                (x + 1, y, x + 1 < BOARD_WIDTH),
                (x, y.wrapping_sub(1), y > 0),
                (x, y + 1, y + 1 < self.board.len()),
            ];
            for (nx, ny, in_bounds) in neighbors {
                if !in_bounds || visited[ny][nx] {
                    continue;
                }
                let cell = self.board[ny][nx];
                if !matches!(cell, CELL_DIRT | CELL_MOSS | CELL_MOSS_SEED) {
                    continue;
                }
                visited[ny][nx] = true;
                queue.push_back((nx, ny, dist + 1));
            }
        }

        false
    }

    fn settle_sand_cells(&mut self) -> bool {
        if self.board.len() < 2 {
            return false;
        }

        let mut moved = false;
        for y in 1..self.board.len() {
            for x in 0..BOARD_WIDTH {
                if self.board[y][x] != CELL_SAND {
                    continue;
                }

                let below_y = y - 1;
                if self.board[below_y][x] == CELL_EMPTY {
                    self.board[below_y][x] = CELL_SAND;
                    self.board_owner[below_y][x] = None;
                    self.board[y][x] = CELL_EMPTY;
                    self.board_owner[y][x] = None;
                    moved = true;
                    continue;
                }

                continue;
            }
        }

        moved
    }

    fn shatter_piece(&mut self, piece_id: PieceId) {
        for y in 0..self.board.len() {
            for x in 0..BOARD_WIDTH {
                if self.board_owner[y][x] == Some(piece_id) {
                    self.board[y][x] = CELL_EMPTY;
                    self.board_owner[y][x] = None;
                }
            }
        }
        self.placed_piece_kind.remove(&piece_id);
        self.glass_shatter_count = self.glass_shatter_count.saturating_add(1);
    }

    fn clear_board_cell(&mut self, x: usize, y: usize) {
        if y >= self.board.len() || x >= BOARD_WIDTH {
            return;
        }
        let owner = self.board_owner[y][x];
        self.board[y][x] = CELL_EMPTY;
        self.board_owner[y][x] = None;
        if let Some(piece_id) = owner {
            let still_exists = self
                .board_owner
                .iter()
                .flat_map(|row| row.iter())
                .any(|&id| id == Some(piece_id));
            if !still_exists {
                self.placed_piece_kind.remove(&piece_id);
            }
        }
    }

    fn cleanup_piece_owners(&mut self) {
        let mut still_present = HashSet::new();
        for row in &self.board_owner {
            for &owner in row {
                if let Some(id) = owner {
                    still_present.insert(id);
                }
            }
        }
        self.placed_piece_kind
            .retain(|piece_id, _| still_present.contains(piece_id));
    }

    // ── Bottomwell helpers ──────────────────────────────────────────

    /// Generate a deterministic earth row for the given depth.
    /// Uses `background_seed` + `depth` to seed a local RNG so the
    /// sequence is reproducible across runs with the same seed.
    pub fn generate_earth_row(seed: u64, depth: u64) -> Vec<u8> {
        Self::generate_earth_row_with_tuning(seed, depth, 0, 0, 0, 0)
    }

    fn generate_earth_row_for_depth(&self, depth: u64) -> Vec<u8> {
        Self::generate_earth_row_with_tuning(
            self.background_seed,
            depth,
            self.ore_weight_points,
            self.coin_weight_points,
            self.hole_patch_chance_bp,
            self.hole_align_chance_bp,
        )
    }

    fn generate_earth_row_with_tuning(
        seed: u64,
        depth: u64,
        ore_weight_points: u32,
        coin_weight_points: u32,
        hole_patch_chance_bp: u32,
        hole_align_chance_bp: u32,
    ) -> Vec<u8> {
        let mixed = seed
            .wrapping_mul(0x5851_F42D_4C95_7F2D)
            .wrapping_add(depth.wrapping_mul(0x14057B7EF767814F));
        let mut rng = Rng::new(if mixed == 0 { 1 } else { mixed });

        // First-biome earth should match the Dirt piece material/appearance.
        let mut row = vec![CELL_DIRT; BOARD_WIDTH];

        // 1) base holes
        let mut hole_positions = vec![rng.next_u32() as usize % BOARD_WIDTH];
        let second_hole_roll = rng.next_u32() % 100;
        let wants_second_hole =
            (hole_patch_chance_bp > 0 || hole_align_chance_bp > 0) && second_hole_roll < 35;
        if wants_second_hole {
            let mut second = rng.next_u32() as usize % BOARD_WIDTH;
            if second == hole_positions[0] {
                second = (second + 1) % BOARD_WIDTH;
            }
            hole_positions.push(second);
        }

        // 2) patch pass
        if hole_positions.len() == 2 && (rng.next_u32() % 10_000) < hole_patch_chance_bp.min(10_000) {
            hole_positions.remove(1);
        }

        // 3) alignment pass
        if depth > 0
            && !hole_positions.is_empty()
            && (rng.next_u32() % 10_000) < hole_align_chance_bp.min(10_000)
        {
            let prev_mixed = seed
                .wrapping_mul(0x5851_F42D_4C95_7F2D)
                .wrapping_add((depth - 1).wrapping_mul(0x14057B7EF767814F));
            let mut prev_rng = Rng::new(if prev_mixed == 0 { 1 } else { prev_mixed });
            let prev_col = prev_rng.next_u32() as usize % BOARD_WIDTH;
            hole_positions[0] = prev_col;
            if hole_positions.len() > 1 && hole_positions[1] == prev_col {
                hole_positions.remove(1);
            }
        }

        for &pos in &hole_positions {
            row[pos] = CELL_EMPTY;
        }

        // 4) material fill pass
        let base_ore_threshold = if depth > 20 {
            15u32
        } else if depth > 10 {
            10u32
        } else {
            5u32
        };
        let base_coin_threshold = if depth > 30 {
            8u32
        } else if depth > 15 {
            4u32
        } else {
            2u32
        };
        let mut ore_threshold = base_ore_threshold.saturating_add(ore_weight_points).min(35);
        let coin_threshold = base_coin_threshold
            .saturating_add(coin_weight_points)
            .min(20);
        if ore_threshold.saturating_add(coin_threshold) > 60 {
            ore_threshold = 60u32.saturating_sub(coin_threshold.min(60));
        }

        for x in 0..BOARD_WIDTH {
            if row[x] == CELL_EMPTY {
                continue;
            }
            let roll = rng.next_u32() % 100;
            if roll < coin_threshold {
                row[x] = CELL_COIN;
            } else if roll < coin_threshold + ore_threshold {
                row[x] = CELL_ORE;
            } else if roll < coin_threshold + ore_threshold + 30 {
                row[x] = CELL_STONE;
            }
        }

        row
    }

    /// Pre-fill the bottom rows of the board with earth during init.
    fn prefill_bottomwell(&mut self) {
        let count = self.initial_bottomwell_fill_rows().min(self.board.len());
        for i in 0..count {
            let row = if i + 1 == count {
                Self::generate_grass_surface_row(self.background_seed)
            } else {
                self.generate_earth_row_for_depth(self.earth_depth)
            };
            self.board[i] = row;
            self.board_owner[i] = vec![None; BOARD_WIDTH];
            self.earth_depth += 1;
        }
    }

    /// Generate the initial top bottomwell row as grass with one hole.
    fn generate_grass_surface_row(seed: u64) -> Vec<u8> {
        let mixed = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut rng = Rng::new(if mixed == 0 { 1 } else { mixed });
        let mut row = vec![CELL_GRASS; BOARD_WIDTH];
        let hole_pos = rng.next_u32() as usize % BOARD_WIDTH;
        row[hole_pos] = CELL_EMPTY;
        row
    }

    /// After clearing `n` lines, reveal `n` new earth rows from below
    /// and keep board height constant at the effective runtime height.
    fn reveal_earth_lines(&mut self, n: usize) -> usize {
        let mut revealed = 0usize;
        for _ in 0..n {
            if self.try_activate_wall_for_next_depth() {
                break;
            }
            let row = self.generate_earth_row_for_depth(self.earth_depth);
            self.earth_depth += 1;
            // Insert at the bottom of the board.
            self.board.insert(0, row);
            self.board_owner.insert(0, vec![None; BOARD_WIDTH]);
            // Remove from the top to keep height constant.
            if self.board.len() > self.effective_board_height() {
                self.board.pop();
                self.board_owner.pop();
            }
            revealed += 1;
        }
        revealed
    }

    fn active_wall_def(&self) -> Option<&DepthWallDef> {
        let id = self.active_wall_id.as_deref()?;
        self.depth_walls.iter().find(|def| def.id == id)
    }

    fn pending_wall_for_depth(&self, depth: u64) -> Option<&DepthWallDef> {
        self.depth_walls.iter().find(|def| {
            def.depth_trigger <= depth
                && !self.broken_walls.contains(&def.id)
                && self.active_wall_id.as_deref() != Some(def.id.as_str())
        })
    }

    fn try_activate_wall_for_next_depth(&mut self) -> bool {
        if self.depth_progress_paused || self.active_wall_id.is_some() {
            return false;
        }

        let trigger_depth = self.earth_depth.saturating_add(1);
        let Some(def) = self.pending_wall_for_depth(trigger_depth).cloned() else {
            return false;
        };

        self.active_wall_id = Some(def.id);
        self.active_wall_hp_remaining = def.hp.max(1);
        self.depth_progress_paused = true;
        true
    }

    fn wall_damage_for_lines(&self, lines_cleared: usize) -> u32 {
        if lines_cleared == 0 {
            return 0;
        }
        let lines = lines_cleared as u32;
        let base = self.depth_wall_damage_per_line.saturating_mul(lines);
        if lines >= 2 {
            base.saturating_mul(self.depth_wall_multi_clear_bonus_percent) / 100
        } else {
            base
        }
    }

    fn apply_active_wall_damage(&mut self, lines_cleared: usize) {
        if !self.depth_progress_paused || self.active_wall_id.is_none() || lines_cleared == 0 {
            return;
        }
        let damage = self.wall_damage_for_lines(lines_cleared);
        if damage == 0 {
            return;
        }

        self.active_wall_hp_remaining = self.active_wall_hp_remaining.saturating_sub(damage);
        if self.active_wall_hp_remaining > 0 {
            return;
        }

        let Some(wall_id) = self.active_wall_id.take() else {
            return;
        };
        self.active_wall_hp_remaining = 0;
        self.depth_progress_paused = false;
        self.broken_walls.insert(wall_id);
        let _ = self.save_depth_wall_progress();
    }

    /// Ensure the bottomwell floor is maintained — bottom `bottomwell_rows`
    /// rows should be non-empty earth. If any are all-empty (shouldn't happen
    /// normally), regenerate them.
    fn ensure_bottomwell_floor(&mut self) {
        let count = self.initial_bottomwell_fill_rows().min(self.board.len());
        for y in 0..count {
            let all_empty = self.board[y].iter().all(|&c| c == CELL_EMPTY);
            if all_empty {
                self.board[y] = self.generate_earth_row_for_depth(self.earth_depth);
                self.board_owner[y] = vec![None; BOARD_WIDTH];
                self.earth_depth += 1;
            }
        }
    }

    /// Count ore and coin cells in a set of rows (used for reward tracking).
    fn count_rewards_in_rows(rows: &[Vec<u8>]) -> (u32, u32) {
        let mut ore = 0u32;
        let mut coins = 0u32;
        for row in rows {
            for &cell in row {
                if cell == CELL_ORE {
                    ore += 1;
                } else if cell == CELL_COIN {
                    coins += 1;
                }
            }
        }
        (ore, coins)
    }

    fn count_bottomwell_clears(rows: &[Vec<u8>]) -> usize {
        rows.iter()
            .filter(|row| row.iter().any(|&cell| Self::is_bottomwell_cell(cell)))
            .count()
    }

    fn is_bottomwell_cell(cell: u8) -> bool {
        matches!(
            cell,
            CELL_DIRT | CELL_GARBAGE | CELL_STONE | CELL_ORE | CELL_COIN | CELL_GRASS
        )
    }

    fn reload_depth_wall_progress(&mut self) {
        let progress = load_depth_wall_progress(&self.depth_wall_progress_path).unwrap_or_default();
        self.broken_walls = progress.broken_walls;
    }

    fn save_depth_wall_progress(&self) -> std::io::Result<()> {
        let progress = DepthWallProgress {
            version: 1,
            broken_walls: self.broken_walls.clone(),
        };
        save_depth_wall_progress(&self.depth_wall_progress_path, &progress)
    }

    fn place_piece(&mut self) {
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return,
        };
        let piece_id = self.next_piece_id;
        self.next_piece_id = self.next_piece_id.saturating_add(1);
        self.placed_piece_kind.insert(piece_id, piece);
        let grid = piece_grid(piece, self.current_piece_rotation);
        let offset = piece_board_offset(piece);
        let piece_type = piece_type(piece);

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }

                let board_x = self.current_piece_pos.x + gx as i32 - offset;
                let board_y = self.current_piece_pos.y - gy as i32 + offset;

                if board_x >= 0
                    && board_x < BOARD_WIDTH as i32
                    && board_y >= 0
                    && board_y < self.board.len() as i32
                {
                    self.board[board_y as usize][board_x as usize] = piece_type;
                    self.board_owner[board_y as usize][board_x as usize] = Some(piece_id);
                }
            }
        }
    }

    fn clear_lock_delay_state(&mut self) {
        self.grounded_lock_ms = 0;
        self.grounded_total_lock_ms = 0;
        self.grounded_for_lock = false;
    }

    fn handle_successful_adjustment(&mut self, piece_changed_location: bool) {
        if !piece_changed_location {
            return;
        }

        if self.grounded_for_lock && self.is_active_piece_grounded() {
            self.grounded_lock_ms = 0;
            self.grounded_for_lock = false;
            return;
        }

        self.clear_lock_delay_state();
    }

    fn is_active_piece_grounded(&self) -> bool {
        self.current_piece.is_some()
            && !self.is_valid_position(
                self.current_piece_pos + Vec2i::new(0, -1),
                self.current_piece_rotation,
            )
    }

    fn occupied_cells(piece: Piece, pos: Vec2i, rotation: u8) -> Vec<(i32, i32)> {
        let grid = piece_grid(piece, rotation);
        let offset = piece_board_offset(piece);
        let mut cells = Vec::new();

        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }

                cells.push((pos.x + gx as i32 - offset, pos.y - gy as i32 + offset));
            }
        }

        cells.sort_unstable();
        cells
    }

    fn piece_tile_role(piece: Piece, rotation: u8, gx: usize, gy: usize) -> PieceTileRole {
        match piece {
            Piece::I => {
                let grid = piece_grid(piece, rotation);
                if grid.cell(gx, gy) != 1 {
                    return PieceTileRole::Body;
                }
                let tip = if rotation % 2 == 0 {
                    (0..grid.size())
                        .rev()
                        .find(|&x| grid.cell(x, gy) == 1)
                        .map(|x| (x, gy))
                } else {
                    (0..grid.size())
                        .rev()
                        .find(|&y| grid.cell(gx, y) == 1)
                        .map(|y| (gx, y))
                };
                if tip == Some((gx, gy)) {
                    PieceTileRole::Tip
                } else {
                    PieceTileRole::Body
                }
            }
            _ => PieceTileRole::Body,
        }
    }

    fn tip_grid_cell(piece: Piece, rotation: u8) -> Option<(usize, usize)> {
        let grid = piece_grid(piece, rotation);
        for gy in 0..grid.size() {
            for gx in 0..grid.size() {
                if grid.cell(gx, gy) != 1 {
                    continue;
                }
                if Self::piece_tile_role(piece, rotation, gx, gy) == PieceTileRole::Tip {
                    return Some((gx, gy));
                }
            }
        }
        None
    }

    fn lock_active_piece(&mut self) {
        self.place_piece();
        self.clear_lock_delay_state();
        if !self.start_line_clear_phase_if_needed() {
            self.spawn_new_piece();
            return;
        }
        self.current_piece = None;
    }

    fn advance_line_clear_phase(&mut self, dt_ms: u32) -> bool {
        let (rows_to_clear, should_commit_now) = match &mut self.line_clear_phase {
            LineClearPhase::Idle => return false,
            LineClearPhase::Delay {
                rows,
                elapsed_ms,
                duration_ms,
            } => {
                let commit_now = if *duration_ms == 0 {
                    true
                } else {
                    *elapsed_ms = elapsed_ms.saturating_add(dt_ms);
                    *elapsed_ms >= *duration_ms
                };
                (rows.clone(), commit_now)
            }
        };

        if !should_commit_now {
            return false;
        }

        self.line_clear_phase = LineClearPhase::Idle;
        self.clear_specific_lines(rows_to_clear);
        if self.current_piece.is_none() {
            self.spawn_new_piece();
        }
        true
    }

    fn start_line_clear_phase_if_needed(&mut self) -> bool {
        let lines_to_clear = self.detect_full_rows();
        if lines_to_clear.is_empty() {
            return false;
        }
        self.line_clear_phase = LineClearPhase::Delay {
            rows: lines_to_clear,
            elapsed_ms: 0,
            duration_ms: self.line_clear_delay_ms,
        };
        true
    }

    fn detect_full_rows(&self) -> Vec<usize> {
        let mut lines_to_clear = Vec::new();
        for y in 0..self.board.len() {
            if self.board[y].iter().all(|&cell| cell != 0) {
                lines_to_clear.push(y);
            }
        }
        lines_to_clear
    }

    fn clear_specific_lines(&mut self, mut lines_to_clear: Vec<usize>) -> usize {
        if lines_to_clear.is_empty() {
            return 0;
        }

        // Collect cleared rows for reward counting before removing them.
        let cleared_rows: Vec<Vec<u8>> = lines_to_clear
            .iter()
            .map(|&y| self.board[y].clone())
            .collect();

        lines_to_clear.sort_unstable_by(|a, b| b.cmp(a));
        for line_y in &lines_to_clear {
            self.board.remove(*line_y);
            self.board_owner.remove(*line_y);
            self.board.push(vec![0; BOARD_WIDTH]);
            self.board_owner.push(vec![None; BOARD_WIDTH]);
        }
        self.cleanup_piece_owners();

        let cleared = lines_to_clear.len() as u32;
        self.lines_cleared = self.lines_cleared.saturating_add(cleared);
        self.score = self.score.saturating_add(line_clear_points(cleared));

        // Bottomwell: collect rewards from cleared rows, then only advance
        // depth for clears that actually include bottomwell earth cells.
        if self.bottomwell_enabled {
            let (ore, coins) = Self::count_rewards_in_rows(&cleared_rows);
            let bottomwell_clears = Self::count_bottomwell_clears(&cleared_rows);
            self.ore_collected = self.ore_collected.saturating_add(ore);
            self.coins_collected = self.coins_collected.saturating_add(coins);

            let reward_score = ore
                .saturating_mul(self.ore_score_value)
                .saturating_add(coins.saturating_mul(self.coin_score_value));
            self.score = self.score.saturating_add(reward_score);

            if self.depth_progress_paused {
                self.apply_active_wall_damage(lines_to_clear.len());
            } else {
                let revealed = self.reveal_earth_lines(bottomwell_clears);
                if revealed > 0 {
                    self.ensure_bottomwell_floor();
                }
            }
        }

        lines_to_clear.len()
    }

    fn try_rotation_with_kicks(&mut self, new_rotation: u8) -> bool {
        if self.current_piece.is_none() {
            return false;
        }

        for (dx, dy) in &GENERIC_KICK_OFFSETS {
            let test_pos = self.current_piece_pos + Vec2i::new(*dx, *dy);
            if self.is_valid_position(test_pos, new_rotation) {
                self.last_kick_offset = Vec2i::new(*dx, *dy);
                self.current_piece_pos = test_pos;
                return true;
            }
        }

        self.last_kick_offset = Vec2i::ZERO;
        false
    }
}

fn line_clear_points(lines: u32) -> u32 {
    // Minimal, deterministic scoring:
    // - 1/2/3/4 line clears: 100/300/500/800
    // - For >4 (only possible via tests / manual board edits), treat as multiple tetrises + remainder.
    let tetrises = lines / 4;
    let rem = lines % 4;

    let base = tetrises.saturating_mul(800);
    let rem_points = match rem {
        0 => 0,
        1 => 100,
        2 => 300,
        3 => 500,
        _ => 0,
    };

    base.saturating_add(rem_points)
}

fn load_depth_wall_progress(path: &Path) -> Result<DepthWallProgress, std::io::Error> {
    let bytes = fs::read(path)?;
    let mut progress: DepthWallProgress = serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if progress.version == 0 {
        progress.version = 1;
    }
    Ok(progress)
}

fn save_depth_wall_progress(path: &Path, progress: &DepthWallProgress) -> std::io::Result<()> {
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
            fs::copy(&tmp, path)?;
            let _ = fs::remove_file(&tmp);
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        let seed = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        Self { state: seed }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

}

const GENERIC_KICK_OFFSETS: [(i32, i32); 7] =
    [(0, 0), (-1, 0), (1, 0), (0, 1), (-2, 0), (2, 0), (0, 2)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PieceGrid {
    size: usize,
    cells: [u8; 16],
}

impl PieceGrid {
    pub(crate) fn size(&self) -> usize {
        self.size
    }

    pub(crate) fn cell(&self, x: usize, y: usize) -> u8 {
        debug_assert!(x < self.size && y < self.size);
        self.cells[y * self.size + x]
    }
}

pub(crate) const fn piece_board_offset(piece: Piece) -> i32 {
    match piece {
        Piece::O | Piece::S | Piece::MossSeed => 0,
        Piece::I | Piece::Glass | Piece::T | Piece::Z | Piece::J | Piece::L => 1,
    }
}

fn piece_grid_size(piece: Piece) -> usize {
    match piece {
        Piece::MossSeed => 1,
        Piece::I => 4,
        Piece::O => 2,
        Piece::S => 2,
        Piece::Glass | Piece::T | Piece::Z | Piece::J | Piece::L => 3,
    }
}

pub(crate) fn piece_grid(piece: Piece, rotation: u8) -> PieceGrid {
    let mut grid = base_piece_grid(piece);
    let steps = rotation % piece_rotation_states(piece);
    for _ in 0..steps {
        grid = rotate_grid_90(&grid);
    }
    grid
}

const fn piece_rotation_states(piece: Piece) -> u8 {
    match piece {
        Piece::O | Piece::MossSeed => 1,
        Piece::I | Piece::Glass | Piece::S => 2,
        Piece::T | Piece::Z | Piece::J | Piece::L => 4,
    }
}

fn rotate_grid_90(grid: &PieceGrid) -> PieceGrid {
    let size = grid.size;
    let mut rotated = PieceGrid {
        size,
        cells: [0u8; 16],
    };

    for y in 0..size {
        for x in 0..size {
            // Rotate clockwise: rotated[x][size-1-y] = grid[y][x]
            let src = grid.cells[y * size + x];
            let dst_row = x;
            let dst_col = size - 1 - y;
            rotated.cells[dst_row * size + dst_col] = src;
        }
    }

    rotated
}

fn base_piece_grid(piece: Piece) -> PieceGrid {
    let size = piece_grid_size(piece);
    match piece {
        Piece::MossSeed => PieceGrid {
            size,
            cells: [
                1, //
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::I => PieceGrid {
            size,
            cells: [
                0, 0, 0, 0, //
                1, 1, 1, 1, //
                0, 0, 0, 0, //
                0, 0, 0, 0, //
            ],
        },
        Piece::Glass => PieceGrid {
            size,
            cells: [
                0, 0, 0, //
                1, 1, 1, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::O => PieceGrid {
            size,
            cells: [
                1, 1, //
                1, 1, //
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::T => PieceGrid {
            size,
            cells: [
                0, 1, 0, //
                1, 1, 1, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::S => PieceGrid {
            size,
            cells: [
                1, 1, //
                0, 0, //
                0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::Z => PieceGrid {
            size,
            cells: [
                1, 1, 0, //
                0, 1, 1, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::J => PieceGrid {
            size,
            cells: [
                1, 0, 0, //
                1, 1, 1, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        },
        Piece::L => PieceGrid {
            size,
            cells: [
                0, 0, 1, //
                1, 1, 1, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
            ],
        },
    }
}

pub(crate) const fn piece_type(piece: Piece) -> u8 {
    match piece {
        Piece::I => 1,
        Piece::O => 2,
        Piece::Glass => CELL_GLASS,
        Piece::T => CELL_SAND,
        Piece::S => CELL_DIRT,
        Piece::MossSeed => CELL_MOSS_SEED,
        Piece::Z => 5,
        Piece::J => 6,
        Piece::L => 7,
    }
}

#[cfg(test)]
mod piece_grid_tests {
    use super::*;

    #[test]
    fn o_piece_grid_is_invariant_under_rotation() {
        for rot in 0..4 {
            let g = piece_grid(Piece::O, rot);
            assert_eq!(g.size(), 2);
            for y in 0..g.size() {
                for x in 0..g.size() {
                    assert_eq!(g.cell(x, y), 1);
                }
            }
        }
    }

    #[test]
    fn i_piece_rotation_1_is_vertical_in_column_2() {
        let g = piece_grid(Piece::I, 1);
        assert_eq!(g.size(), 4);

        for y in 0..g.size() {
            for x in 0..g.size() {
                let expected = if x == 2 { 1 } else { 0 };
                assert_eq!(g.cell(x, y), expected, "unexpected cell at x={x} y={y}");
            }
        }
    }

    #[test]
    fn piece_board_offset_matches_piece_sizes() {
        assert_eq!(piece_board_offset(Piece::O), 0);
        assert_eq!(piece_board_offset(Piece::S), 0);
        assert_eq!(piece_board_offset(Piece::MossSeed), 0);
        for p in [
            Piece::I,
            Piece::Glass,
            Piece::T,
            Piece::Z,
            Piece::J,
            Piece::L,
        ] {
            assert_eq!(piece_board_offset(p), 1);
        }
    }

    #[test]
    fn moss_seed_piece_is_single_cell_invariant_under_rotation() {
        for rot in 0..4 {
            let g = piece_grid(Piece::MossSeed, rot);
            assert_eq!(g.size(), 1);
            assert_eq!(g.cell(0, 0), 1);
        }
    }
}

#[cfg(test)]
mod bottomwell_tests {
    use super::*;
    use std::{
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn make_bottomwell_core(seed: u64) -> TetrisCore {
        let mut core = TetrisCore::new(seed);
        core.set_available_pieces(Piece::all());
        core.set_bottomwell_enabled(true);
        core.initialize_game();
        core
    }

    fn default_bottomwell_height() -> usize {
        BOARD_HEIGHT + DEFAULT_BOTTOMWELL_ROWS
    }

    #[test]
    fn default_first_depth_wall_triggers_at_5() {
        let defs = default_depth_wall_defs();
        assert_eq!(defs.first().map(|def| def.depth_trigger), Some(5));
    }

    fn fill_clearable_row(core: &mut TetrisCore) {
        let target_y = 0;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, CELL_GARBAGE);
        }
    }

    fn fill_clearable_non_bottomwell_row(core: &mut TetrisCore) {
        let target_y = DEFAULT_BOTTOMWELL_ROWS;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, 1);
        }
    }

    fn unique_progress_path(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "sycho_depth_wall_{test_name}_{}_{}.json",
            process::id(),
            nanos
        ))
    }

    fn make_wall_test_core(progress_path: PathBuf, hp: u32) -> TetrisCore {
        let mut core = TetrisCore::new(7);
        core.set_available_pieces(Piece::all());
        core.set_bottomwell_enabled(true);
        core.set_depth_wall_progress_path(progress_path);
        core.set_depth_wall_defs(vec![DepthWallDef {
            id: "test_wall".to_string(),
            depth_trigger: (DEFAULT_BOTTOMWELL_ROWS as u64).saturating_add(1),
            hp,
            biome_from: "dirt".to_string(),
            biome_to: "stone".to_string(),
        }]);
        core.set_depth_wall_damage_tuning(4, 100);
        core.initialize_game();
        core
    }

    #[test]
    fn bottom_3_rows_exist_after_init() {
        let core = make_bottomwell_core(42);
        assert_eq!(core.board.len(), default_bottomwell_height());

        // Bottom 3 rows should have at least some non-empty cells.
        for y in 0..DEFAULT_BOTTOMWELL_ROWS {
            let non_empty = core.board[y].iter().filter(|&&c| c != CELL_EMPTY).count();
            assert!(
                non_empty > 0,
                "bottomwell row y={y} should have non-empty cells, got all empty"
            );
        }
    }

    #[test]
    fn bottom_rows_have_single_hole_for_playability() {
        let core = make_bottomwell_core(42);

        for y in 0..DEFAULT_BOTTOMWELL_ROWS {
            let holes = core.board[y].iter().filter(|&&c| c == CELL_EMPTY).count();
            assert_eq!(
                holes, 1,
                "bottomwell row y={y} should have 1 hole, got {holes}"
            );
        }
    }

    #[test]
    fn top_bottomwell_row_is_grass_with_single_hole() {
        let core = make_bottomwell_core(42);
        let top = DEFAULT_BOTTOMWELL_ROWS - 1;
        let row = &core.board[top];
        let holes = row.iter().filter(|&&c| c == CELL_EMPTY).count();
        assert_eq!(holes, 1, "top bottomwell row should keep exactly one hole");
        for &cell in row {
            assert!(
                cell == CELL_EMPTY || cell == CELL_GRASS,
                "top bottomwell row should contain only grass + one hole"
            );
        }
    }

    #[test]
    fn earth_depth_equals_bottomwell_rows_after_init() {
        let core = make_bottomwell_core(42);
        assert_eq!(core.earth_depth, DEFAULT_BOTTOMWELL_ROWS as u64);
    }

    #[test]
    fn board_height_constant_after_init() {
        let core = make_bottomwell_core(42);
        assert_eq!(core.board.len(), default_bottomwell_height());
    }

    #[test]
    fn generation_is_deterministic_by_seed_depth() {
        let row_a = TetrisCore::generate_earth_row(42, 0);
        let row_b = TetrisCore::generate_earth_row(42, 0);
        assert_eq!(row_a, row_b, "same seed+depth must produce identical rows");

        let row_c = TetrisCore::generate_earth_row(42, 1);
        assert_ne!(
            row_a, row_c,
            "different depths should (usually) produce different rows"
        );

        let row_d = TetrisCore::generate_earth_row(99, 0);
        assert_ne!(
            row_a, row_d,
            "different seeds should (usually) produce different rows"
        );
    }

    #[test]
    fn clearing_1_line_reveals_1_earth_line() {
        let mut core = make_bottomwell_core(42);
        let depth_before = core.earth_depth;

        // Clearing an earth row should reveal exactly one new earth row.
        let target_y = 0;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, CELL_GARBAGE);
        }

        let cleared = core.clear_lines();
        assert_eq!(cleared, 1, "should clear exactly 1 line");
        assert_eq!(
            core.earth_depth,
            depth_before + 1,
            "earth_depth should advance by 1"
        );
        assert_eq!(
            core.board.len(),
            default_bottomwell_height(),
            "board height must stay constant"
        );
    }

    #[test]
    fn clearing_the_grass_row_removes_grass_property() {
        let mut core = make_bottomwell_core(42);
        let top = DEFAULT_BOTTOMWELL_ROWS - 1;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, top, CELL_GRASS);
        }

        let cleared = core.clear_lines();
        assert_eq!(cleared, 1, "expected the full grass row to clear");
        let grass_cells = core
            .board
            .iter()
            .flat_map(|row| row.iter())
            .filter(|&&cell| cell == CELL_GRASS)
            .count();
        assert_eq!(
            grass_cells, 0,
            "cleared grass row should not be regenerated"
        );
    }

    #[test]
    fn clearing_non_bottomwell_line_does_not_reveal_earth_line() {
        let mut core = make_bottomwell_core(42);
        let depth_before = core.earth_depth;

        let target_y = DEFAULT_BOTTOMWELL_ROWS;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, 1);
        }

        let cleared = core.clear_lines();
        assert_eq!(cleared, 1, "should clear exactly 1 line");
        assert_eq!(
            core.earth_depth, depth_before,
            "non-bottomwell clears should not reveal new earth rows"
        );
    }

    #[test]
    fn clearing_n_lines_reveals_n_earth_lines() {
        let mut core = make_bottomwell_core(42);
        let depth_before = core.earth_depth;

        // Fill 2 bottomwell rows so both clears count as digging clears.
        for n in 0..2 {
            let target_y = n;
            for x in 0..BOARD_WIDTH {
                core.set_cell(x, target_y, CELL_GARBAGE);
            }
        }

        let cleared = core.clear_lines();
        assert_eq!(cleared, 2, "should clear 2 lines");
        assert_eq!(
            core.earth_depth,
            depth_before + 2,
            "earth_depth should advance by 2"
        );
        assert_eq!(
            core.board.len(),
            default_bottomwell_height(),
            "board height must stay constant"
        );
    }

    #[test]
    fn board_height_always_stays_constant_after_clears() {
        let mut core = make_bottomwell_core(123);

        // Simulate several clears.
        for round in 0..5 {
            let target_y = DEFAULT_BOTTOMWELL_ROWS;
            for x in 0..BOARD_WIDTH {
                core.set_cell(x, target_y, 1);
            }
            let cleared = core.clear_lines();
            assert!(cleared >= 1, "round {round}: should clear at least 1 line");
            assert_eq!(
                core.board.len(),
                default_bottomwell_height(),
                "round {round}: board height must remain {}",
                default_bottomwell_height()
            );
        }
    }

    #[test]
    fn deep_shaft_rows_expand_effective_board_height() {
        let mut core = TetrisCore::new(42);
        core.set_available_pieces(Piece::all());
        core.set_bottomwell_enabled(true);
        core.set_bottomwell_run_mods(BottomwellRunMods {
            deep_shaft_rows: 2,
            ..BottomwellRunMods::default()
        });
        core.initialize_game();
        assert_eq!(
            core.board.len(),
            BOARD_HEIGHT + DEFAULT_BOTTOMWELL_ROWS + 2,
            "deep shaft rows should extend runtime board height"
        );
        assert_eq!(
            core.earth_depth,
            (DEFAULT_BOTTOMWELL_ROWS + 2) as u64,
            "deep shaft visibility should also prefill matching bottomwell rows"
        );
    }

    #[test]
    fn bottomwell_disabled_does_not_prefill() {
        let mut core = TetrisCore::new(42);
        core.set_available_pieces(Piece::all());
        // bottomwell_enabled is false by default.
        core.initialize_game();

        // All rows should be empty (except active piece doesn't write to board until locked).
        for y in 0..BOARD_HEIGHT {
            let non_empty = core.board[y].iter().filter(|&&c| c != CELL_EMPTY).count();
            assert_eq!(
                non_empty, 0,
                "row y={y} should be empty when bottomwell is disabled"
            );
        }
        assert_eq!(core.earth_depth, 0);
    }

    #[test]
    fn ore_and_coin_rewards_tracked_on_clear() {
        let mut core = make_bottomwell_core(42);

        // Manually place ore and coin cells in a clearable row.
        let target_y = DEFAULT_BOTTOMWELL_ROWS;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, 1);
        }
        // Put some ore/coin in that row.
        core.set_cell(0, target_y, CELL_ORE);
        core.set_cell(1, target_y, CELL_COIN);

        let ore_before = core.ore_collected;
        let coins_before = core.coins_collected;

        let cleared = core.clear_lines();
        assert_eq!(cleared, 1);
        assert_eq!(core.ore_collected, ore_before + 1);
        assert_eq!(core.coins_collected, coins_before + 1);
    }

    #[test]
    fn refinery_bonus_increases_score_and_meta_money() {
        let mut core = make_bottomwell_core(42);
        core.set_bottomwell_run_mods(BottomwellRunMods {
            ore_score_bonus: 25,
            coin_score_bonus: 60,
            ore_money_bonus: 1,
            coin_money_bonus: 3,
            ..BottomwellRunMods::default()
        });

        let target_y = DEFAULT_BOTTOMWELL_ROWS;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, 1);
        }
        core.set_cell(0, target_y, CELL_ORE);
        core.set_cell(1, target_y, CELL_COIN);

        let score_before = core.score();
        assert_eq!(core.clear_lines(), 1);
        let score_gain = core.score().saturating_sub(score_before);
        assert_eq!(score_gain, 100 + 75 + 260);
        assert_eq!(core.refinery_money_from_collected_resources(), 4);
    }

    #[test]
    fn weighting_mods_increase_ore_and_coin_generation() {
        let mut base_ore = 0u32;
        let mut base_coin = 0u32;
        let mut mod_ore = 0u32;
        let mut mod_coin = 0u32;
        for depth in 20..120 {
            let base = TetrisCore::generate_earth_row_with_tuning(42, depth, 0, 0, 0, 0);
            let tuned = TetrisCore::generate_earth_row_with_tuning(42, depth, 6, 3, 0, 0);
            for cell in base {
                if cell == CELL_ORE {
                    base_ore += 1;
                } else if cell == CELL_COIN {
                    base_coin += 1;
                }
            }
            for cell in tuned {
                if cell == CELL_ORE {
                    mod_ore += 1;
                } else if cell == CELL_COIN {
                    mod_coin += 1;
                }
            }
        }
        assert!(mod_ore > base_ore, "ore weighting should increase ore frequency");
        assert!(mod_coin > base_coin, "coin weighting should increase coin frequency");
    }

    #[test]
    fn generate_earth_row_has_correct_width() {
        let row = TetrisCore::generate_earth_row(42, 0);
        assert_eq!(row.len(), BOARD_WIDTH);
    }

    #[test]
    fn generate_earth_row_contains_only_valid_cell_types() {
        for depth in 0..50 {
            let row = TetrisCore::generate_earth_row(42, depth);
            for &cell in &row {
                assert!(
                    cell == CELL_EMPTY
                        || cell == CELL_DIRT
                        || cell == CELL_GARBAGE
                        || cell == CELL_STONE
                        || cell == CELL_ORE
                        || cell == CELL_COIN,
                    "unexpected cell type {cell} at depth {depth}"
                );
            }
        }
    }

    #[test]
    fn generate_earth_row_does_not_emit_grass_cells() {
        for depth in 0..50 {
            let row = TetrisCore::generate_earth_row(42, depth);
            assert!(
                row.iter().all(|&cell| cell != CELL_GRASS),
                "earth generation should not include grass at depth {depth}"
            );
        }
    }

    #[test]
    fn wall_activates_at_trigger_depth_and_pauses_reveal() {
        let progress_path = unique_progress_path("activation");
        let mut core = make_wall_test_core(progress_path.clone(), 8);
        let depth_before = core.earth_depth();

        fill_clearable_row(&mut core);
        assert_eq!(core.clear_lines(), 1);
        assert_eq!(core.active_wall_id(), Some("test_wall"));
        assert_eq!(core.active_wall_hp_remaining(), 8);
        assert!(core.depth_progress_paused());
        assert_eq!(
            core.earth_depth(),
            depth_before,
            "depth should stop advancing as soon as the wall activates"
        );

        let _ = std::fs::remove_file(&progress_path);
        let _ = std::fs::remove_file(progress_path.with_extension("tmp"));
    }

    #[test]
    fn active_wall_damage_breaks_and_unpauses_depth_progression() {
        let progress_path = unique_progress_path("damage_break");
        let mut core = make_wall_test_core(progress_path.clone(), 8);

        // Activate wall using a bottomwell clear.
        fill_clearable_row(&mut core);
        assert_eq!(core.clear_lines(), 1);
        let depth_at_activation = core.earth_depth();

        // While paused, non-bottomwell clears still damage the wall.
        fill_clearable_non_bottomwell_row(&mut core);
        assert_eq!(core.clear_lines(), 1);
        assert_eq!(core.active_wall_hp_remaining(), 4);
        assert!(core.depth_progress_paused());
        assert_eq!(
            core.earth_depth(),
            depth_at_activation,
            "depth remains frozen while a wall is active"
        );

        fill_clearable_non_bottomwell_row(&mut core);
        assert_eq!(core.clear_lines(), 1);
        assert_eq!(core.active_wall_id(), None);
        assert_eq!(core.active_wall_hp_remaining(), 0);
        assert!(!core.depth_progress_paused());

        // Depth only resumes when we clear another bottomwell row.
        fill_clearable_row(&mut core);
        assert_eq!(core.clear_lines(), 1);
        assert!(
            core.earth_depth() >= depth_at_activation + 1,
            "depth progression should resume on the clear after the break"
        );

        let _ = std::fs::remove_file(&progress_path);
        let _ = std::fs::remove_file(progress_path.with_extension("tmp"));
    }

    #[test]
    fn broken_walls_persist_and_are_skipped_on_new_runs() {
        let progress_path = unique_progress_path("persist");

        {
            let mut first_run = make_wall_test_core(progress_path.clone(), 4);
            fill_clearable_row(&mut first_run);
            assert_eq!(first_run.clear_lines(), 1);
            fill_clearable_non_bottomwell_row(&mut first_run);
            assert_eq!(first_run.clear_lines(), 1);
            assert_eq!(first_run.active_wall_id(), None);
            assert!(!first_run.depth_progress_paused());
        }

        let mut second_run = make_wall_test_core(progress_path.clone(), 4);
        let depth_before = second_run.earth_depth();
        fill_clearable_row(&mut second_run);
        assert_eq!(second_run.clear_lines(), 1);
        assert_eq!(second_run.active_wall_id(), None);
        assert!(!second_run.depth_progress_paused());
        assert_eq!(
            second_run.earth_depth(),
            depth_before + 1,
            "a previously broken wall should no longer block progression"
        );

        let _ = std::fs::remove_file(&progress_path);
        let _ = std::fs::remove_file(progress_path.with_extension("tmp"));
    }
}
