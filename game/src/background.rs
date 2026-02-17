/// Tile-based background system.
///
/// Generates deterministic tile patterns behind the board that scroll as
/// the player digs deeper. Visual only — no gameplay collision.
///
/// Supports multiple parallax layers (Far, Mid, Near) and hard biome
/// transitions, including a jagged Overworld->Dirt separator.
use std::sync::OnceLock;

use engine::graphics::{Color, Renderer2d};
use engine::render::{CELL_SIZE, clip_rect_i32_to_viewport, clip_rect_to_viewport};
use engine::ui::Rect;

// ── Biome model ─────────────────────────────────────────────────────

/// Background biome bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundBiome {
    Overworld,
    Dirt,
    Stone,
}

/// Number of near-layer rows represented by the default board viewport.
///
/// Bottomwell is enabled for normal runs, so the default visible board height
/// is 23 rows (20 core + 3 initial bottomwell rows). Calibrating the surface
/// threshold to this runtime viewport keeps run-start dirt reveal aligned with
/// the number of prefilled earth rows.
const START_VIEW_NEAR_ROWS: u32 = 23;

/// Number of earth-biome rows that should be visible at run start.
const START_DIRT_ROWS: u32 = 3;

/// Hard depth threshold separating Overworld from Dirt.
///
/// At depth 0, world rows with depth >= this threshold are Dirt. With
/// `START_VIEW_NEAR_ROWS=20` and `START_DIRT_ROWS=3`, this is 17, so start
/// view always includes at least 3 Dirt rows at the bottom.
const SURFACE_DEPTH_START: u32 = START_VIEW_NEAR_ROWS - START_DIRT_ROWS;

/// Hard depth threshold separating Dirt from Stone (used by the simple
/// `biome_at_depth` helper that does not expose blend info).
const STONE_DEPTH_START: u32 = 50;

/// Width (in near-layer columns) of each jag step in the surface separator.
const SURFACE_JAGGED_SEGMENT_COLS: u32 = 2;

/// Maximum upward raise (in rows) of the Dirt boundary from the base depth.
///
/// Kept at zero so the opening ground line does not start one row too high.
/// Visual jitter is still provided by the grassline separator teeth.
const SURFACE_JAGGED_MAX_RAISE_ROWS: u32 = 0;

// ── Palette ─────────────────────────────────────────────────────────

const CLEAR_COLOR: Color = [0, 0, 0, 255];
const GRASSLINE_COLOR: Color = [94, 152, 72, 255];

/// Tile palettes for each biome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundPalette {
    pub base: Color,
    pub accent_a: Color,
    pub accent_b: Color,
    pub vein: Color,
    pub crack: Color,
    pub fossil: Color,
}

pub const PALETTE_OVERWORLD: BackgroundPalette = BackgroundPalette {
    base: [90, 112, 144, 255],
    accent_a: [104, 128, 162, 255],
    accent_b: [118, 142, 176, 255],
    vein: [136, 160, 194, 255],
    crack: [74, 94, 124, 255],
    fossil: [154, 178, 208, 255],
};

pub const PALETTE_DIRT: BackgroundPalette = BackgroundPalette {
    base: [26, 18, 12, 255],
    accent_a: [40, 29, 18, 255],
    accent_b: [56, 40, 22, 255],
    vein: [76, 55, 30, 255],
    crack: [14, 10, 8, 255],
    fossil: [94, 76, 58, 255],
};

pub const PALETTE_STONE: BackgroundPalette = BackgroundPalette {
    base: [18, 20, 28, 255],
    accent_a: [32, 35, 44, 255],
    accent_b: [46, 52, 66, 255],
    vein: [78, 88, 106, 255],
    crack: [10, 12, 18, 255],
    fossil: [102, 112, 128, 255],
};

pub const V1_BIOME_PALETTES: [BackgroundPalette; 3] =
    [PALETTE_OVERWORLD, PALETTE_DIRT, PALETTE_STONE];

fn palette_for_biome(biome: BackgroundBiome) -> BackgroundPalette {
    match biome {
        BackgroundBiome::Overworld => PALETTE_OVERWORLD,
        BackgroundBiome::Dirt => PALETTE_DIRT,
        BackgroundBiome::Stone => PALETTE_STONE,
    }
}

// ── Biome lookup ────────────────────────────────────────────────────

/// Return the primary biome for a given depth row using the default
/// Overworld->Dirt threshold.
pub fn biome_at_depth(depth: u32) -> BackgroundBiome {
    biome_at_depth_with_surface(depth, SURFACE_DEPTH_START)
}

/// Return biome at depth using a caller-provided Overworld->Dirt threshold.
fn biome_at_depth_with_surface(depth: u32, surface_depth_start: u32) -> BackgroundBiome {
    if depth < surface_depth_start {
        BackgroundBiome::Overworld
    } else if depth < STONE_DEPTH_START {
        BackgroundBiome::Dirt
    } else {
        BackgroundBiome::Stone
    }
}

/// Deterministic jagged depth threshold for the Overworld->Dirt separator.
///
/// `near_col` is expressed in near-layer cell columns (CELL_SIZE pixels).
fn surface_boundary_depth_for_near_col(seed: u64, near_col: u32) -> u32 {
    let segment = near_col / SURFACE_JAGGED_SEGMENT_COLS.max(1);
    let hash = tile_hash(seed ^ 0x9E37_79B9_7F4A_7C15, 0, segment, 0);
    let raise = hash % (SURFACE_JAGGED_MAX_RAISE_ROWS + 1);
    SURFACE_DEPTH_START.saturating_sub(raise)
}

/// Resolve biome for a world depth and near-layer column.
///
/// This is the canonical hard-transition algorithm used at render-time.
fn biome_for_world_depth_at_near_col(
    world_depth: u32,
    near_col: u32,
    seed: u64,
    legacy_underground_start: bool,
) -> BackgroundBiome {
    let surface_depth_start = if legacy_underground_start {
        0
    } else {
        surface_boundary_depth_for_near_col(seed, near_col)
    };
    biome_at_depth_with_surface(world_depth, surface_depth_start)
}

// ── Depth offset ────────────────────────────────────────────────────

