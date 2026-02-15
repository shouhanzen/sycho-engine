/// Tile-based background system.
///
/// Generates deterministic tile patterns behind the board that scroll as
/// the player digs deeper. Visual only — no gameplay collision.
///
/// Supports multiple parallax layers (Far, Mid, Near) and smooth biome
/// transitions via blend windows.
use std::sync::OnceLock;

use engine::graphics::{Color, Renderer2d};
use engine::render::CELL_SIZE;
use engine::ui::Rect;

// ── Biome model ─────────────────────────────────────────────────────

/// Background biome bands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundBiome {
    Dirt,
    Stone,
}

/// Hard depth threshold separating Dirt from Stone (used by the simple
/// `biome_at_depth` helper that does not expose blend info).
const STONE_DEPTH_START: u32 = 50;

/// Total rows around the boundary where blending occurs.
const BIOME_BLEND_BAND_ROWS: u32 = 10;

// ── Palette ─────────────────────────────────────────────────────────

const CLEAR_COLOR: Color = [0, 0, 0, 255];

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

pub const V1_BIOME_PALETTES: [BackgroundPalette; 2] = [PALETTE_DIRT, PALETTE_STONE];

fn palette_for_biome(biome: BackgroundBiome) -> BackgroundPalette {
    match biome {
        BackgroundBiome::Dirt => PALETTE_DIRT,
        BackgroundBiome::Stone => PALETTE_STONE,
    }
}

// ── Biome lookup ────────────────────────────────────────────────────

/// Return the primary biome for a given depth row (hard switch, no blend).
pub fn biome_at_depth(depth: u32) -> BackgroundBiome {
    if depth < STONE_DEPTH_START {
        BackgroundBiome::Dirt
    } else {
        BackgroundBiome::Stone
    }
}

/// Biome band definition for the blend-window system.
#[derive(Debug, Clone, Copy)]
pub struct BiomeBand {
    /// Depth at which the transition center lies.
    pub center_depth: u32,
    /// Half-width of the blend window (rows on each side of center).
    pub half_width: u32,
    /// Biome below the band.
    pub biome_below: BackgroundBiome,
    /// Biome above the band.
    pub biome_above: BackgroundBiome,
}

/// Default v1 biome bands: Dirt -> Stone at depth 50 with 10-row blend.
pub const V1_BIOME_BANDS: &[BiomeBand] = &[BiomeBand {
    center_depth: STONE_DEPTH_START,
    half_width: BIOME_BLEND_BAND_ROWS / 2,
    biome_below: BackgroundBiome::Dirt,
    biome_above: BackgroundBiome::Stone,
}];

/// Result of querying the biome blend at a depth.
///
/// `blend_alpha` is 0 when entirely in `primary`, and 255 when entirely in
/// `secondary`. Values in between indicate a smooth transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BiomeBlendInfo {
    pub primary: BackgroundBiome,
    pub secondary: BackgroundBiome,
    /// 0 = pure primary, 255 = pure secondary.
    pub blend_alpha: u8,
}

impl BiomeBlendInfo {
    /// Convenience: a single-biome result with no blending.
    pub const fn pure(biome: BackgroundBiome) -> Self {
        Self {
            primary: biome,
            secondary: biome,
            blend_alpha: 0,
        }
    }
}

/// Compute the blended biome pair for a given depth, using the band table.
///
/// Returns `(primary, secondary, blend_alpha)` where `blend_alpha` is 0..255.
/// Outside any blend window the result is a single biome with alpha 0.
pub fn biome_blend_at_depth(depth: u32) -> BiomeBlendInfo {
    biome_blend_at_depth_with_bands(depth, V1_BIOME_BANDS)
}

/// Testable version that accepts an explicit band table.
pub fn biome_blend_at_depth_with_bands(depth: u32, bands: &[BiomeBand]) -> BiomeBlendInfo {
    for band in bands {
        let band_start = band.center_depth.saturating_sub(band.half_width);
        let band_end = band.center_depth.saturating_add(band.half_width);

        if depth < band_start {
            return BiomeBlendInfo::pure(band.biome_below);
        }
        if depth >= band_end {
            // Past this band — continue to next band (or fall through to last biome).
            continue;
        }

        // Inside the blend window.
        // Use (width - 1) as denominator so alpha reaches 255 at the last
        // row of the window, making the transition to the pure upper biome
        // seamless (no abrupt jump).
        let width = band_end.saturating_sub(band_start).max(1);
        let pos = depth.saturating_sub(band_start);
        let denom = width.saturating_sub(1).max(1) as u64;
        let alpha_255 = ((pos as u64).saturating_mul(255)) / denom;
        return BiomeBlendInfo {
            primary: band.biome_below,
            secondary: band.biome_above,
            blend_alpha: alpha_255.min(255) as u8,
        };
    }

    // Past all bands: use the last band's upper biome.
    let last_biome = bands
        .last()
        .map(|b| b.biome_above)
        .unwrap_or(BackgroundBiome::Dirt);
    BiomeBlendInfo::pure(last_biome)
}

