use std::{error::Error, path::Path, sync::Arc, time::Duration};

use engine::{
    app::{AppConfig, AppContext, GameApp, TimelineGame, run_game},
    embedded::valid_logical_size,
    graphics::{FittedRenderer, ImageRect, Renderer2d},
    input::{InputFrame, VirtualKeyCode as Key},
    launch::{parse_timeline_cli, project_timeline},
    surface::SurfaceSize,
    tiles::{Cell, GridSize, TileGrid, TileLevel, TilePalette},
    tilesets::Tilesets,
    timeline::StateSchema,
    ui::Rect,
};
use serde::{Deserialize, Serialize};

const PALETTE: TilePalette = TilePalette {
    empty: [19, 28, 39, 255],
    solid: [52, 100, 91, 255],
};
const BODY: [f64; 2] = [0.72, 0.9];
const SPAWN: [f64; 2] = [2.0, 12.0];
const TILE_POINTS: f64 = 20.0;
const EPSILON: f64 = 1e-7;

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Level {
    Courtyard,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "SavedState")]
struct State {
    level: Level,
    position: [f64; 2],
    velocity: [f64; 2],
    grounded: bool,
    alternate_color: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewport: Option<SurfaceSize>,
}

#[derive(Deserialize)]
struct SavedState {
    level: Option<Level>,
    position: [f64; 2],
    #[serde(default)]
    velocity: [f64; 2],
    #[serde(default)]
    grounded: bool,
    #[serde(default)]
    alternate_color: bool,
    #[serde(default)]
    viewport: Option<SurfaceSize>,
}

impl From<SavedState> for State {
    fn from(saved: SavedState) -> Self {
        let legacy = saved.level.is_none();
        Self {
            level: saved.level.unwrap_or(Level::Courtyard),
            // The original starter used pixel positions in its preview.
            position: if legacy {
                saved.position.map(|value| value / 20.0)
            } else {
                saved.position
            },
            velocity: saved.velocity,
            grounded: saved.grounded,
            alternate_color: saved.alternate_color,
            viewport: saved.viewport,
        }
    }
}

#[derive(Default)]
struct Game {
    grid: Option<Arc<TileGrid>>,
    tilesets: Arc<Tilesets>,
}

struct Preview {
    state: Arc<State>,
    grid: Option<Arc<TileGrid>>,
    tilesets: Arc<Tilesets>,
}

fn courtyard() -> TileGrid {
    TileGrid::new(32, 18, |Cell { x, y }| {
        y >= 16
            || (y == 13 && (7..12).contains(&x))
            || (y == 11 && (14..20).contains(&x))
            || (y == 13 && (21..27).contains(&x))
            || ((14..16).contains(&y) && x == 29)
    })
}

struct Camera {
    origin: [f64; 2],
    viewport: [u32; 2],
    extent: [u16; 2],
}

impl Camera {
    fn new(surface: SurfaceSize, size: GridSize, position: [f64; 2]) -> Self {
        let viewport = [surface.width, surface.height];
        let extent = [size.width, size.height];
        let origin = std::array::from_fn(|axis| {
            let visible = f64::from(viewport[axis]) / TILE_POINTS;
            let far_edge = f64::from(extent[axis]) - visible;
            if far_edge <= 0.0 {
                far_edge / 2.0
            } else {
                (position[axis] + BODY[axis] / 2.0 - visible / 2.0).clamp(0.0, far_edge)
            }
        });
        Self {
            origin,
            viewport,
            extent,
        }
    }

    fn visible_tiles(&self, axis: usize) -> std::ops::Range<u16> {
        let start = (self.origin[axis].floor() as u16).min(self.extent[axis]);
        let end = ((self.origin[axis] + f64::from(self.viewport[axis]) / TILE_POINTS).ceil()
            as u16)
            .min(self.extent[axis]);
        start..end
    }

    fn tile_rect(&self, x: u16, y: u16) -> ImageRect {
        let at = |axis: usize, cell: u16| {
            ((f64::from(cell) - self.origin[axis]) * TILE_POINTS).round() as i32
        };
        let (left, top) = (at(0, x), at(1, y));
        ImageRect::new(
            left,
            top,
            (at(0, x + 1) - left) as u32,
            (at(1, y + 1) - top) as u32,
        )
    }