/// Map gameplay depth rows to background world-row offset.
///
/// MVP uses a 1:1 mapping. Keeping the conversion in one helper makes it easy
/// to tune parallax scaling later without changing call sites.
pub fn depth_to_background_row_offset(depth_rows: u32) -> u32 {
    depth_rows
}

// ── Tile kinds ──────────────────────────────────────────────────────

/// Visual tile kinds used for procedural motif drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTileKind {
    /// No special motif — base biome fill only.
    Empty,
    Cloud,
    DirtA,
    DirtB,
    RockA,
    RockB,
    Vein,
    Crack,
    Fossil,
}

impl BackgroundTileKind {
    /// Choose a tile kind from a hash value using per-biome weighted tables.
    pub fn from_hash(hash: u32, biome: BackgroundBiome) -> Self {
        let roll = (hash & 0xFF) as u8;

        match biome {
            BackgroundBiome::Overworld => match roll {
                0..228 => BackgroundTileKind::Empty,
                228..250 => BackgroundTileKind::Cloud,
                _ => BackgroundTileKind::Empty,
            },
            BackgroundBiome::Dirt => match roll {
                0..160 => BackgroundTileKind::Empty,
                160..210 => BackgroundTileKind::DirtA,
                210..240 => BackgroundTileKind::DirtB,
                240..250 => BackgroundTileKind::Crack,
                _ => BackgroundTileKind::Fossil,
            },
            BackgroundBiome::Stone => match roll {
                0..100 => BackgroundTileKind::Empty,
                100..160 => BackgroundTileKind::RockA,
                160..210 => BackgroundTileKind::RockB,
                210..235 => BackgroundTileKind::Vein,
                235..250 => BackgroundTileKind::Crack,
                _ => BackgroundTileKind::Fossil,
            },
        }
    }
}

// ── Tile hash ───────────────────────────────────────────────────────

/// Deterministic hash for a background tile position.
///
/// Produces a uniformly-distributed `u32` from a world seed and tile
/// coordinates. The same inputs always yield the same output, which keeps
/// background rendering fully deterministic for replays and profiling.
///
/// Uses an FNV-1a-style mixing loop (fast, no allocation, good distribution
/// for visual purposes).
pub fn tile_hash(seed: u64, depth: u32, col: u32, layer: u32) -> u32 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut h = FNV_OFFSET ^ seed;

    // Mix each coordinate byte-by-byte via FNV-1a.
    for &word in &[depth, col, layer] {
        let bytes = word.to_le_bytes();
        for &b in &bytes {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }

    // Finalise: fold 64-bit hash into 32 bits.
    ((h >> 32) ^ h) as u32
}

// ── Layer model (Phase 2) ───────────────────────────────────────────

/// Named layer identifiers for the parallax background system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundLayerId {
    /// Farthest layer: large tiles, low alpha, slowest scroll.
    Far = 0,
    /// Middle layer: medium tiles, medium alpha.
    Mid = 1,
    /// Nearest layer: gameplay-scale tiles, full alpha.
    Near = 2,
}

impl BackgroundLayerId {
    pub const ALL: [BackgroundLayerId; 3] = [
        BackgroundLayerId::Far,
        BackgroundLayerId::Mid,
        BackgroundLayerId::Near,
    ];
}

/// Per-layer rendering specification.
#[derive(Debug, Clone, Copy)]
pub struct BackgroundLayerSpec {
    /// Layer identifier (also used as hash input for deterministic sampling).
    pub layer_id: BackgroundLayerId,
    /// Tile size in pixels.
    pub tile_px: u32,
    /// Compositing alpha (0 = invisible, 255 = opaque).
    pub alpha: u8,
    /// Parallax depth divisor: higher = slower scroll relative to Near layer.
    pub depth_divisor: u32,
    /// Whether this layer uses full motif rendering (false = base fill only,
    /// cheaper for far layers).
    pub motif_enabled: bool,
}

/// Default v1 layer specs.
///
/// - Far: large tiles, low contrast, slowest movement, no motifs.
/// - Mid: medium tiles, medium alpha, half-speed parallax.
/// - Near: gameplay-scale tiles, full alpha, 1:1 scroll.
pub const LAYER_FAR: BackgroundLayerSpec = BackgroundLayerSpec {
    layer_id: BackgroundLayerId::Far,
    tile_px: CELL_SIZE * 3,
    alpha: 70,
    depth_divisor: 3,
    motif_enabled: false,
};

pub const LAYER_MID: BackgroundLayerSpec = BackgroundLayerSpec {
    layer_id: BackgroundLayerId::Mid,
    tile_px: CELL_SIZE * 2,
    alpha: 108,
    depth_divisor: 2,
    motif_enabled: true,
};

pub const LAYER_NEAR: BackgroundLayerSpec = BackgroundLayerSpec {
    layer_id: BackgroundLayerId::Near,
    tile_px: CELL_SIZE,
    alpha: 255,
    depth_divisor: 1,
    motif_enabled: true,
};

/// All layers in back-to-front draw order.
pub const V1_LAYERS: [BackgroundLayerSpec; 3] = [LAYER_FAR, LAYER_MID, LAYER_NEAR];

// ── Environment toggles (Phase 5) ──────────────────────────────────

const BACKGROUND_DISABLE_ENV: &str = "ROLLOUT_DISABLE_TILE_BG";
const FORCE_BIOME_ENV: &str = "ROLLOUT_FORCE_BIOME";
const PARALLAX_MULT_ENV: &str = "ROLLOUT_PARALLAX_MULT";
const GRID_OVERLAY_ENV: &str = "ROLLOUT_TILE_GRID_OVERLAY";
const DISABLE_LAYER_FAR_ENV: &str = "ROLLOUT_DISABLE_LAYER_FAR";
const DISABLE_LAYER_MID_ENV: &str = "ROLLOUT_DISABLE_LAYER_MID";
const DISABLE_LAYER_NEAR_ENV: &str = "ROLLOUT_DISABLE_LAYER_NEAR";
const BLEND_DEBUG_ENV: &str = "ROLLOUT_BLEND_DEBUG";
const LEGACY_UNDERGROUND_START_ENV: &str = "ROLLOUT_BG_FORCE_LEGACY_UNDERGROUND_START";

