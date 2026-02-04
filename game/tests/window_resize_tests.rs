use engine::graphics::CpuRenderer;
use engine::render::draw_board;
use engine::surface::{RgbaBufferSurface, Surface, SurfaceSize};
use engine::HeadlessRunner;

use game::playtest::TetrisLogic;
use game::tetris_core::Piece;

#[test]
fn headless_surface_resize_changes_buffer_size_and_allows_rendering() {
    let logic = TetrisLogic::new(0, vec![Piece::O]);
    let runner = HeadlessRunner::new(logic);

    let mut surface = RgbaBufferSurface::new(SurfaceSize::new(320, 240));
    assert_eq!(surface.frame().len(), 320 * 240 * 4);

    // Render once at the initial size.
    let board = runner.state().tetris.board_with_active_piece();
    let size = surface.size();
    {
        let mut gfx = CpuRenderer::new(surface.frame_mut(), size);
        draw_board(&mut gfx, &board);
    }
    surface.present().unwrap();

    // Simulate a window resize (headless).
    surface.resize(SurfaceSize::new(800, 600)).unwrap();
    assert_eq!(surface.frame().len(), 800 * 600 * 4);

    // Render again at the new size.
    let board = runner.state().tetris.board_with_active_piece();
    let size = surface.size();
    {
        let mut gfx = CpuRenderer::new(surface.frame_mut(), size);
        draw_board(&mut gfx, &board);
    }
    surface.present().unwrap();
}