    fn rect(&self, position: [f64; 2], size: [f64; 2]) -> Option<Rect> {
        let edge = |axis: usize, coordinate: f64| {
            let world = coordinate.clamp(0.0, f64::from(self.extent[axis]));
            (((world - self.origin[axis]) * TILE_POINTS).round() as u32).min(self.viewport[axis])
        };
        let left = edge(0, position[0]);
        let top = edge(1, position[1]);
        let right = edge(0, position[0] + size[0]);
        let bottom = edge(1, position[1] + size[1]);
        (left < right && top < bottom).then(|| Rect::new(left, top, right - left, bottom - top))
    }
}

fn draw(
    state: &State,
    grid: Option<&TileGrid>,
    tilesets: &Tilesets,
    renderer: &mut dyn Renderer2d,
) {
    renderer.clear(PALETTE.empty);
    let Some(grid) = grid else { return };
    if let Some(viewport) = state.viewport.filter(|&size| valid_logical_size(size))
        && viewport != renderer.size()
    {
        draw_level(
            state,
            grid,
            tilesets,
            &mut FittedRenderer::new(renderer, viewport),
        );
    } else {
        draw_level(state, grid, tilesets, renderer);
    }
}

fn draw_level(state: &State, grid: &TileGrid, tilesets: &Tilesets, renderer: &mut dyn Renderer2d) {
    let size = renderer.size();
    let camera = Camera::new(size, grid.size(), state.position);
    let clip = Rect::from_size(size.width, size.height);
    for y in camera.visible_tiles(1) {
        for x in camera.visible_tiles(0) {
            let cell = Cell::new(x, y);
            if let Some(tile) = grid.sprite(cell) {
                let (image, source) = tilesets.sprite(tile);
                renderer.draw_image(image, source, camera.tile_rect(x, y), clip);
            } else if grid.solid(cell)
                && let Some(rect) = camera.rect([f64::from(x), f64::from(y)], [1.0, 1.0])
            {
                renderer.fill_rect(rect, PALETTE.solid);
            }
        }
    }
    let color = if state.alternate_color {
        [246, 177, 96, 255]
    } else {
        [235, 220, 151, 255]
    };
    if let Some(rect) = camera.rect(state.position, BODY) {
        renderer.fill_rect(rect, color);
    }
}

fn cells(start: f64, end: f64, limit: u16) -> std::ops::RangeInclusive<u16> {
    let last = limit - 1;
    ((start + EPSILON).floor() as u16).min(last)..=((end - EPSILON).floor() as u16).min(last)
}

fn clear(grid: &TileGrid, position: [f64; 2]) -> bool {
    cells(position[1], position[1] + BODY[1], grid.height()).all(|y| {
        cells(position[0], position[0] + BODY[0], grid.width())
            .all(|x| !grid.solid(Cell::new(x, y)))
    })
}

/// Sweep the body's leading edge across every crossed cell and stop at the first face.
fn sweep(grid: &TileGrid, position: [f64; 2], axis: usize, delta: f64) -> f64 {
    let across = 1 - axis;
    let limits = [grid.width(), grid.height()];
    let from = position[axis];
    let wanted = (from + delta).clamp(0.0, f64::from(limits[axis]) - BODY[axis]);
    let mut result = wanted;
    for along in cells(
        from.min(wanted),
        from.max(wanted) + BODY[axis],
        limits[axis],
    ) {
        for side in cells(
            position[across],
            position[across] + BODY[across],
            limits[across],
        ) {
            let cell = if axis == 0 {
                Cell::new(along, side)
            } else {
                Cell::new(side, along)
            };
            if !grid.solid(cell) {
                continue;
            }
            let face = f64::from(along);
            if wanted > from && face >= from + BODY[axis] - EPSILON {
                result = result.min(face - BODY[axis]);
            } else if wanted < from && face + 1.0 <= from + EPSILON {
                result = result.max(face + 1.0);
            }
        }
    }
    result
}

fn recover(grid: &TileGrid, position: [f64; 2]) -> Option<[f64; 2]> {
    let clamped = [
        position[0].clamp(0.0, f64::from(grid.width()) - BODY[0]),
        position[1].clamp(0.0, f64::from(grid.height()) - BODY[1]),
    ];
    if clear(grid, clamped) {
        return Some(clamped);
    }
    // Cells touched by an edit can overlap the historical pose. Resolve on the next live tick.
    (0..grid.height())
        .flat_map(|y| (0..grid.width()).map(move |x| Cell::new(x, y)))
        .filter(|&cell| !grid.solid(cell))
        .map(|cell| [f64::from(cell.x), f64::from(cell.y)])
        .min_by(|a, b| {
            let distance = |p: &[f64; 2]| (p[0] - clamped[0]).powi(2) + (p[1] - clamped[1]).powi(2);
            distance(a).total_cmp(&distance(b))
        })
}