/// Environment-controlled kill switch for quick debugging.
///
/// Set `ROLLOUT_DISABLE_TILE_BG=1` to disable the tile background.
pub fn tile_background_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| !env_flag(BACKGROUND_DISABLE_ENV))
}

/// Force all tiles to render as a specific biome.
///
/// Set `ROLLOUT_FORCE_BIOME=overworld|dirt|stone` to override.
fn forced_biome() -> Option<BackgroundBiome> {
    static FORCED: OnceLock<Option<BackgroundBiome>> = OnceLock::new();
    *FORCED.get_or_init(|| {
        std::env::var(FORCE_BIOME_ENV).ok().and_then(|v| {
            match v.trim().to_ascii_lowercase().as_str() {
                "overworld" | "sky" | "surface" => Some(BackgroundBiome::Overworld),
                "dirt" => Some(BackgroundBiome::Dirt),
                "stone" => Some(BackgroundBiome::Stone),
                _ => None,
            }
        })
    })
}

/// Parallax depth-divisor multiplier override.
///
/// Set `ROLLOUT_PARALLAX_MULT=<float>` to scale non-near layers' depth
/// divisors. Values >1 reduce parallax effect, <1 increase it.
fn parallax_mult() -> f32 {
    static MULT: OnceLock<f32> = OnceLock::new();
    *MULT.get_or_init(|| {
        std::env::var(PARALLAX_MULT_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .unwrap_or(1.0)
            .clamp(0.1, 10.0)
    })
}

/// Show a thin grid overlay on each tile boundary for tuning art.
///
/// Set `ROLLOUT_TILE_GRID_OVERLAY=1` to enable.
fn grid_overlay_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(GRID_OVERLAY_ENV))
}

/// Per-layer disable toggles.
///
/// Set `ROLLOUT_DISABLE_LAYER_FAR=1`, `_MID=1`, or `_NEAR=1`.
fn layer_disabled(layer_id: BackgroundLayerId) -> bool {
    static FAR: OnceLock<bool> = OnceLock::new();
    static MID: OnceLock<bool> = OnceLock::new();
    static NEAR: OnceLock<bool> = OnceLock::new();
    match layer_id {
        BackgroundLayerId::Far => *FAR.get_or_init(|| env_flag(DISABLE_LAYER_FAR_ENV)),
        BackgroundLayerId::Mid => *MID.get_or_init(|| env_flag(DISABLE_LAYER_MID_ENV)),
        BackgroundLayerId::Near => *NEAR.get_or_init(|| env_flag(DISABLE_LAYER_NEAR_ENV)),
    }
}

/// Enable debug overlay showing biome pair and blend alpha.
///
/// Set `ROLLOUT_BLEND_DEBUG=1` to enable.
fn blend_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(BLEND_DEBUG_ENV))
}

/// Force old behavior where runs start underground (Dirt).
///
/// Set `ROLLOUT_BG_FORCE_LEGACY_UNDERGROUND_START=1` to bypass the Overworld
/// surface region entirely.
fn legacy_underground_start_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(LEGACY_UNDERGROUND_START_ENV))
}

// ── Draw entry point ────────────────────────────────────────────────

/// Draws the depth-scrolling tile background behind the board.
pub fn draw_tile_background(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    board_rect: Rect,
    depth_rows: u32,
    seed: u64,
) {
    draw_tile_background_in_viewport(frame, width, height, board_rect, depth_rows, seed, 0);
}

/// Draw the background into a fixed viewport while applying a vertical content offset.
///
/// Positive `content_offset_y_px` moves world content downward inside `viewport_rect`.
pub fn draw_tile_background_in_viewport(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    viewport_rect: Rect,
    depth_rows: u32,
    seed: u64,
    content_offset_y_px: i32,
) {
    draw_tile_background_impl(
        frame,
        width,
        height,
        viewport_rect,
        depth_rows,
        seed,
        tile_background_enabled(),
        content_offset_y_px,
    );
}

/// Draw only the Overworld->Dirt grassline separator in board viewport space.
///
/// This is used as an overlay pass so the separator remains visible above
/// bottomwell/locked board cells.
pub fn draw_surface_grass_overlay_in_viewport(
    frame: &mut dyn Renderer2d,
    viewport_rect: Rect,
    depth_rows: u32,
    seed: u64,
    content_offset_y_px: i32,
) {
    if viewport_rect.w == 0 || viewport_rect.h == 0 || !tile_background_enabled() {
        return;
    }
    if layer_disabled(BackgroundLayerId::Near) || legacy_underground_start_enabled() {
        return;
    }
    if forced_biome().is_some() {
        return;
    }

    draw_surface_grass_overlay_impl(frame, viewport_rect, depth_rows, seed, content_offset_y_px);
}

fn draw_tile_background_impl(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    viewport_rect: Rect,
    depth_rows: u32,
    seed: u64,
    enabled: bool,
    content_offset_y_px: i32,
) {
    frame.fill_rect(Rect::from_size(width, height), CLEAR_COLOR);
    if !enabled || viewport_rect.w == 0 || viewport_rect.h == 0 {
        return;
    }

    // Draw back-to-front: Far, Mid, Near.
    for layer in &V1_LAYERS {
        if !layer_disabled(layer.layer_id) {
            draw_tile_layer(
                frame,
                viewport_rect,
                depth_rows,
                seed,
                *layer,
                content_offset_y_px,
            );
        }
    }

    // Optional blend-debug overlay.
    if blend_debug_enabled() {
        draw_blend_debug_overlay(frame, viewport_rect, depth_rows, seed);
    }
}

