/// Tile-based background system.
///
/// Generates deterministic tile patterns behind the board that scroll as
/// the player digs deeper. Visual only — no gameplay collision.
use std::sync::OnceLock;

use engine::graphics::{Color, Renderer2d};
use engine::render::CELL_SIZE;
use engine::ui::Rect;

/// Background biome bands (v1: Dirt and Stone only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundBiome {
    Dirt,
    Stone,
}

/// Hard depth threshold separating Dirt from Stone.
const STONE_DEPTH_START: u32 = 50;
/// Rows around the threshold where Dirt/Stone are blended deterministically.
const BIOME_BLEND_BAND_ROWS: u32 = 10;
const CLEAR_COLOR: Color = [4, 6, 10, 255];
const BACKGROUND_DISABLE_ENV: &str = "ROLLOUT_DISABLE_TILE_BG";

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

/// Return the biome for a given depth row.
///
/// Base lookup keeps a hard switch at `STONE_DEPTH_START` for callers that
/// do not want transitional blending.
pub fn biome_at_depth(depth: u32) -> BackgroundBiome {
    if depth < STONE_DEPTH_START {
        BackgroundBiome::Dirt
    } else {
        BackgroundBiome::Stone
    }
}

/// Map gameplay depth rows to background world-row offset.
///
/// MVP uses a 1:1 mapping. Keeping the conversion in one helper makes it easy
/// to tune parallax scaling later without changing call sites.
pub fn depth_to_background_row_offset(depth_rows: u32) -> u32 {
    depth_rows
}

/// Visual tile kinds used for procedural motif drawing (v1).
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
    ///
    /// The weights are intentionally simple for v1: most tiles are plain fill
    /// or simple motifs, with rare accent tiles at low probability.
    pub fn from_hash(hash: u32, biome: BackgroundBiome) -> Self {
        // Use the low 8 bits as a 0..255 roll.
        let roll = (hash & 0xFF) as u8;

        match biome {
            // Dirt: mostly empty/soft patterns, occasional pebble/crack.
            //   0..159  Empty        (62.5 %)
            //  160..209  DirtA       (19.5 %)
            //  210..239  DirtB       (11.7 %)
            //  240..249  Crack       ( 3.9 %)
            //  250..255  Fossil      ( 2.3 %)
            BackgroundBiome::Dirt => match roll {
                0..160 => BackgroundTileKind::Empty,
                160..210 => BackgroundTileKind::DirtA,
                210..240 => BackgroundTileKind::DirtB,
                240..250 => BackgroundTileKind::Crack,
                _ => BackgroundTileKind::Fossil,
            },
            // Stone: denser rock motifs, fewer bright accents.
            //   0..99   Empty        (39.1 %)
            //  100..159  RockA       (23.4 %)
            //  160..209  RockB       (19.5 %)
            //  210..234  Vein        ( 9.8 %)
            //  235..249  Crack       ( 5.9 %)
            //  250..255  Fossil      ( 2.3 %)
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

#[derive(Debug, Clone, Copy)]
struct LayerConfig {
    layer_id: u32,
    tile_px: u32,
    alpha: u8,
    depth_divisor: u32,
}

const BASE_LAYER: LayerConfig = LayerConfig {
    layer_id: 0,
    tile_px: CELL_SIZE,
    alpha: 255,
    depth_divisor: 1,
};

const PARALLAX_LAYER: LayerConfig = LayerConfig {
    layer_id: 1,
    tile_px: CELL_SIZE * 2,
    alpha: 108,
    depth_divisor: 2,
};

/// Environment-controlled kill switch for quick debugging.
///
/// Set `ROLLOUT_DISABLE_TILE_BG=1` to disable the tile background.
pub fn tile_background_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| !env_flag(BACKGROUND_DISABLE_ENV))
}

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

    // Draw back-to-front: parallax first, then primary layer.
    draw_tile_layer(frame, board_rect, depth_rows, seed, PARALLAX_LAYER);
    draw_tile_layer(frame, board_rect, depth_rows, seed, BASE_LAYER);
}

fn draw_tile_layer(
    frame: &mut dyn Renderer2d,
    board_rect: Rect,
    depth_rows: u32,
    seed: u64,
    layer: LayerConfig,
) {
    let tile_px = layer.tile_px.max(1);
    let visible_cols = board_rect.w.div_ceil(tile_px);
    let visible_rows = board_rect.h.div_ceil(tile_px);
    let layer_offset = depth_to_background_row_offset(depth_rows) / layer.depth_divisor.max(1);
    let board_max_x = board_rect.x.saturating_add(board_rect.w);
    let board_max_y = board_rect.y.saturating_add(board_rect.h);

    for tile_y in 0..visible_rows {
        let y = board_rect.y.saturating_add(tile_y.saturating_mul(tile_px));
        if y >= board_max_y {
            continue;
        }
        let h = board_max_y.saturating_sub(y).min(tile_px);
        if h == 0 {
            continue;
        }

        // Depth grows as we dig down, so increasing offset should scroll motifs downward.
        let row_from_bottom = visible_rows.saturating_sub(1).saturating_sub(tile_y);
        let world_depth = layer_offset.saturating_add(row_from_bottom);

        for tile_x in 0..visible_cols {
            let x = board_rect.x.saturating_add(tile_x.saturating_mul(tile_px));
            if x >= board_max_x {
                continue;
            }
            let w = board_max_x.saturating_sub(x).min(tile_px);
            if w == 0 {
                continue;
            }

            let hash = tile_hash(seed, world_depth, tile_x, layer.layer_id);
            let biome = biome_with_transition(world_depth, hash);
            let palette = palette_for_biome(biome);
            let kind = BackgroundTileKind::from_hash(hash, biome);
            let tile_rect = Rect::new(x, y, w, h);

            draw_tile_rect(frame, tile_rect, palette.base, layer.alpha);
            draw_tile_motif(frame, tile_rect, kind, palette, layer.alpha);
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

fn biome_with_transition(depth: u32, hash: u32) -> BackgroundBiome {
    let half_band = BIOME_BLEND_BAND_ROWS / 2;
    let band_start = STONE_DEPTH_START.saturating_sub(half_band);
    let band_end = STONE_DEPTH_START.saturating_add(half_band);

    if depth < band_start {
        return BackgroundBiome::Dirt;
    }
    if depth >= band_end {
        return BackgroundBiome::Stone;
    }

    let width = band_end.saturating_sub(band_start).max(1);
    let pos = depth.saturating_sub(band_start);
    let stone_weight_255 = (pos.saturating_mul(255)) / width;
    let roll = (hash >> 8) & 0xFF;
    if roll <= stone_weight_255 {
        BackgroundBiome::Stone
    } else {
        BackgroundBiome::Dirt
    }
}

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
}
