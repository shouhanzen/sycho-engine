use std::time::Duration;

use engine::graphics::{CpuRenderer, Renderer2d};
use engine::surface::SurfaceSize;
use game::debug::DebugHud;

#[test]
fn draw_text_writes_some_pixels() {
    let width = 64u32;
    let height = 32u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let color = [255u8, 255u8, 255u8, 255u8];
    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    gfx.draw_text(0, 0, "FPS", color);

    let any_text_pixel = frame.chunks_exact(4).any(|px| px == color);
    assert!(
        any_text_pixel,
        "expected draw_text to paint at least one pixel"
    );
}

#[test]
fn draw_text_renders_distinct_glyphs_for_distinct_chars() {
    let width = 32u32;
    let height = 16u32;
    let color = [255u8, 255u8, 255u8, 255u8];

    let mut frame_a = vec![0u8; (width * height * 4) as usize];
    let mut frame_b = vec![0u8; (width * height * 4) as usize];

    let mut gfx_a = CpuRenderer::new(&mut frame_a, SurfaceSize::new(width, height));
    gfx_a.draw_text(0, 0, "A", color);
    let mut gfx_b = CpuRenderer::new(&mut frame_b, SurfaceSize::new(width, height));
    gfx_b.draw_text(0, 0, "B", color);

    assert_ne!(
        frame_a, frame_b,
        "expected different pixel output for different characters"
    );
}

#[test]
fn draw_text_clips_without_panicking() {
    let width = 8u32;
    let height = 8u32;
    let mut frame = vec![0u8; (width * height * 4) as usize];

    let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
    gfx.draw_text(
        width.saturating_sub(1),
        height.saturating_sub(1),
        "A",
        [255, 0, 0, 255],
    );
}

#[test]
fn draw_text_renders_brackets_and_dollar_sign() {
    fn render_char(ch: &str) -> Vec<u8> {
        let width = 32u32;
        let height = 16u32;
        let color = [255u8, 255u8, 255u8, 255u8];
        let mut frame = vec![0u8; (width * height * 4) as usize];
        let mut gfx = CpuRenderer::new(&mut frame, SurfaceSize::new(width, height));
        gfx.draw_text(0, 0, ch, color);
        frame
    }

    let fallback = render_char("?");
    assert_ne!(
        render_char("$"),
        fallback,
        "expected $ to render a distinct glyph"
    );
    assert_ne!(
        render_char("["),
        fallback,
        "expected [ to render a distinct glyph"
    );
    assert_ne!(
        render_char("]"),
        fallback,
        "expected ] to render a distinct glyph"
    );
}

#[test]
fn debug_hud_emits_lines_with_expected_labels() {
    let mut hud = DebugHud::new();
    hud.record_input(Duration::from_micros(250));
    hud.record_gravity(Duration::from_micros(500));
    hud.on_frame(
        Duration::from_millis(16),
        Duration::from_millis(1),
        Duration::from_millis(2),
        Duration::from_millis(1),
        Duration::from_millis(3),
        Duration::from_millis(7),
    );

    let lines = hud.lines();
    assert!(
        lines.iter().any(|l| l.contains("FPS")),
        "expected an FPS line, got: {lines:?}"
    );

    let lowercase: Vec<String> = lines.iter().map(|l| l.to_ascii_lowercase()).collect();
    assert!(
        lowercase.iter().any(|l| l.contains("input")),
        "expected an input line, got: {lines:?}"
    );
    assert!(
        lowercase.iter().any(|l| l.contains("present")),
        "expected a present line, got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("BUD FRAME")),
        "expected a budget frame line, got: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.starts_with("BUD NOW")),
        "expected a budget summary line, got: {lines:?}"
    );
}

#[test]
fn debug_hud_minimize_collapses_overlay_lines() {
    let mut hud = DebugHud::new();
    let expanded = hud.overlay_lines();
    assert!(
        expanded.len() > 1,
        "expected expanded overlay to include multiple lines"
    );

    hud.toggle_minimized();
    let minimized = hud.overlay_lines();
    assert_eq!(
        minimized.len(),
        1,
        "expected minimized overlay to render a single header line"
    );
    assert!(
        minimized[0].contains("DEBUG"),
        "expected minimized header to label the debug overlay"
    );

    hud.toggle_minimized();
    let expanded_again = hud.overlay_lines();
    assert!(
        expanded_again.len() > 1,
        "expected overlay to expand after toggling again"
    );
}

#[test]
fn debug_hud_exposes_timer_toggle_line() {
    let hud = DebugHud::new();
    let lines = hud.overlay_lines();
    assert!(
        lines.iter().any(|l| l.starts_with("TIMER ON [CLICK]")),
        "expected timer toggle line in overlay, got: {lines:?}"
    );
}

#[test]
fn clicking_timer_line_toggles_round_timer_override() {
    let mut hud = DebugHud::new();
    let width = 640;
    let height = 360;

    assert!(!hud.round_timer_disabled());
    let rect = hud
        .timer_toggle_rect(width, height)
        .expect("expected timer toggle rect when expanded");
    assert!(hud.handle_click(
        rect.x.saturating_add(1),
        rect.y.saturating_add(1),
        width,
        height
    ));
    assert!(hud.round_timer_disabled());

    let rect = hud
        .timer_toggle_rect(width, height)
        .expect("expected timer toggle rect after first toggle");
    assert!(hud.handle_click(
        rect.x.saturating_add(1),
        rect.y.saturating_add(1),
        width,
        height
    ));
    assert!(!hud.round_timer_disabled());
}