fn draw_surface_grass_overlay_impl(
    frame: &mut dyn Renderer2d,
    viewport_rect: Rect,
    depth_rows: u32,
    seed: u64,
    content_offset_y_px: i32,
) {
    let tile_px = LAYER_NEAR.tile_px.max(1);
    let visible_cols = viewport_rect.w.div_ceil(tile_px);
    if visible_cols == 0 || viewport_rect.h == 0 {
        return;
    }

    let tile_px_i32 = tile_px.min(i32::MAX as u32) as i32;
    let viewport_h_i32 = viewport_rect.h.min(i32::MAX as u32) as i32;
    let layer_offset = depth_to_background_row_offset(depth_rows) / LAYER_NEAR.depth_divisor.max(1);
    let viewport_max_x = viewport_rect.x.saturating_add(viewport_rect.w);
    let layer_id_u32 = BackgroundLayerId::Near as u32;

    // Match background row coverage so overlay remains aligned under camera offsets.
    let first_visible_row = (-content_offset_y_px - tile_px_i32).div_euclid(tile_px_i32) + 1;
    let last_visible_row = (viewport_h_i32 - content_offset_y_px - 1).div_euclid(tile_px_i32);
    let draw_row_start = first_visible_row.saturating_sub(1);
    let draw_row_end = last_visible_row.saturating_add(1);

    for tile_row in draw_row_start..=draw_row_end {
        let y_i32 = (viewport_rect.y as i32)
            .saturating_add(content_offset_y_px)
            .saturating_add(tile_row.saturating_mul(tile_px_i32));
        let Some(clipped_row_rect) = clip_rect_i32_to_viewport(
            viewport_rect.x as i32,
            y_i32,
            viewport_rect.w,
            tile_px,
            viewport_rect,
        ) else {
            continue;
        };

        let world_depth = world_depth_for_tile_row(layer_offset, tile_row);

        for tile_x in 0..visible_cols {
            let x = viewport_rect
                .x
                .saturating_add(tile_x.saturating_mul(tile_px));
            if x >= viewport_max_x {
                continue;
            }
            let w = viewport_max_x.saturating_sub(x).min(tile_px);
            if w == 0 {
                continue;
            }

            let hash = tile_hash(seed, world_depth, tile_x, layer_id_u32);
            let near_col = near_col_for_tile(viewport_rect, x, w);
            if biome_for_world_depth_at_near_col(world_depth, near_col, seed, false)
                != BackgroundBiome::Dirt
                || world_depth != surface_boundary_depth_for_near_col(seed, near_col)
            {
                continue;
            }

            let unclipped_tile = Rect::new(x, clipped_row_rect.y, w, clipped_row_rect.h);
            let Some(tile_rect) = clip_rect_to_viewport(unclipped_tile, viewport_rect) else {
                continue;
            };
            draw_grassline_separator(frame, tile_rect, LAYER_NEAR.alpha, hash);
        }
    }
}

// ── Per-layer rendering (Phase 4) ───────────────────────────────────

/// Convert a layer-local tile row index into world depth.
///
/// `tile_row_from_top` increases downward in screen space, which keeps deeper
/// biomes entering from the lower board region as dig-progress increases.
fn world_depth_for_tile_row(layer_offset: u32, tile_row_from_top: i32) -> u32 {
    if tile_row_from_top >= 0 {
        layer_offset.saturating_add(tile_row_from_top as u32)
    } else {
        layer_offset.saturating_sub(tile_row_from_top.unsigned_abs())
    }
}

fn near_col_for_tile(viewport_rect: Rect, tile_x: u32, tile_w: u32) -> u32 {
    let rel_center_x = tile_x
        .saturating_sub(viewport_rect.x)
        .saturating_add(tile_w / 2);
    rel_center_x / CELL_SIZE.max(1)
}

fn draw_tile_layer(
    frame: &mut dyn Renderer2d,
    viewport_rect: Rect,
    depth_rows: u32,
    seed: u64,
    layer: BackgroundLayerSpec,
    content_offset_y_px: i32,
) {
    let tile_px = layer.tile_px.max(1);
    let visible_cols = viewport_rect.w.div_ceil(tile_px);
    if visible_cols == 0 || viewport_rect.h == 0 {
        return;
    }

    let tile_px_i32 = tile_px.min(i32::MAX as u32) as i32;
    let viewport_h_i32 = viewport_rect.h.min(i32::MAX as u32) as i32;
    let legacy_underground_start = legacy_underground_start_enabled();
    let forced = forced_biome();

    // Apply parallax multiplier override for non-near layers.
    let effective_divisor = if layer.layer_id != BackgroundLayerId::Near && layer.depth_divisor > 1
    {
        let mult = parallax_mult();
        ((layer.depth_divisor as f32) * mult).round().max(1.0) as u32
    } else {
        layer.depth_divisor.max(1)
    };
    let layer_offset = depth_to_background_row_offset(depth_rows) / effective_divisor;
    let viewport_max_x = viewport_rect.x.saturating_add(viewport_rect.w);
    let layer_id_u32 = layer.layer_id as u32;

    // Draw one extra row above and below the visible range so edge content persists
    // until fully clipped out under partial camera offsets.
    let first_visible_row = (-content_offset_y_px - tile_px_i32).div_euclid(tile_px_i32) + 1;
    let last_visible_row = (viewport_h_i32 - content_offset_y_px - 1).div_euclid(tile_px_i32);
    let draw_row_start = first_visible_row.saturating_sub(1);
    let draw_row_end = last_visible_row.saturating_add(1);

    for tile_row in draw_row_start..=draw_row_end {
        let y_i32 = (viewport_rect.y as i32)
            .saturating_add(content_offset_y_px)
            .saturating_add(tile_row.saturating_mul(tile_px_i32));
        let Some(clipped_row_rect) = clip_rect_i32_to_viewport(
            viewport_rect.x as i32,
            y_i32,
            viewport_rect.w,
            tile_px,
            viewport_rect,
        ) else {
            continue;
        };

        let world_depth = world_depth_for_tile_row(layer_offset, tile_row);

        for tile_x in 0..visible_cols {
            let x = viewport_rect
                .x
                .saturating_add(tile_x.saturating_mul(tile_px));
            if x >= viewport_max_x {
                continue;
            }
            let w = viewport_max_x.saturating_sub(x).min(tile_px);
            if w == 0 {
                continue;
            }

            let hash = tile_hash(seed, world_depth, tile_x, layer_id_u32);
            let near_col = near_col_for_tile(viewport_rect, x, w);
            let biome = if let Some(forced_biome) = forced {
                forced_biome
            } else {
                biome_for_world_depth_at_near_col(
                    world_depth,
                    near_col,
                    seed,
                    legacy_underground_start,
                )
            };
            let palette = palette_for_biome(biome);

            let unclipped_tile = Rect::new(x, clipped_row_rect.y, w, clipped_row_rect.h);
            let Some(tile_rect) = clip_rect_to_viewport(unclipped_tile, viewport_rect) else {
                continue;
            };
            draw_tile_rect(frame, tile_rect, palette.base, layer.alpha);

            if layer.motif_enabled {
                let kind = BackgroundTileKind::from_hash(hash, biome);
                draw_tile_motif(frame, tile_rect, kind, palette, layer.alpha);
            }

            // Draw only the Overworld->Dirt separator as a jagged grassline.
            if forced.is_none()
                && !legacy_underground_start
                && layer.layer_id == BackgroundLayerId::Near
                && biome == BackgroundBiome::Dirt
                && world_depth == surface_boundary_depth_for_near_col(seed, near_col)
            {
                draw_grassline_separator(frame, tile_rect, layer.alpha, hash);
            }

            if grid_overlay_enabled() {
                const GRID_COLOR: Color = [255, 255, 255, 255];
                const GRID_ALPHA: u8 = 30;
                frame.blend_rect(
                    Rect::new(tile_rect.x, tile_rect.y, tile_rect.w, 1),
                    GRID_COLOR,
                    GRID_ALPHA,
                );
                frame.blend_rect(
                    Rect::new(tile_rect.x, tile_rect.y, 1, tile_rect.h),
                    GRID_COLOR,
                    GRID_ALPHA,
                );
            }
        }
    }
}