/// Legacy probabilistic transition: picks a single biome for a tile
/// using the blend window and a hash-based probabilistic roll.
///
/// Used in tests and available for callers that need a single biome
/// rather than a blended palette.
#[allow(dead_code)]
pub(crate) fn biome_with_transition(depth: u32, hash: u32) -> BackgroundBiome {
    let blend = biome_blend_at_depth(depth);
    if blend.blend_alpha == 0 {
        return blend.primary;
    }
    // Probabilistic: use hash bits to pick one biome weighted by blend_alpha.
    let roll = (hash >> 8) & 0xFF;
    if roll <= blend.blend_alpha as u32 {
        blend.secondary
    } else {
        blend.primary
    }
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
const FORCE_BLEND_ALPHA_ENV: &str = "ROLLOUT_FORCE_BLEND_ALPHA";
const BLEND_DEBUG_ENV: &str = "ROLLOUT_BLEND_DEBUG";

/// Environment-controlled kill switch for quick debugging.
///
/// Set `ROLLOUT_DISABLE_TILE_BG=1` to disable the tile background.
pub fn tile_background_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| !env_flag(BACKGROUND_DISABLE_ENV))
}

/// Force all tiles to render as a specific biome.
///
/// Set `ROLLOUT_FORCE_BIOME=dirt` or `ROLLOUT_FORCE_BIOME=stone` to override.
fn forced_biome() -> Option<BackgroundBiome> {
    static FORCED: OnceLock<Option<BackgroundBiome>> = OnceLock::new();
    *FORCED.get_or_init(|| {
        std::env::var(FORCE_BIOME_ENV).ok().and_then(|v| {
            match v.trim().to_ascii_lowercase().as_str() {
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

/// Force blend alpha override (0..255). `None` = use computed blend.
///
/// Set `ROLLOUT_FORCE_BLEND_ALPHA=128` to force half-blend everywhere.
fn forced_blend_alpha() -> Option<u8> {
    static VAL: OnceLock<Option<u8>> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var(FORCE_BLEND_ALPHA_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u16>().ok())
            .map(|v| v.min(255) as u8)
    })
}

/// Enable debug overlay showing biome pair and blend alpha.
///
/// Set `ROLLOUT_BLEND_DEBUG=1` to enable.
fn blend_debug_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| env_flag(BLEND_DEBUG_ENV))
}

// ── Color blending utilities ────────────────────────────────────────

/// Linearly interpolate two colors by `t` (0..255). `t=0` returns `a`, `t=255` returns `b`.
fn lerp_color(a: Color, b: Color, t: u8) -> Color {
    if t == 0 {
        return a;
    }
    if t == 255 {
        return b;
    }
    let t16 = t as u16;
    let inv = 255u16 - t16;
    [
        ((a[0] as u16 * inv + b[0] as u16 * t16) / 255) as u8,
        ((a[1] as u16 * inv + b[1] as u16 * t16) / 255) as u8,
        ((a[2] as u16 * inv + b[2] as u16 * t16) / 255) as u8,
        ((a[3] as u16 * inv + b[3] as u16 * t16) / 255) as u8,
    ]
}

/// Blend two palettes by `t` (0..255).
fn lerp_palette(a: BackgroundPalette, b: BackgroundPalette, t: u8) -> BackgroundPalette {
    BackgroundPalette {
        base: lerp_color(a.base, b.base, t),
        accent_a: lerp_color(a.accent_a, b.accent_a, t),
        accent_b: lerp_color(a.accent_b, b.accent_b, t),
        vein: lerp_color(a.vein, b.vein, t),
        crack: lerp_color(a.crack, b.crack, t),
        fossil: lerp_color(a.fossil, b.fossil, t),
    }
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
    draw_tile_background_impl(
        frame,
        width,
        height,
        board_rect,
        depth_rows,
        seed,
        tile_background_enabled(),
    );
}

fn draw_tile_background_impl(
    frame: &mut dyn Renderer2d,
    width: u32,
    height: u32,
    board_rect: Rect,
    depth_rows: u32,
    seed: u64,
    enabled: bool,
) {
    frame.fill_rect(Rect::from_size(width, height), CLEAR_COLOR);
    if !enabled || board_rect.w == 0 || board_rect.h == 0 {
        return;
    }

    // Draw back-to-front: Far, Mid, Near.
    for layer in &V1_LAYERS {
        if !layer_disabled(layer.layer_id) {
            draw_tile_layer(frame, board_rect, depth_rows, seed, *layer);
        }
    }

    // Optional blend-debug overlay.
    if blend_debug_enabled() {
        draw_blend_debug_overlay(frame, board_rect, depth_rows);
    }
}

// ── Per-layer rendering (Phase 4) ───────────────────────────────────

fn draw_tile_layer(
    frame: &mut dyn Renderer2d,
    board_rect: Rect,
    depth_rows: u32,
    seed: u64,
    layer: BackgroundLayerSpec,
) {
    let tile_px = layer.tile_px.max(1);
    let visible_cols = board_rect.w.div_ceil(tile_px);
    let visible_rows = board_rect.h.div_ceil(tile_px);

    // Apply parallax multiplier override for non-near layers.
    let effective_divisor = if layer.layer_id != BackgroundLayerId::Near && layer.depth_divisor > 1
    {
        let mult = parallax_mult();
        ((layer.depth_divisor as f32) * mult).round().max(1.0) as u32
    } else {
        layer.depth_divisor.max(1)
    };
    let layer_offset = depth_to_background_row_offset(depth_rows) / effective_divisor;
    let board_max_x = board_rect.x.saturating_add(board_rect.w);
    let board_max_y = board_rect.y.saturating_add(board_rect.h);
    let layer_id_u32 = layer.layer_id as u32;

    for tile_y in 0..visible_rows {
        let y = board_rect.y.saturating_add(tile_y.saturating_mul(tile_px));
        if y >= board_max_y {
            continue;
        }
        let h = board_max_y.saturating_sub(y).min(tile_px);
        if h == 0 {
            continue;
        }

        let row_from_bottom = visible_rows.saturating_sub(1).saturating_sub(tile_y);
        let world_depth = layer_offset.saturating_add(row_from_bottom);

        // Compute blend info once per row (all tiles in a row share the same depth).
        let blend = if let Some(forced) = forced_biome() {
            BiomeBlendInfo::pure(forced)
        } else {
            let mut info = biome_blend_at_depth(world_depth);
            if let Some(forced_alpha) = forced_blend_alpha() {
                info.blend_alpha = forced_alpha;
                // When forcing alpha, ensure secondary differs from primary for
                // visibility. If we only have one band, set secondary to Stone.
                if info.primary == info.secondary && forced_alpha > 0 {
                    info.secondary = match info.primary {
                        BackgroundBiome::Dirt => BackgroundBiome::Stone,
                        BackgroundBiome::Stone => BackgroundBiome::Dirt,
                    };
                }
            }
            info
        };

        // Pre-compute the effective palette for this row (blend primary+secondary).
        // Phase 6: this avoids per-tile palette blending when blend_alpha is 0.
        let palette = if blend.blend_alpha == 0 {
            palette_for_biome(blend.primary)
        } else {
            let pa = palette_for_biome(blend.primary);
            let pb = palette_for_biome(blend.secondary);
            lerp_palette(pa, pb, blend.blend_alpha)
        };

        // Choose the biome for tile-kind sampling. For blended rows, use
        // probabilistic selection per-tile (keeps motif variety across the
        // transition window while the palette smoothly interpolates).
        for tile_x in 0..visible_cols {
            let x = board_rect.x.saturating_add(tile_x.saturating_mul(tile_px));
            if x >= board_max_x {
                continue;
            }
            let w = board_max_x.saturating_sub(x).min(tile_px);
            if w == 0 {
                continue;
            }

            let hash = tile_hash(seed, world_depth, tile_x, layer_id_u32);
            let sample_biome = if blend.blend_alpha == 0 {
                blend.primary
            } else {
                // Use hash bits for probabilistic biome selection.
                let roll = (hash >> 8) & 0xFF;
                if roll <= blend.blend_alpha as u32 {
                    blend.secondary
                } else {
                    blend.primary
                }
            };

            let tile_rect = Rect::new(x, y, w, h);
            draw_tile_rect(frame, tile_rect, palette.base, layer.alpha);

            if layer.motif_enabled {
                let kind = BackgroundTileKind::from_hash(hash, sample_biome);
                draw_tile_motif(frame, tile_rect, kind, palette, layer.alpha);
            }

            if grid_overlay_enabled() {
                const GRID_COLOR: Color = [255, 255, 255, 255];
                const GRID_ALPHA: u8 = 30;
                frame.blend_rect(Rect::new(x, y, w, 1), GRID_COLOR, GRID_ALPHA);
                frame.blend_rect(Rect::new(x, y, 1, h), GRID_COLOR, GRID_ALPHA);
            }
        }
    }
}

// ── Tile drawing helpers ────────────────────────────────────────────

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

fn draw_blend_debug_overlay(frame: &mut dyn Renderer2d, board_rect: Rect, depth_rows: u32) {
    // Show current blend info as a small text overlay at the top of the board.
    let blend = biome_blend_at_depth(depth_to_background_row_offset(depth_rows));
    let biome_name = |b: BackgroundBiome| match b {
        BackgroundBiome::Dirt => "DIRT",
        BackgroundBiome::Stone => "STONE",
    };
    let text = if blend.blend_alpha == 0 {
        format!("BG: {} a=0", biome_name(blend.primary))
    } else {
        format!(
            "BG: {}+{} a={}",
            biome_name(blend.primary),
            biome_name(blend.secondary),
            blend.blend_alpha
        )
    };
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
    fn biome_dirt_below_threshold() {
        for d in 0..STONE_DEPTH_START {
            assert_eq!(biome_at_depth(d), BackgroundBiome::Dirt);
        }
    }

    #[test]
    fn biome_stone_at_and_above_threshold() {
        assert_eq!(biome_at_depth(STONE_DEPTH_START), BackgroundBiome::Stone);
        assert_eq!(
            biome_at_depth(STONE_DEPTH_START + 100),
            BackgroundBiome::Stone
        );
        assert_eq!(biome_at_depth(u32::MAX), BackgroundBiome::Stone);
    }

    // -- biome_blend_at_depth ------------------------------------------------

    #[test]
    fn blend_pure_dirt_well_below_boundary() {
        let info = biome_blend_at_depth(0);
        assert_eq!(info.primary, BackgroundBiome::Dirt);
        assert_eq!(info.blend_alpha, 0);
    }

    #[test]
    fn blend_pure_stone_well_above_boundary() {
        let info = biome_blend_at_depth(STONE_DEPTH_START + 100);
        assert_eq!(info.primary, BackgroundBiome::Stone);
        assert_eq!(info.blend_alpha, 0);
    }

    #[test]
    fn blend_nonzero_at_boundary_center() {
        let info = biome_blend_at_depth(STONE_DEPTH_START);
        // At the center of a 10-row band, alpha should be ~128.
        assert!(
            info.blend_alpha > 100 && info.blend_alpha < 200,
            "blend_alpha at center should be roughly half, got {}",
            info.blend_alpha
        );
        assert_eq!(info.primary, BackgroundBiome::Dirt);
        assert_eq!(info.secondary, BackgroundBiome::Stone);
    }

    #[test]
    fn blend_alpha_increases_through_window() {
        let half = BIOME_BLEND_BAND_ROWS / 2;
        let start = STONE_DEPTH_START.saturating_sub(half);
        let end = STONE_DEPTH_START.saturating_add(half);

        let mut prev_alpha = 0u8;
        for d in start..end {
            let info = biome_blend_at_depth(d);
            assert!(
                info.blend_alpha >= prev_alpha,
                "blend_alpha should be monotonically non-decreasing through window: \
                 depth={d}, alpha={}, prev={}",
                info.blend_alpha,
                prev_alpha
            );
            prev_alpha = info.blend_alpha;
        }
        // Alpha should reach a meaningful value by the end of the window.
        assert!(
            prev_alpha > 200,
            "blend_alpha should be close to 255 at end of window, got {prev_alpha}"
        );
    }

    #[test]
    fn blend_no_abrupt_visual_jumps() {
        // Check that the effective blended palette base color never jumps
        // abruptly between consecutive depths. Raw alpha can reset at the
        // window edge (e.g. from 255 to 0), but the visual output is smooth
        // because both sides of the boundary produce the same biome palette.
        let max_channel_jump = 12u16; // per channel, per row

        let effective_base = |d: u32| -> Color {
            let info = biome_blend_at_depth(d);
            if info.blend_alpha == 0 {
                palette_for_biome(info.primary).base
            } else {
                let pa = palette_for_biome(info.primary);
                let pb = palette_for_biome(info.secondary);
                lerp_palette(pa, pb, info.blend_alpha).base
            }
        };

        for d in 1..200 {
            let a = effective_base(d - 1);
            let b = effective_base(d);
            for ch in 0..4 {
                let diff = (b[ch] as i16 - a[ch] as i16).unsigned_abs();
                assert!(
                    diff <= max_channel_jump,
                    "base color channel {} jumped by {} between depth {} and {} ({:?}->{:?})",
                    ch,
                    diff,
                    d - 1,
                    d,
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn blend_with_custom_bands() {
        let bands = &[BiomeBand {
            center_depth: 100,
            half_width: 20,
            biome_below: BackgroundBiome::Dirt,
            biome_above: BackgroundBiome::Stone,
        }];
        let below = biome_blend_at_depth_with_bands(50, bands);
        assert_eq!(below.primary, BackgroundBiome::Dirt);
        assert_eq!(below.blend_alpha, 0);

        let mid = biome_blend_at_depth_with_bands(100, bands);
        assert_eq!(mid.primary, BackgroundBiome::Dirt);
        assert_eq!(mid.secondary, BackgroundBiome::Stone);
        assert!(mid.blend_alpha > 100);

        let above = biome_blend_at_depth_with_bands(200, bands);
        assert_eq!(above.primary, BackgroundBiome::Stone);
        assert_eq!(above.blend_alpha, 0);
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

    // -- tile kind from hash -------------------------------------------------

    #[test]
    fn tile_kind_deterministic() {
        let h = tile_hash(42, 5, 3, 0);
        let a = BackgroundTileKind::from_hash(h, BackgroundBiome::Dirt);
        let b = BackgroundTileKind::from_hash(h, BackgroundBiome::Dirt);
        assert_eq!(a, b);
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

    // -- color blending ------------------------------------------------------

    #[test]
    fn lerp_color_endpoints() {
        let a: Color = [0, 0, 0, 255];
        let b: Color = [255, 255, 255, 255];
        assert_eq!(lerp_color(a, b, 0), a);
        assert_eq!(lerp_color(a, b, 255), b);
    }

    #[test]
    fn lerp_color_midpoint() {
        let a: Color = [0, 0, 0, 255];
        let b: Color = [254, 254, 254, 255];
        let mid = lerp_color(a, b, 128);
        // Should be roughly halfway.
        for i in 0..3 {
            assert!(
                (mid[i] as i16 - 127).abs() < 5,
                "channel {} should be ~127, got {}",
                i,
                mid[i]
            );
        }
    }

    // -- draw_tile_background ------------------------------------------------

    fn draw_background_frame(seed: u64, depth_rows: u32) -> Vec<u8> {
        let width = 480;
        let height = 360;
        let board_rect = Rect::new(120, 0, 240, 360);
        let mut frame = vec![0u8; (width * height * 4) as usize];
        let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
        draw_tile_background_impl(&mut gfx, width, height, board_rect, depth_rows, seed, true);
        frame
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
    fn draw_tile_background_differs_at_biome_boundary() {
        // Compare a frame well in Dirt vs one well in Stone.
        let dirt = draw_background_frame(42, 10);
        let stone = draw_background_frame(42, 80);
        assert_ne!(
            dirt, stone,
            "background should look different between Dirt and Stone depths"
        );
    }

    #[test]
    fn draw_tile_background_blend_transition_is_smooth() {
        // Frames at consecutive depths through the blend window should change
        // gradually (no identical adjacent frames that suddenly jump).
        let half = BIOME_BLEND_BAND_ROWS / 2;
        let start = STONE_DEPTH_START.saturating_sub(half);
        let end = STONE_DEPTH_START.saturating_add(half);

        let mut prev = draw_background_frame(42, start);
        let mut identical_count = 0u32;
        for d in (start + 1)..=end {
            let current = draw_background_frame(42, d);
            if current == prev {
                identical_count += 1;
            }
            prev = current;
        }
        // It's ok if a few adjacent frames happen to be identical (probabilistic
        // sampling can collide), but a majority should differ.
        let window_size = end - start;
        assert!(
            identical_count < window_size / 2,
            "too many identical frames in blend window ({identical_count}/{window_size})"
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
            let biome = biome_with_transition(depth, h);
            let kind = BackgroundTileKind::from_hash(h, biome);

            let h2 = tile_hash(seed, depth, col, layer as u32);
            let biome2 = biome_with_transition(depth, h2);
            let kind2 = BackgroundTileKind::from_hash(h2, biome2);

            assert_eq!(biome, biome2);
            assert_eq!(kind, kind2);
        }
    }
}