impl GameApp for Game {
    type State = State;
    const LOGICAL_DISPLAY: bool = true;

    fn init_state(&mut self, ctx: &mut AppContext) -> State {
        State {
            level: Level::Courtyard,
            position: SPAWN,
            velocity: [0.0; 2],
            grounded: false,
            alternate_color: false,
            viewport: Some(ctx.display().logical_size),
        }
    }

    fn update_state(
        &mut self,
        state: &mut State,
        input: &InputFrame,
        dt: Duration,
        ctx: &mut AppContext,
    ) {
        state.viewport = Some(ctx.display().logical_size);
        let Some(grid) = &self.grid else { return };
        let Some(start) = recover(grid, state.position) else {
            state.velocity = [0.0; 2];
            state.grounded = false;
            return;
        };
        if start != state.position {
            state.position = start;
            state.velocity = [0.0; 2];
            state.grounded = false;
        }
        let dt = dt.as_secs_f64().min(0.05);
        let left = input.keys_down.contains(&Key::Left) || input.keys_down.contains(&Key::A);
        let right = input.keys_down.contains(&Key::Right) || input.keys_down.contains(&Key::D);
        state.velocity[0] = f64::from(i32::from(right) - i32::from(left)) * 7.5;
        let jump = [Key::Space, Key::Up, Key::W]
            .iter()
            .any(|key| input.keys_pressed.contains(key));
        if state.grounded && jump {
            state.velocity[1] = -12.0;
        }
        state.velocity[1] = (state.velocity[1] + 29.0 * dt).min(24.0);
        for axis in 0..2 {
            let delta = state.velocity[axis] * dt;
            let position = sweep(grid, state.position, axis, delta);
            if (position - state.position[axis] - delta).abs() > EPSILON {
                state.velocity[axis] = 0.0;
            }
            state.position[axis] = position;
        }
        state.grounded = state.velocity[1] >= 0.0
            && (sweep(grid, state.position, 1, 0.001) - state.position[1]).abs() < EPSILON;
    }

    fn render(&mut self, state: &State, _: bool, renderer: &mut dyn Renderer2d) {
        draw(state, self.grid.as_deref(), &self.tilesets, renderer);
    }
}

impl TimelineGame for Game {
    type Snapshot = State;
    type PreviewSnapshot = Preview;
    const SCHEMA: StateSchema = StateSchema {
        game: env!("CARGO_PKG_NAME"),
        version: 1,
    };

    fn snapshot(state: &State) -> &State {
        state
    }
    fn restore_state(&mut self, snapshot: State) -> State {
        snapshot
    }

    fn tile_level(state: &State) -> Option<TileLevel> {
        let (id, build) = match state.level {
            Level::Courtyard => ("courtyard", courtyard as fn() -> TileGrid),
        };
        Some(TileLevel {
            id,
            project: Path::new(env!("CARGO_MANIFEST_DIR")),
            binary: env!("CARGO_BIN_NAME"),
            build,
            palette: PALETTE,
        })
    }

    fn set_tile_grid(&mut self, grid: Option<Arc<TileGrid>>) {
        self.grid = grid;
    }

    fn set_tilesets(&mut self, tilesets: Arc<Tilesets>) {
        self.tilesets = tilesets;
    }

    fn preview_snapshot(&self, state: Arc<State>, _: bool) -> Preview {
        Preview {
            state,
            grid: self.grid.clone(),
            tilesets: Arc::clone(&self.tilesets),
        }
    }