// ── Tile drawing helpers ────────────────────────────────────────────

fn draw_grassline_separator(frame: &mut dyn Renderer2d, rect: Rect, alpha: u8, hash: u32) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }
    let top_h = (rect.h / 8).max(1).min(3);
    draw_tile_rect(
        frame,
        Rect::new(rect.x, rect.y, rect.w, top_h),
        GRASSLINE_COLOR,
        alpha,
    );

    // Add tiny deterministic "teeth" to avoid a perfectly straight stripe.
    if rect.h > top_h + 1 {
        let tooth_h = (rect.h / 10).max(1).min(2);
        let half_w = (rect.w / 2).max(1);
        let y = rect.y.saturating_add(top_h);
        if (hash & 0b01) != 0 {
            draw_tile_rect(
                frame,
                Rect::new(rect.x, y, half_w, tooth_h),
                GRASSLINE_COLOR,
                alpha,
            );
        }
        if (hash & 0b10) != 0 {
            draw_tile_rect(
                frame,
                Rect::new(rect.x + rect.w.saturating_sub(half_w), y, half_w, tooth_h),
                GRASSLINE_COLOR,
                alpha,
            );
        }
    }
}

fn draw_tile_rect(frame: &mut dyn Renderer2d, rect: Rect, color: Color, alpha: u8) {
    if alpha >= 255 {
        frame.fill_rect(rect, color);
    } else {
        frame.blend_rect(rect, color, alpha);
    }
}

fn draw_tile_motif(
    frame: &mut dyn Renderer2d,
    rect: Rect,
    kind: BackgroundTileKind,
    palette: BackgroundPalette,
    alpha: u8,
) {
    if rect.w == 0 || rect.h == 0 {
        return;
    }

    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let dot = (rect.w.min(rect.h) / 6).max(1);

    match kind {
        BackgroundTileKind::Empty => {}
        BackgroundTileKind::Cloud => {
            let cloud_alpha = ((alpha as u16).saturating_mul(160) / 255).max(20) as u8;
            let span_w = (rect.w.saturating_mul(2) / 3).max(2).min(rect.w);
            let puff_h = (rect.h / 6).max(1);
            let x0 = rect.x + rect.w.saturating_sub(span_w) / 2;
            let y0 = rect.y + rect.h / 3;
            draw_tile_rect(
                frame,
                Rect::new(x0, y0, span_w, puff_h),
                palette.accent_a,
                cloud_alpha,
            );

            if rect.h > puff_h + 1 {
                let y1 = y0.saturating_add(puff_h.saturating_add(1));
                let inner_w = (span_w.saturating_mul(2) / 3).max(1);
                let x1 = x0 + span_w.saturating_sub(inner_w) / 2;
                draw_tile_rect(
                    frame,
                    Rect::new(x1, y1, inner_w, puff_h),
                    palette.accent_b,
                    cloud_alpha,
                );
            }
        }
        BackgroundTileKind::DirtA | BackgroundTileKind::RockA => {
            let x = cx.saturating_sub(dot);
            let y = cy.saturating_sub(dot);
            draw_tile_rect(
                frame,
                Rect::new(x, y, dot * 2, dot * 2),
                palette.accent_a,
                alpha,
            );
        }
        BackgroundTileKind::DirtB | BackgroundTileKind::RockB => {
            let x0 = rect.x + rect.w / 3;
            let y0 = rect.y + rect.h / 3;
            let x1 = rect.x + (rect.w * 2) / 3;
            let y1 = rect.y + (rect.h * 2) / 3;
            draw_tile_rect(frame, Rect::new(x0, y0, dot, dot), palette.accent_b, alpha);
            draw_tile_rect(
                frame,
                Rect::new(x1.saturating_sub(dot), y1.saturating_sub(dot), dot, dot),
                palette.accent_b,
                alpha,
            );
        }
        BackgroundTileKind::Vein => {
            let vein_w = (rect.w / 5).max(1);
            let vein_x = rect.x + rect.w / 2 - vein_w / 2;
            draw_tile_rect(
                frame,
                Rect::new(vein_x, rect.y, vein_w, rect.h),
                palette.vein,
                alpha,
            );
        }
        BackgroundTileKind::Crack => {
            let crack_w = (rect.w / 6).max(1);
            draw_tile_rect(
                frame,
                Rect::new(rect.x, cy, rect.w, crack_w),
                palette.crack,
                alpha,
            );
            if rect.h > crack_w + 1 {
                let diag_y = rect.y + rect.h / 3;
                draw_tile_rect(
                    frame,
                    Rect::new(rect.x + rect.w / 3, diag_y, rect.w / 2, crack_w),
                    palette.crack,
                    alpha,
                );
            }
        }
        BackgroundTileKind::Fossil => {
            let fw = (rect.w / 2).max(2).min(rect.w);
            let fh = (rect.h / 2).max(2).min(rect.h);
            let fx = rect.x + rect.w.saturating_sub(fw) / 2;
            let fy = rect.y + rect.h.saturating_sub(fh) / 2;
            draw_tile_rect(frame, Rect::new(fx, fy, fw, 1), palette.fossil, alpha);
            draw_tile_rect(
                frame,
                Rect::new(fx, fy + fh.saturating_sub(1), fw, 1),
                palette.fossil,
                alpha,
            );
            draw_tile_rect(frame, Rect::new(fx, fy, 1, fh), palette.fossil, alpha);
            draw_tile_rect(
                frame,
                Rect::new(fx + fw.saturating_sub(1), fy, 1, fh),
                palette.fossil,
                alpha,
            );
        }
    }
}

