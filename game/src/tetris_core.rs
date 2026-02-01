use std::ops::Add;

pub const BOARD_WIDTH: usize = 10;
pub const BOARD_HEIGHT: usize = 20;
pub const NEXT_QUEUE_LEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub game_over: bool,
}

#[derive(Debug, Clone)]
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
    rng: Rng,
    lines_cleared: u32,
    game_over: bool,
    last_srs_kick_offset: Vec2i,
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
            rng: Rng::new(seed),
            lines_cleared: 0,
            game_over: false,
            last_srs_kick_offset: Vec2i::ZERO,
        }
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
        self.game_over = false;
        self.last_srs_kick_offset = Vec2i::ZERO;
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

        let grid = get_rotated_piece(piece, self.current_piece_rotation);
        let offset = get_board_offset(piece) as i32;
        let piece_type = piece_type(piece);

        for (y, row) in grid.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                if cell != 1 {
                    continue;
                }

                let board_x = self.current_piece_pos.x + x as i32 - offset;
                let board_y = self.current_piece_pos.y - y as i32 + offset;

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
        self.piece_bag
            .pop()
            .unwrap_or(Piece::O)
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
        let grid = get_rotated_piece(piece, rotation);
        let offset = get_board_offset(piece) as i32;

        for (y, row) in grid.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                if cell != 1 {
                    continue;
                }

                let board_x = pos.x + x as i32 - offset;
                let board_y = pos.y - y as i32 + offset;

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
        let mut drop_distance = 0;
        while self.move_piece_down() {
            drop_distance += 1;
        }

        self.place_piece();
        self.clear_lines();
        self.spawn_new_piece();
        drop_distance
    }

    pub fn clear_lines(&mut self) -> usize {
        let mut lines_to_clear = Vec::new();

        for y in 0..BOARD_HEIGHT {
            if self.board[y].iter().all(|&cell| cell != 0) {
                lines_to_clear.push(y);
            }
        }

        if !lines_to_clear.is_empty() {
            lines_to_clear.sort_unstable_by(|a, b| b.cmp(a));
            for line_y in &lines_to_clear {
                self.board.remove(*line_y);
                self.board.push(vec![0; BOARD_WIDTH]);
            }
            self.lines_cleared += lines_to_clear.len() as u32;
        }

        lines_to_clear.len()
    }

    fn place_piece(&mut self) {
        let piece = match self.current_piece {
            Some(piece) => piece,
            None => return,
        };
        let grid = get_rotated_piece(piece, self.current_piece_rotation);
        let offset = get_board_offset(piece) as i32;
        let piece_type = piece_type(piece);

        for (y, row) in grid.iter().enumerate() {
            for (x, &cell) in row.iter().enumerate() {
                if cell != 1 {
                    continue;
                }

                let board_x = self.current_piece_pos.x + x as i32 - offset;
                let board_y = self.current_piece_pos.y - y as i32 + offset;

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

#[derive(Debug, Clone)]
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        let seed = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
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

fn piece_type(piece: Piece) -> u8 {
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

fn get_rotated_piece(piece: Piece, rotation: u8) -> Vec<Vec<u8>> {
    let mut grid = base_piece_grid(piece);
    for _ in 0..(rotation % 4) {
        grid = rotate_grid_90(&grid);
    }
    grid
}

fn rotate_grid_90(grid: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let size = grid.len();
    let mut rotated = vec![vec![0; size]; size];

    for y in 0..size {
        for x in 0..size {
            rotated[x][size - 1 - y] = grid[y][x];
        }
    }

    rotated
}

fn get_board_offset(piece: Piece) -> usize {
    let size = base_piece_grid(piece).len();
    if size == 2 {
        0
    } else {
        1
    }
}

fn base_piece_grid(piece: Piece) -> Vec<Vec<u8>> {
    match piece {
        Piece::I => vec![
            vec![0, 0, 0, 0],
            vec![1, 1, 1, 1],
            vec![0, 0, 0, 0],
            vec![0, 0, 0, 0],
        ],
        Piece::O => vec![vec![1, 1], vec![1, 1]],
        Piece::T => vec![vec![0, 1, 0], vec![1, 1, 1], vec![0, 0, 0]],
        Piece::S => vec![vec![0, 1, 1], vec![1, 1, 0], vec![0, 0, 0]],
        Piece::Z => vec![vec![1, 1, 0], vec![0, 1, 1], vec![0, 0, 0]],
        Piece::J => vec![vec![1, 0, 0], vec![1, 1, 1], vec![0, 0, 0]],
        Piece::L => vec![vec![0, 0, 1], vec![1, 1, 1], vec![0, 0, 0]],
    }
}