    fn draw_preview(preview: Preview, renderer: &mut dyn Renderer2d) {
        draw(
            &preview.state,
            preview.grid.as_deref(),
            &preview.tilesets,
            renderer,
        );
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let name = env!("CARGO_PKG_NAME");
    let cli = parse_timeline_cli()?;
    if cli.help {
        println!("Move with the arrow keys or A/D. Jump with Space, Up, or W.");
        return Ok(());
    }
    if cli.check_launch {
        return Ok(());
    }
    let config = AppConfig {
        title: name.into(),
        desired_size: SurfaceSize::new(640, 400),
        clamp_to_monitor: true,
    };
    run_game(
        config,
        Game::default(),
        cli.run_config(project_timeline(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            name,
        ))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_ground_cells_render_mixed_atlases_and_block_movement_until_erased() {
        use engine::graphics::CpuRenderer;
        use engine::tiles::{LevelBase, TileEdit};
        use engine::tilesets::{AssetWatch, AtlasRef, TileRef};

        let project =
            std::env::temp_dir().join(format!("sycho-platformer-ground-{}", std::process::id()));
        let asset_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("levels");
        let reference = |atlas: &str| AtlasRef::File {
            atlas_path: format!("../assets/tiles/{atlas}.atlas.json").into(),
        };
        let base = LevelBase {
            id: "painted".into(),
            palette: PALETTE,
            grid: TileGrid::new(8, 8, |_| false),
        };
        let masonry = Cell::new(2, 4);
        let terrain = Cell::new(3, 4);
        for (cell, atlas) in [(masonry, "masonry"), (terrain, "terrain")] {
            base.save_edits(
                &project,
                &[TileEdit {
                    cell,
                    solid: true,
                    sprite: Some(TileRef {
                        source: reference(atlas),
                        tile: Cell::new(0, 0),
                    }),
                }],
            )
            .unwrap();
        }
        let grid = base.resolve(&project).unwrap();
        assert_eq!(grid.sprite(masonry).unwrap().source, reference("masonry"));
        assert_eq!(grid.sprite(terrain).unwrap().source, reference("terrain"));
        let mut watch = AssetWatch::default();
        let sources = grid.atlas_sources();
        watch.watch(
            sources
                .iter()
                .map(|source| source.key(&asset_directory))
                .collect(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !watch.poll() {
            assert!(std::time::Instant::now() < deadline, "tiles did not load");
            std::thread::sleep(Duration::from_millis(10));
        }
        let assets = Tilesets::from_watch(&asset_directory, &sources, &watch);
        assert!(assets.errors.is_empty(), "{:?}", assets.errors);
        assert!((sweep(&grid, [2.0, 2.0], 1, 2.0) - (4.0 - BODY[1])).abs() < EPSILON);

        let size = SurfaceSize::new(160, 160);
        let mut game = Game::default();
        let mut state = game.init_state(&mut AppContext::for_pixels(size));
        state.position = [0.0, 0.0];
        let sample = |grid: &TileGrid, cell: Cell| {
            let mut frame = vec![0; size.rgba_len()];
            draw(
                &state,
                Some(grid),
                &assets,
                &mut CpuRenderer::new(&mut frame, size),
            );
            let offset =
                ((usize::from(cell.y) * 20 + 10) * 160 + usize::from(cell.x) * 20 + 10) * 4;
            <[u8; 4]>::try_from(&frame[offset..offset + 4]).unwrap()
        };
        let stone = sample(&grid, masonry);
        let soil = sample(&grid, terrain);
        assert_ne!(stone, soil);
        for pixel in [stone, soil] {
            assert_ne!(pixel, PALETTE.empty);
            assert_ne!(pixel, PALETTE.solid);
        }

        let swapped = base
            .save_edits(
                &project,
                &[TileEdit {
                    cell: masonry,
                    solid: true,
                    sprite: Some(TileRef {
                        source: reference("masonry"),
                        tile: Cell::new(1, 0),
                    }),
                }],
            )
            .unwrap();
        assert_ne!(sample(&swapped, masonry), stone);
        assert!(swapped.solid(masonry));
        assert_eq!(sample(&swapped, terrain), soil);

        base.save_edits(&project, &[TileEdit::plain(masonry, false)])
            .unwrap();
        let grid = base.resolve(&project).unwrap();
        assert!(grid.sprite(masonry).is_none());
        assert_eq!(sample(&grid, masonry), PALETTE.empty);
        assert_eq!(sample(&grid, terrain), soil);
        assert_eq!(sweep(&grid, [2.0, 2.0], 1, 2.0), 4.0);
        std::fs::remove_dir_all(project).unwrap();
    }

    #[test]
    fn recorded_viewport_preserves_camera_framing_at_retina_resolution() {
        use engine::graphics::CpuRenderer;
        use engine::surface::DisplayMetrics;

        let logical = SurfaceSize::new(400, 240);
        let retina = SurfaceSize::new(800, 480);
        let mut game = Game::default();
        let mut ctx = AppContext::new(DisplayMetrics::for_surface(retina, 2.0));
        let mut state = game.init_state(&mut ctx);
        state.position = [23.0, 14.0];
        assert_eq!(state.viewport, Some(logical));
        let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        let grid = TileGrid::new(48, 28, |cell| cell == Cell::new(24, 14));
        let frame = |size: SurfaceSize| {
            let mut pixels = vec![0; size.rgba_len()];
            draw(
                &restored,
                Some(&grid),
                &Tilesets::default(),
                &mut CpuRenderer::new(&mut pixels, size),
            );
            pixels
        };
        let normal = frame(logical);
        let doubled = frame(retina);
        let tiles = |buffer: &[u8]| {
            buffer
                .chunks_exact(4)
                .filter(|pixel| *pixel == PALETTE.solid)
                .count()
        };
        assert_eq!(tiles(&normal), 20 * 20);
        assert_eq!(tiles(&doubled), 40 * 40);
        for (i, pixel) in normal.chunks_exact(4).enumerate() {
            let x = (i % logical.width as usize) * 2;
            let y = (i / logical.width as usize) * 2;
            let start = (y * retina.width as usize + x) * 4;
            assert_eq!(pixel, &doubled[start..start + 4]);
        }
    }

    #[test]
    fn camera_follows_the_player_clamps_to_edges_and_centers_small_levels() {
        let surface = SurfaceSize::new(400, 240);
        let large = GridSize::new(60, 32);
        let start = Camera::new(surface, large, [1.0, 2.0]);
        assert_eq!(start.origin, [0.0, 0.0]);
        let following = Camera::new(surface, large, [25.0 - BODY[0] / 2.0, 13.0 - BODY[1] / 2.0]);
        assert_eq!(following.origin, [15.0, 7.0]);
        let end = Camera::new(surface, large, [120.0, 90.0]);
        assert_eq!(end.origin, [40.0, 20.0]);
        assert_eq!(end.visible_tiles(0), 40..60);
        assert!(end.rect([61.0, 20.0], [1.0, 1.0]).is_none());

        let small = Camera::new(surface, GridSize::new(8, 6), [2.0, 2.0]);
        assert_eq!(small.origin, [-6.0, -3.0]);
        assert_eq!(
            small.rect([0.0, 0.0], [1.0, 1.0]),
            Some(Rect::new(120, 60, 20, 20))
        );
        assert_eq!(
            start.rect([0.0, 0.0], [1.0, 1.0]),
            Some(Rect::new(0, 0, 20, 20))
        );
    }

    #[test]
    fn sweep_stops_at_first_wall_and_at_the_floor() {
        let grid = TileGrid::new(9, 8, |Cell { x, y }| x == 5 || y == 7);
        assert!((sweep(&grid, [1.0, 3.0], 0, 20.0) - (5.0 - BODY[0])).abs() < EPSILON);
        assert!((sweep(&grid, [2.0, 1.0], 1, 20.0) - (7.0 - BODY[1])).abs() < EPSILON);
        assert_eq!(sweep(&grid, [7.0, 2.0], 0, -7.0), 6.0);
        assert_eq!(recover(&grid, [5.0, 3.0]), Some([4.0, 3.0]));
    }

    #[test]
    fn reads_starter_recordings_and_records_only_changing_state() {
        let state: State =
            serde_json::from_str(r#"{"position":[40,60],"alternate_color":true}"#).unwrap();
        assert_eq!(state.position, [2.0, 3.0]);
        assert!(state.alternate_color);
        let serialized = serde_json::to_string(&state).unwrap();
        assert!(serialized.contains("courtyard"));
        assert!(!serialized.contains("cells"));
    }

    #[test]
    fn a_grounded_player_can_walk_and_jump() {
        let mut game = Game::default();
        game.set_tile_grid(Some(Arc::new(courtyard())));
        let mut context = AppContext::for_pixels(SurfaceSize::new(640, 400));
        let mut state = game.init_state(&mut context);
        state.position = [2.0, 16.0 - BODY[1]];
        let dt = Duration::from_millis(16);
        game.update_state(&mut state, &InputFrame::default(), dt, &mut context);
        assert!(state.grounded);
        let mut input = InputFrame::default();
        input.keys_down.insert(Key::Right);
        input.keys_pressed.insert(Key::Space);
        game.update_state(&mut state, &input, dt, &mut context);
        assert!(state.position[0] > 2.0);
        assert!(state.position[1] < 16.0 - BODY[1]);
        assert!(!state.grounded);
    }
}