// ── Blend debug overlay (Phase 5) ───────────────────────────────────

fn draw_blend_debug_overlay(
    frame: &mut dyn Renderer2d,
    board_rect: Rect,
    depth_rows: u32,
    seed: u64,
) {
    // Show current biome state for the center column.
    let sample_depth = depth_to_background_row_offset(depth_rows);
    let center_col = board_rect
        .w
        .saturating_div(CELL_SIZE.max(1))
        .saturating_div(2);
    let legacy = legacy_underground_start_enabled();
    let surface_depth = if legacy {
        0
    } else {
        surface_boundary_depth_for_near_col(seed, center_col)
    };
    let biome = biome_for_world_depth_at_near_col(sample_depth, center_col, seed, legacy);
    let biome_name = |b: BackgroundBiome| match b {
        BackgroundBiome::Overworld => "OVERWORLD",
        BackgroundBiome::Dirt => "DIRT",
        BackgroundBiome::Stone => "STONE",
    };
    let text = format!(
        "BG: {} d={} surf={} stone={}",
        biome_name(biome),
        sample_depth,
        surface_depth,
        STONE_DEPTH_START
    );
    let tx = board_rect.x + 4;
    let ty = board_rect.y + 4;
    frame.draw_text(tx, ty, &text, [255, 255, 0, 255]);
}

// ── Helpers ─────────────────────────────────────────────────────────

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

