use std::ops::Add;

use serde::{Deserialize, Serialize};

pub const BOARD_WIDTH: usize = 10;
pub const BOARD_HEIGHT: usize = 20;
pub const NEXT_QUEUE_LEN: usize = 5;

const HARD_DROP_POINTS_PER_ROW: u32 = 2;

// Bottomwell cell-type constants (standard piece types are 1-7).
pub const CELL_EMPTY: u8 = 0;
pub const CELL_GARBAGE: u8 = 8;
pub const CELL_STONE: u8 = 9;
pub const CELL_ORE: u8 = 10;
pub const CELL_COIN: u8 = 11;

pub const DEFAULT_BOTTOMWELL_ROWS: usize = 3;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Piece {
    I,
    O,
    T,
    S,
    Z,
    J,
    L,
}

impl Piece {
    pub const ALL: [Piece; 7] = [
        Piece::I,
        Piece::O,
        Piece::T,
        Piece::S,
        Piece::Z,
        Piece::J,
        Piece::L,
    ];

    pub fn all() -> Vec<Piece> {
        Self::ALL.to_vec()
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
    fn apply(self, rotation: u8) -> u8 {
        match self {
            RotationDir::Cw => (rotation + 1) % 4,
            RotationDir::Ccw => (rotation + 3) % 4,
            RotationDir::Half => (rotation + 2) % 4,
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TetrisCore {
    board: Vec<Vec<u8>>,
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
    last_srs_kick_offset: Vec2i,
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
}

fn default_bottomwell_rows() -> usize {
    DEFAULT_BOTTOMWELL_ROWS
}

impl TetrisCore {
    pub fn new(seed: u64) -> Self {
        Self {
            board: vec![vec![0; BOARD_WIDTH]; BOARD_HEIGHT],
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
            last_srs_kick_offset: Vec2i::ZERO,
            bottomwell_enabled: false,
            bottomwell_rows: DEFAULT_BOTTOMWELL_ROWS,
            earth_depth: 0,
            ore_collected: 0,
            coins_collected: 0,
        }
    }

    pub fn set_bottomwell_enabled(&mut self, enabled: bool) {
        self.bottomwell_enabled = enabled;
    }

    pub fn bottomwell_enabled(&self) -> bool {
        self.bottomwell_enabled
    }

    pub fn earth_depth(&self) -> u64 {
        self.earth_depth
    }

    /// Render-facing depth signal used by background/camera systems.
    ///
    /// This intentionally maps to canonical revealed-earth depth instead of
    /// gameplay line-count metrics so world-motion effects stay in sync with
    /// bottomwell reveal progression.
    pub fn background_depth_rows(&self) -> u32 {
        self.earth_depth.min(u32::MAX as u64) as u32
    }

    pub fn ore_collected(&self) -> u32 {
        self.ore_collected
    }

    pub fn coins_collected(&self) -> u32 {
        self.coins_collected
    }

    pub fn set_available_pieces(&mut self, pieces: Vec<Piece>) {
        if pieces.is_empty() {
            self.available_pieces = vec![Piece::O];
        } else {
            self.available_pieces = pieces;
        }
    }

    pub fn initialize_game(&mut self) {
        self.board = vec![vec![0; BOARD_WIDTH]; BOARD_HEIGHT];
        self.current_piece = None;
        self.next_queue.clear();
        self.held_piece = None;
        self.can_hold = true;
        self.current_piece_pos = Vec2i::new(4, BOARD_HEIGHT as i32);
        self.current_piece_rotation = 0;
        self.piece_bag.clear();
        self.lines_cleared = 0;
        self.score = 0;
        self.game_over = false;
        self.last_srs_kick_offset = Vec2i::ZERO;
        self.earth_depth = 0;
        self.ore_collected = 0;
        self.coins_collected = 0;

        if self.bottomwell_enabled {
            self.prefill_bottomwell();
        }

        self.spawn_new_piece();
    }

    pub fn board(&self) -> &[Vec<u8>] {
        &self.board
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
                    && board_y < BOARD_HEIGHT as i32
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
    }

    pub fn set_cell(&mut self, x: usize, y: usize, value: u8) {
        if y < BOARD_HEIGHT && x < BOARD_WIDTH {
            self.board[y][x] = value;
        }
    }

    pub fn draw_piece(&mut self) -> Piece {
        if self.piece_bag.is_empty() {
            self.refill_bag();
        }
        self.piece_bag.pop().unwrap_or(Piece::O)
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
        self.current_piece_pos = Vec2i::new(4, BOARD_HEIGHT as i32);
        self.current_piece_rotation = 0;
        self.fill_next_queue();
        self.can_hold = true;
        self.last_srs_kick_offset = Vec2i::ZERO;

        if !self.is_valid_position(self.current_piece_pos, self.current_piece_rotation) {
            self.game_over = true;
            return false;
        }

        true
    }

    pub fn hold_piece(&mut self) -> bool {
        if self.game_over || !self.can_hold {
            return false;
        }

        let Some(current) = self.current_piece else {
            return false;
        };

        if let Some(held) = self.held_piece {
            self.held_piece = Some(current);
            self.current_piece = Some(held);
            self.current_piece_pos = Vec2i::new(4, BOARD_HEIGHT as i32);
            self.current_piece_rotation = 0;
            self.last_srs_kick_offset = Vec2i::ZERO;
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
                if board_y < BOARD_HEIGHT as i32 {
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
        let new_pos = self.current_piece_pos + dir;
        if self.is_valid_position(new_pos, self.current_piece_rotation) {
            self.current_piece_pos = new_pos;
            return true;
        }
        false
    }

    pub fn move_piece_down(&mut self) -> bool {
        self.move_piece(Vec2i::new(0, -1))
    }

    pub fn rotate_piece(&mut self, dir: RotationDir) -> bool {
        let new_rotation = dir.apply(self.current_piece_rotation);
        if self.try_rotation_with_kicks(new_rotation) {
            self.current_piece_rotation = new_rotation;
            return true;
        }
        false
    }

    pub fn hard_drop(&mut self) -> i32 {
        let mut drop_distance = 0u32;
        while self.move_piece_down() {
            drop_distance = drop_distance.saturating_add(1);
        }

        self.score = self
            .score
            .saturating_add(drop_distance.saturating_mul(HARD_DROP_POINTS_PER_ROW));

        self.place_piece();
        self.clear_lines();
        self.spawn_new_piece();
        drop_distance as i32
    }

    pub fn clear_lines(&mut self) -> usize {
        let mut lines_to_clear = Vec::new();

        for y in 0..BOARD_HEIGHT {
            if self.board[y].iter().all(|&cell| cell != 0) {
                lines_to_clear.push(y);
            }
        }

        if !lines_to_clear.is_empty() {
            // Collect cleared rows for reward counting before removing them.
            let cleared_rows: Vec<Vec<u8>> = lines_to_clear
                .iter()
                .map(|&y| self.board[y].clone())
                .collect();

            lines_to_clear.sort_unstable_by(|a, b| b.cmp(a));
            for line_y in &lines_to_clear {
                self.board.remove(*line_y);
                self.board.push(vec![0; BOARD_WIDTH]);
            }

            let cleared = lines_to_clear.len() as u32;
            self.lines_cleared = self.lines_cleared.saturating_add(cleared);
            self.score = self.score.saturating_add(line_clear_points(cleared));

            // Bottomwell: collect rewards from cleared rows, reveal new earth.
            if self.bottomwell_enabled {
                let (ore, coins) = Self::count_rewards_in_rows(&cleared_rows);
                self.ore_collected = self.ore_collected.saturating_add(ore);
                self.coins_collected = self.coins_collected.saturating_add(coins);

                // Ore/coin reward: each ore = 50 score, each coin = 200 score.
                let reward_score = ore
                    .saturating_mul(50)
                    .saturating_add(coins.saturating_mul(200));
                self.score = self.score.saturating_add(reward_score);

                self.reveal_earth_lines(lines_to_clear.len());
                self.ensure_bottomwell_floor();
            }
        }

        lines_to_clear.len()
    }

    // ── Bottomwell helpers ──────────────────────────────────────────

    /// Generate a deterministic earth row for the given depth.
    /// Uses `background_seed` + `depth` to seed a local RNG so the
    /// sequence is reproducible across runs with the same seed.
    pub fn generate_earth_row(seed: u64, depth: u64) -> Vec<u8> {
        // Mix seed and depth into a local RNG state.
        let mixed = seed
            .wrapping_mul(0x5851_F42D_4C95_7F2D)
            .wrapping_add(depth.wrapping_mul(0x14057B7EF767814F));
        let mut rng = Rng::new(if mixed == 0 { 1 } else { mixed });

        let mut row = vec![CELL_GARBAGE; BOARD_WIDTH];

        // Determine how many holes (1-2) for playability.
        let hole_count = 1 + (rng.next_u32() as usize % 2); // 1 or 2 holes
        let mut holes_placed = 0;
        // Pick hole positions (non-repeating).
        let mut hole_positions = Vec::new();
        while holes_placed < hole_count {
            let pos = rng.next_u32() as usize % BOARD_WIDTH;
            if !hole_positions.contains(&pos) {
                hole_positions.push(pos);
                holes_placed += 1;
            }
        }
        for &pos in &hole_positions {
            row[pos] = CELL_EMPTY;
        }

        // Fill non-hole cells with a mix of materials.
        for x in 0..BOARD_WIDTH {
            if row[x] == CELL_EMPTY {
                continue;
            }
            let roll = rng.next_u32() % 100;
            // Deeper depths have more ore/coin chance.
            let ore_threshold = if depth > 20 {
                15
            } else if depth > 10 {
                10
            } else {
                5
            };
            let coin_threshold = if depth > 30 {
                8
            } else if depth > 15 {
                4
            } else {
                2
            };

            if roll < coin_threshold {
                row[x] = CELL_COIN;
            } else if roll < coin_threshold + ore_threshold {
                row[x] = CELL_ORE;
            } else if roll < coin_threshold + ore_threshold + 30 {
                row[x] = CELL_STONE;
            }
            // else stays CELL_GARBAGE (most common)
        }

        row
    }

    /// Pre-fill the bottom rows of the board with earth during init.
    fn prefill_bottomwell(&mut self) {
        let count = self.bottomwell_rows.min(BOARD_HEIGHT);
        for i in 0..count {
            let row = Self::generate_earth_row(self.background_seed, self.earth_depth);
            self.board[i] = row;
            self.earth_depth += 1;
        }
    }

    /// After clearing `n` lines, reveal `n` new earth rows from below
    /// and keep board height constant at BOARD_HEIGHT.
    fn reveal_earth_lines(&mut self, n: usize) {
        for _ in 0..n {
            let row = Self::generate_earth_row(self.background_seed, self.earth_depth);
            self.earth_depth += 1;
            // Insert at the bottom of the board.
            self.board.insert(0, row);
            // Remove from the top to keep height constant.
            if self.board.len() > BOARD_HEIGHT {
                self.board.pop();
            }
        }
    }

    /// Ensure the bottomwell floor is maintained — bottom `bottomwell_rows`
    /// rows should be non-empty earth. If any are all-empty (shouldn't happen
    /// normally), regenerate them.
    fn ensure_bottomwell_floor(&mut self) {
        let count = self.bottomwell_rows.min(BOARD_HEIGHT);
        for y in 0..count {
            let all_empty = self.board[y].iter().all(|&c| c == CELL_EMPTY);
            if all_empty {
                self.board[y] = Self::generate_earth_row(self.background_seed, self.earth_depth);
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

    fn place_piece(&mut self) {
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return,
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
                    && board_y < BOARD_HEIGHT as i32
                {
                    self.board[board_y as usize][board_x as usize] = piece_type;
                }
            }
        }
    }

    fn try_rotation_with_kicks(&mut self, new_rotation: u8) -> bool {
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return false,
        };

        let kicks = if piece == Piece::I {
            srs_i_kicks(self.current_piece_rotation, new_rotation)
        } else {
            srs_kicks(self.current_piece_rotation, new_rotation)
        };

        if let Some(kicks) = kicks {
            for (dx, dy) in kicks {
                let test_pos = self.current_piece_pos + Vec2i::new(*dx, *dy);
                if self.is_valid_position(test_pos, new_rotation) {
                    self.last_srs_kick_offset = Vec2i::new(*dx, *dy);
                    self.current_piece_pos = test_pos;
                    return true;
                }
            }
            return false;
        }

        self.last_srs_kick_offset = Vec2i::ZERO;
        self.is_valid_position(self.current_piece_pos, new_rotation)
    }

    fn refill_bag(&mut self) {
        if self.available_pieces.is_empty() {
            self.available_pieces = vec![Piece::O];
        }

        self.piece_bag = self.available_pieces.clone();
        if self.piece_bag.len() <= 1 {
            return;
        }

        for i in (1..self.piece_bag.len()).rev() {
            let j = self.rng.gen_range_inclusive(i);
            self.piece_bag.swap(i, j);
        }
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

    fn gen_range_inclusive(&mut self, upper: usize) -> usize {
        if upper == 0 {
            return 0;
        }
        (self.next_u32() as usize) % (upper + 1)
    }
}

const KICKS_0_1: [(i32, i32); 5] = [(0, 0), (-1, 0), (-1, 1), (0, -2), (-1, -2)];
const KICKS_1_0: [(i32, i32); 5] = [(0, 0), (1, 0), (1, -1), (0, 2), (1, 2)];
const KICKS_1_2: [(i32, i32); 5] = [(0, 0), (1, 0), (1, -1), (0, 2), (1, 2)];
const KICKS_2_1: [(i32, i32); 5] = [(0, 0), (-1, 0), (-1, 1), (0, -2), (-1, -2)];
const KICKS_2_3: [(i32, i32); 5] = [(0, 0), (1, 0), (1, 1), (0, -2), (1, -2)];
const KICKS_3_2: [(i32, i32); 5] = [(0, 0), (-1, 0), (-1, -1), (0, 2), (-1, 2)];
const KICKS_3_0: [(i32, i32); 5] = [(0, 0), (-1, 0), (-1, -1), (0, 2), (-1, 2)];
const KICKS_0_3: [(i32, i32); 5] = [(0, 0), (1, 0), (1, 1), (0, -2), (1, -2)];

const I_KICKS_0_1: [(i32, i32); 5] = [(0, 0), (-2, 0), (1, 0), (-2, -1), (1, 2)];
const I_KICKS_1_0: [(i32, i32); 5] = [(0, 0), (2, 0), (-1, 0), (2, 1), (-1, -2)];
const I_KICKS_1_2: [(i32, i32); 5] = [(0, 0), (-1, 0), (2, 0), (-1, 2), (2, -1)];
const I_KICKS_2_1: [(i32, i32); 5] = [(0, 0), (1, 0), (-2, 0), (1, -2), (-2, 1)];
const I_KICKS_2_3: [(i32, i32); 5] = [(0, 0), (2, 0), (-1, 0), (2, 1), (-1, -2)];
const I_KICKS_3_2: [(i32, i32); 5] = [(0, 0), (-2, 0), (1, 0), (-2, -1), (1, 2)];
const I_KICKS_3_0: [(i32, i32); 5] = [(0, 0), (1, 0), (-2, 0), (1, -2), (-2, 1)];
const I_KICKS_0_3: [(i32, i32); 5] = [(0, 0), (-1, 0), (2, 0), (-1, 2), (2, -1)];

fn srs_kicks(from: u8, to: u8) -> Option<&'static [(i32, i32); 5]> {
    match (from, to) {
        (0, 1) => Some(&KICKS_0_1),
        (1, 0) => Some(&KICKS_1_0),
        (1, 2) => Some(&KICKS_1_2),
        (2, 1) => Some(&KICKS_2_1),
        (2, 3) => Some(&KICKS_2_3),
        (3, 2) => Some(&KICKS_3_2),
        (3, 0) => Some(&KICKS_3_0),
        (0, 3) => Some(&KICKS_0_3),
        _ => None,
    }
}

fn srs_i_kicks(from: u8, to: u8) -> Option<&'static [(i32, i32); 5]> {
    match (from, to) {
        (0, 1) => Some(&I_KICKS_0_1),
        (1, 0) => Some(&I_KICKS_1_0),
        (1, 2) => Some(&I_KICKS_1_2),
        (2, 1) => Some(&I_KICKS_2_1),
        (2, 3) => Some(&I_KICKS_2_3),
        (3, 2) => Some(&I_KICKS_3_2),
        (3, 0) => Some(&I_KICKS_3_0),
        (0, 3) => Some(&I_KICKS_0_3),
        _ => None,
    }
}

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
        Piece::O => 0,
        Piece::I | Piece::T | Piece::S | Piece::Z | Piece::J | Piece::L => 1,
    }
}

fn piece_grid_size(piece: Piece) -> usize {
    match piece {
        Piece::I => 4,
        Piece::O => 2,
        Piece::T | Piece::S | Piece::Z | Piece::J | Piece::L => 3,
    }
}

pub(crate) fn piece_grid(piece: Piece, rotation: u8) -> PieceGrid {
    let mut grid = base_piece_grid(piece);
    for _ in 0..(rotation % 4) {
        grid = rotate_grid_90(&grid);
    }
    grid
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
        Piece::I => PieceGrid {
            size,
            cells: [
                0, 0, 0, 0, //
                1, 1, 1, 1, //
                0, 0, 0, 0, //
                0, 0, 0, 0, //
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
                0, 1, 1, //
                1, 1, 0, //
                0, 0, 0, //
                0, 0, 0, 0, 0, 0, 0,
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
        Piece::T => 3,
        Piece::S => 4,
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
    fn piece_board_offset_matches_tetris_piece_sizes() {
        assert_eq!(piece_board_offset(Piece::O), 0);
        for p in [Piece::I, Piece::T, Piece::S, Piece::Z, Piece::J, Piece::L] {
            assert_eq!(piece_board_offset(p), 1);
        }
    }
}

#[cfg(test)]
mod bottomwell_tests {
    use super::*;

    fn make_bottomwell_core(seed: u64) -> TetrisCore {
        let mut core = TetrisCore::new(seed);
        core.set_available_pieces(Piece::all());
        core.set_bottomwell_enabled(true);
        core.initialize_game();
        core
    }

    #[test]
    fn bottom_3_rows_exist_after_init() {
        let core = make_bottomwell_core(42);
        assert_eq!(core.board.len(), BOARD_HEIGHT);

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
    fn bottom_rows_have_holes_for_playability() {
        let core = make_bottomwell_core(42);

        for y in 0..DEFAULT_BOTTOMWELL_ROWS {
            let holes = core.board[y].iter().filter(|&&c| c == CELL_EMPTY).count();
            assert!(
                holes >= 1 && holes <= 2,
                "bottomwell row y={y} should have 1-2 holes, got {holes}"
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
        assert_eq!(core.board.len(), BOARD_HEIGHT);
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

        // Fill row at y=3 (just above the bottomwell) completely to make it clearable.
        // We also need to fill the bottomwell rows to be full (no holes) so they clear too.
        // Actually, let's set up a single clearable line above the bottomwell.
        // Row at y=DEFAULT_BOTTOMWELL_ROWS is the first "player" row.
        let target_y = DEFAULT_BOTTOMWELL_ROWS;
        for x in 0..BOARD_WIDTH {
            core.set_cell(x, target_y, 1); // fill entire row
        }

        // Force clear.
        let cleared = core.clear_lines();
        assert_eq!(cleared, 1, "should clear exactly 1 line");
        assert_eq!(
            core.earth_depth,
            depth_before + 1,
            "earth_depth should advance by 1"
        );
        assert_eq!(
            core.board.len(),
            BOARD_HEIGHT,
            "board height must stay constant"
        );
    }

    #[test]
    fn clearing_n_lines_reveals_n_earth_lines() {
        let mut core = make_bottomwell_core(42);
        let depth_before = core.earth_depth;

        // Fill 2 rows above the bottomwell.
        for n in 0..2 {
            let target_y = DEFAULT_BOTTOMWELL_ROWS + n;
            for x in 0..BOARD_WIDTH {
                core.set_cell(x, target_y, 1);
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
            BOARD_HEIGHT,
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
                BOARD_HEIGHT,
                "round {round}: board height must remain {BOARD_HEIGHT}"
            );
        }
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
                        || cell == CELL_GARBAGE
                        || cell == CELL_STONE
                        || cell == CELL_ORE
                        || cell == CELL_COIN,
                    "unexpected cell type {cell} at depth {depth}"
                );
            }
        }
    }
}