// ── Tests (Phase 7) ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use engine::graphics::CpuRenderer;
    use engine::surface::SurfaceSize;

    // -- tile_hash -----------------------------------------------------------

    #[test]
    fn tile_hash_deterministic() {
        let a = tile_hash(42, 10, 3, 0);
        let b = tile_hash(42, 10, 3, 0);
        assert_eq!(a, b, "same inputs must produce same hash");
    }

    #[test]
    fn tile_hash_varies_with_seed() {
        let a = tile_hash(1, 10, 3, 0);
        let b = tile_hash(2, 10, 3, 0);
        assert_ne!(a, b, "different seeds should produce different hashes");
    }

    #[test]
    fn tile_hash_varies_with_depth() {
        let a = tile_hash(42, 0, 3, 0);
        let b = tile_hash(42, 1, 3, 0);
        assert_ne!(a, b, "different depths should produce different hashes");
    }

    #[test]
    fn tile_hash_varies_with_col() {
        let a = tile_hash(42, 10, 0, 0);
        let b = tile_hash(42, 10, 1, 0);
        assert_ne!(a, b, "different columns should produce different hashes");
    }

    #[test]
    fn tile_hash_varies_with_layer() {
        let a = tile_hash(42, 10, 3, 0);
        let b = tile_hash(42, 10, 3, 1);
        assert_ne!(a, b, "different layers should produce different hashes");
    }

    #[test]
    fn tile_hash_varies_across_all_three_layers() {
        let far = tile_hash(42, 10, 3, BackgroundLayerId::Far as u32);
        let mid = tile_hash(42, 10, 3, BackgroundLayerId::Mid as u32);
        let near = tile_hash(42, 10, 3, BackgroundLayerId::Near as u32);
        assert_ne!(far, mid, "Far and Mid layers should hash differently");
        assert_ne!(mid, near, "Mid and Near layers should hash differently");
        assert_ne!(far, near, "Far and Near layers should hash differently");
    }

    // -- biome_at_depth ------------------------------------------------------

    #[test]
    fn biome_overworld_below_surface_threshold() {
        for d in 0..SURFACE_DEPTH_START {
            assert_eq!(biome_at_depth(d), BackgroundBiome::Overworld);
        }
    }

    #[test]
    fn biome_dirt_between_surface_and_stone_thresholds() {
        for d in SURFACE_DEPTH_START..STONE_DEPTH_START {
            assert_eq!(biome_at_depth(d), BackgroundBiome::Dirt);
        }
    }

    #[test]
    fn biome_stone_at_and_above_stone_threshold() {
        assert_eq!(biome_at_depth(STONE_DEPTH_START), BackgroundBiome::Stone);
        assert_eq!(
            biome_at_depth(STONE_DEPTH_START + 100),
            BackgroundBiome::Stone
        );
        assert_eq!(biome_at_depth(u32::MAX), BackgroundBiome::Stone);
    }

    // -- hard surface separator ----------------------------------------------

    #[test]
    fn surface_boundary_is_deterministic_for_same_seed_and_column() {
        let a = surface_boundary_depth_for_near_col(1234, 7);
        let b = surface_boundary_depth_for_near_col(1234, 7);
        assert_eq!(a, b);
    }

    #[test]
    fn surface_boundary_stays_flat_when_row_raise_is_disabled() {
        let mut seen = std::collections::BTreeSet::new();
        for col in 0..32 {
            seen.insert(surface_boundary_depth_for_near_col(1234, col));
        }
        assert!(
            seen.len() == 1,
            "surface boundary row should stay fixed when row-raise jitter is disabled"
        );
    }

    #[test]
    fn start_view_has_bottom_three_rows_in_dirt_for_all_columns() {
        let seed = 42u64;
        let bottom_three = [
            SURFACE_DEPTH_START,
            SURFACE_DEPTH_START + 1,
            SURFACE_DEPTH_START + 2,
        ];
        for col in 0..48 {
            for &depth in &bottom_three {
                assert_eq!(
                    biome_for_world_depth_at_near_col(depth, col, seed, false),
                    BackgroundBiome::Dirt,
                    "expected start bottom rows to be Dirt at col={col}, depth={depth}"
                );
            }
        }
    }

    #[test]
    fn separator_moves_up_one_row_per_depth_step_in_near_layer() {
        let seed = 77u64;
        let col = 5u32;
        let boundary = surface_boundary_depth_for_near_col(seed, col);
        assert!(
            boundary >= 1,
            "boundary must support a one-step upward move"
        );

        let row_at_depth0 = boundary;
        let row_at_depth1 = boundary - 1;
        assert_eq!(row_at_depth1 + 1, row_at_depth0);

        // At depth_rows=0, row_at_depth0 is first Dirt row.
        assert_eq!(
            biome_for_world_depth_at_near_col(row_at_depth0, col, seed, false),
            BackgroundBiome::Dirt
        );
        assert_eq!(
            biome_for_world_depth_at_near_col(row_at_depth0 - 1, col, seed, false),
            BackgroundBiome::Overworld
        );

        // At depth_rows=1, separator shifts up by one row.
        let world_depth_new_separator = 1 + row_at_depth1;
        assert_eq!(
            world_depth_new_separator, boundary,
            "new separator row should map to the same boundary depth"
        );
    }

    #[test]
    fn legacy_underground_start_skips_overworld() {
        assert_eq!(
            biome_for_world_depth_at_near_col(0, 0, 1, true),
            BackgroundBiome::Dirt
        );
    }

    // -- depth_to_background_row_offset --------------------------------------

    #[test]
    fn depth_to_background_row_offset_is_identity_in_mvp() {
        assert_eq!(depth_to_background_row_offset(0), 0);
        assert_eq!(depth_to_background_row_offset(1), 1);
        assert_eq!(depth_to_background_row_offset(17), 17);
        assert_eq!(depth_to_background_row_offset(1_000), 1_000);
        assert_eq!(depth_to_background_row_offset(u32::MAX), u32::MAX);
    }

    #[test]
    fn depth_to_background_row_offset_is_monotonic() {
        let mut prev = depth_to_background_row_offset(0);
        for depth in 1..=1_024 {
            let next = depth_to_background_row_offset(depth);
            assert!(
                next >= prev,
                "row offset should not decrease as depth increases"
            );
            prev = next;
        }
    }

    #[test]
    fn world_depth_increases_from_top_to_bottom_rows() {
        let layer_offset = 7u32;
        let top_row_depth = world_depth_for_tile_row(layer_offset, 0);
        let bottom_row_depth = world_depth_for_tile_row(layer_offset, 19);
        assert!(
            bottom_row_depth > top_row_depth,
            "deeper biome contribution should grow toward lower board rows"
        );
    }

    // -- tile kind from hash -------------------------------------------------

    #[test]
    fn tile_kind_deterministic() {
        let h = tile_hash(42, 5, 3, 0);
        let a = BackgroundTileKind::from_hash(h, BackgroundBiome::Dirt);
        let b = BackgroundTileKind::from_hash(h, BackgroundBiome::Dirt);
        assert_eq!(a, b);
    }

    #[test]
    fn tile_kind_covers_all_rolls_overworld() {
        for roll in 0u32..=255 {
            let _ = BackgroundTileKind::from_hash(roll, BackgroundBiome::Overworld);
        }
    }

    #[test]
    fn tile_kind_covers_all_rolls_dirt() {
        for roll in 0u32..=255 {
            let _ = BackgroundTileKind::from_hash(roll, BackgroundBiome::Dirt);
        }
    }

    #[test]
    fn tile_kind_covers_all_rolls_stone() {
        for roll in 0u32..=255 {
            let _ = BackgroundTileKind::from_hash(roll, BackgroundBiome::Stone);
        }
    }

    #[test]
    fn tile_kind_distribution_dirt_mostly_empty() {
        let empties = (0u32..256)
            .filter(|&r| {
                BackgroundTileKind::from_hash(r, BackgroundBiome::Dirt) == BackgroundTileKind::Empty
            })
            .count();
        assert!(empties > 128, "Dirt biome should be majority Empty tiles");
    }

    #[test]
    fn tile_kind_distribution_overworld_is_low_noise() {
        let empties = (0u32..256)
            .filter(|&r| {
                BackgroundTileKind::from_hash(r, BackgroundBiome::Overworld)
                    == BackgroundTileKind::Empty
            })
            .count();
        assert!(
            empties > 200,
            "Overworld should remain mostly empty to keep readability"
        );
    }

    #[test]
    fn tile_kind_distribution_stone_has_rock() {
        let rocks = (0u32..256)
            .filter(|&r| {
                matches!(
                    BackgroundTileKind::from_hash(r, BackgroundBiome::Stone),
                    BackgroundTileKind::RockA | BackgroundTileKind::RockB
                )
            })
            .count();
        assert!(
            rocks > 40,
            "Stone biome should have a notable share of rock tiles"
        );
    }

    // -- layer model ---------------------------------------------------------

    #[test]
    fn layer_specs_have_correct_ids() {
        assert_eq!(LAYER_FAR.layer_id, BackgroundLayerId::Far);
        assert_eq!(LAYER_MID.layer_id, BackgroundLayerId::Mid);
        assert_eq!(LAYER_NEAR.layer_id, BackgroundLayerId::Near);
    }

    #[test]
    fn layer_parallax_factors_increase_front_to_back() {
        assert!(
            LAYER_FAR.depth_divisor > LAYER_MID.depth_divisor,
            "Far should have higher divisor than Mid"
        );
        assert!(
            LAYER_MID.depth_divisor > LAYER_NEAR.depth_divisor,
            "Mid should have higher divisor than Near"
        );
    }

    #[test]
    fn layer_alpha_increases_front_to_back() {
        assert!(
            LAYER_NEAR.alpha > LAYER_MID.alpha,
            "Near should have higher alpha than Mid"
        );
        assert!(
            LAYER_MID.alpha > LAYER_FAR.alpha,
            "Mid should have higher alpha than Far"
        );
    }

    #[test]
    fn near_layer_is_full_alpha() {
        assert_eq!(LAYER_NEAR.alpha, 255);
    }

    #[test]
    fn far_layer_disables_motifs() {
        assert!(
            !LAYER_FAR.motif_enabled,
            "Far layer should not render motifs"
        );
    }

    #[test]
    fn mid_and_near_layers_enable_motifs() {
        assert!(LAYER_MID.motif_enabled);
        assert!(LAYER_NEAR.motif_enabled);
    }

    #[test]
    fn v1_layers_in_back_to_front_order() {
        assert_eq!(V1_LAYERS[0].layer_id, BackgroundLayerId::Far);
        assert_eq!(V1_LAYERS[1].layer_id, BackgroundLayerId::Mid);
        assert_eq!(V1_LAYERS[2].layer_id, BackgroundLayerId::Near);
    }

    // -- parallax offset differences -----------------------------------------

    #[test]
    fn parallax_offsets_differ_between_layers() {
        let depth = 60u32;
        let far_offset = depth_to_background_row_offset(depth) / LAYER_FAR.depth_divisor;
        let mid_offset = depth_to_background_row_offset(depth) / LAYER_MID.depth_divisor;
        let near_offset = depth_to_background_row_offset(depth) / LAYER_NEAR.depth_divisor;
        // Near should move faster (larger offset) than Far for the same depth.
        assert!(
            near_offset >= mid_offset,
            "Near offset should be >= Mid offset"
        );
        assert!(
            mid_offset >= far_offset,
            "Mid offset should be >= Far offset"
        );
        // At depth 60, they should actually differ.
        assert_ne!(far_offset, near_offset);
    }

    // -- draw_tile_background ------------------------------------------------

    const TEST_FRAME_W: u32 = 480;
    const TEST_FRAME_H: u32 = 360;

    fn test_board_rect() -> Rect {
        Rect::new(120, 0, 240, 360)
    }

    fn draw_background_frame(seed: u64, depth_rows: u32) -> Vec<u8> {
        draw_background_frame_with_offset(seed, depth_rows, 0)
    }

    fn draw_background_frame_with_offset(
        seed: u64,
        depth_rows: u32,
        content_offset_y_px: i32,
    ) -> Vec<u8> {
        let mut frame = vec![0u8; (TEST_FRAME_W * TEST_FRAME_H * 4) as usize];
        let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(TEST_FRAME_W, TEST_FRAME_H));
        draw_tile_background_impl(
            &mut gfx,
            TEST_FRAME_W,
            TEST_FRAME_H,
            test_board_rect(),
            depth_rows,
            seed,
            true,
            content_offset_y_px,
        );
        frame
    }

    fn frame_diff_count(a: &[u8], b: &[u8]) -> usize {
        a.chunks_exact(4)
            .zip(b.chunks_exact(4))
            .filter(|(pa, pb)| pa != pb)
            .count()
    }

    fn pixel_at(frame: &[u8], frame_w: u32, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y * frame_w + x) * 4) as usize;
        [frame[idx], frame[idx + 1], frame[idx + 2], frame[idx + 3]]
    }

    #[test]
    fn draw_tile_background_same_seed_and_depth_matches() {
        let a = draw_background_frame(777, 12);
        let b = draw_background_frame(777, 12);
        assert_eq!(a, b, "same seed + depth should draw identical background");
    }

    #[test]
    fn draw_tile_background_scrolls_when_depth_changes() {
        let a = draw_background_frame(777, 12);
        let b = draw_background_frame(777, 13);
        assert_ne!(a, b, "depth offset should move visible tile pattern");
    }

    #[test]
    fn draw_tile_background_differs_between_surface_and_deep_depths() {
        let surface = draw_background_frame(42, 0);
        let stone = draw_background_frame(42, 80);
        assert_ne!(
            surface, stone,
            "background should look different between Overworld and deep Stone depths"
        );
    }

    #[test]
    fn draw_tile_background_first_dig_step_changes_surface_frame() {
        let start = draw_background_frame(42, 0);
        let progressed = draw_background_frame(42, 1);
        assert!(
            frame_diff_count(&start, &progressed) > 0,
            "first dig step should change the rendered background frame"
        );
    }

    #[test]
    fn draw_tile_background_positive_offset_keeps_top_edge_filled() {
        let shifted = draw_background_frame_with_offset(42, 0, (CELL_SIZE as i32) / 2);
        let board = test_board_rect();
        let sample = pixel_at(&shifted, TEST_FRAME_W, board.x + 1, board.y + 1);
        assert_ne!(
            sample, CLEAR_COLOR,
            "top edge should remain background-filled while content is partially offset"
        );
    }

    // -- deterministic per-layer sampling ------------------------------------

    #[test]
    fn same_tile_same_layer_is_deterministic() {
        for layer in BackgroundLayerId::ALL {
            let h1 = tile_hash(99, 20, 5, layer as u32);
            let h2 = tile_hash(99, 20, 5, layer as u32);
            assert_eq!(h1, h2, "hash must be deterministic for layer {:?}", layer);
        }
    }

    #[test]
    fn tile_selection_stable_with_fixed_seed_depth_layer() {
        // Verify that tile kind + biome are stable for a fixed configuration.
        let seed = 12345u64;
        let depth = 30u32;
        let col = 7u32;
        for layer in BackgroundLayerId::ALL {
            let h = tile_hash(seed, depth, col, layer as u32);
            let biome = biome_for_world_depth_at_near_col(depth, col, seed, false);
            let kind = BackgroundTileKind::from_hash(h, biome);

            let h2 = tile_hash(seed, depth, col, layer as u32);
            let biome2 = biome_for_world_depth_at_near_col(depth, col, seed, false);
            let kind2 = BackgroundTileKind::from_hash(h2, biome2);

            assert_eq!(biome, biome2);
            assert_eq!(kind, kind2);
        }
    }
}
