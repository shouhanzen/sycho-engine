use super::*;
use game::tetris_core::{BOARD_HEIGHT, BOARD_WIDTH, CELL_GARBAGE, Piece, TetrisCore, Vec2i};

fn make_test_headful(view: GameView) -> (HeadfulApp, HeadlessRunner<TetrisLogic>) {
    let base_logic = TetrisLogic::new(0, Piece::all()).with_bottomwell(true);
    let mut app = HeadfulApp::new(
        base_logic.clone(),
        DEFAULT_ROUND_LIMIT,
        DEFAULT_GRAVITY_INTERVAL,
    );
    app.sfx = None;
    app.dig_camera = DigCameraController::new_with_config(true, DigCameraConfig::default());

    let mut runner = HeadlessRunner::new(base_logic);
    {
        let state = runner.state_mut();
        state.view = view;
        state.skilltree = SkillTreeRuntime::load_default();
        state.round_timer = RoundTimer::new(DEFAULT_ROUND_LIMIT);
        state.gravity_interval = DEFAULT_GRAVITY_INTERVAL;
        state.gravity_elapsed = Duration::ZERO;
    }

    (app, runner)
}

fn assert_approx_eq(actual: f32, expected: f32) {
    let delta = (actual - expected).abs();
    assert!(
        delta < 0.001,
        "expected ~{expected}, got {actual} (delta={delta})"
    );
}

fn force_depth_reveal(core: &mut TetrisCore) {
    let target_y = 0;
    let depth_before = core.background_depth_rows();
    for x in 0..BOARD_WIDTH {
        core.set_cell(x, target_y, CELL_GARBAGE);
    }
    let cleared = core.clear_lines();
    assert!(cleared >= 1, "expected at least one cleared line");
    assert!(
        core.background_depth_rows() > depth_before,
        "expected background depth to increase"
    );
}

fn setup_grounded_lock_delay_case(runner: &mut HeadlessRunner<TetrisLogic>) {
    let state = runner.state_mut();
    state.view = GameView::Tetris { paused: false };
    state.gravity_interval = Duration::from_millis(100);
    state.gravity_elapsed = Duration::ZERO;
    state.tetris.set_available_pieces(vec![Piece::O]);
    for y in 0..BOARD_HEIGHT {
        for x in 0..BOARD_WIDTH {
            state.tetris.set_cell(x, y, 0);
        }
    }
    state
        .tetris
        .set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
}

fn input_frame_for_keys(
    pressed: &[VirtualKeyCode],
    down: &[VirtualKeyCode],
    released: &[VirtualKeyCode],
) -> InputFrame {
    let mut input = InputFrame::default();
    for &key in down {
        input.keys_down.insert(key);
    }
    for &key in pressed {
        input.keys_pressed.insert(key);
    }
    for &key in released {
        input.keys_released.insert(key);
    }
    input
}

#[test]
fn key_a_maps_to_rotate_180() {
    assert_eq!(
        map_key_to_action(VirtualKeyCode::A),
        Some(InputAction::Rotate180)
    );
}

#[test]
fn left_arrow_maps_to_move_left() {
    assert_eq!(
        map_key_to_action(VirtualKeyCode::Left),
        Some(InputAction::MoveLeft)
    );
}

#[test]
fn only_hard_drop_plays_gameplay_action_sfx() {
    for action in [
        InputAction::Noop,
        InputAction::MoveLeft,
        InputAction::MoveRight,
        InputAction::SoftDrop,
        InputAction::RotateCw,
        InputAction::RotateCcw,
        InputAction::Rotate180,
        InputAction::Hold,
    ] {
        assert!(
            !should_play_action_sfx(action),
            "expected no action sfx for {action:?}"
        );
    }

    assert!(should_play_action_sfx(InputAction::HardDrop));
}

#[test]
fn horizontal_repeat_ignores_os_repeat_pressed_events_and_repeats_on_timer() {
    let mut repeat = HorizontalRepeat::default();
    let t0 = Instant::now();

    // Initial press should be accepted.
    assert!(repeat.on_press(HorizontalDir::Left, t0));

    // A repeated "Pressed" event (OS key-repeat) should be ignored and not reset the timer.
    assert!(!repeat.on_press(HorizontalDir::Left, t0 + Duration::from_millis(10)));

    // Before the repeat delay expires: no repeat.
    assert_eq!(
        repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY - Duration::from_millis(1)),
        None
    );

    // Once the delay expires: we should get a move action even without any further key events.
    assert_eq!(
        repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY),
        Some(InputAction::MoveLeft)
    );

    // And again at the interval.
    assert_eq!(
        repeat.next_repeat_action(
            t0 + HorizontalRepeat::REPEAT_DELAY + HorizontalRepeat::REPEAT_INTERVAL
        ),
        Some(InputAction::MoveLeft)
    );
}

#[test]
fn keyboard_frame_flow_preserves_menu_pause_and_game_over_keys() {
    let (mut app, mut runner) = make_test_headful(GameView::MainMenu);
    let now = Instant::now();

    app.process_keyboard_frame(
        &mut runner,
        &input_frame_for_keys(&[VirtualKeyCode::Return], &[VirtualKeyCode::Return], &[]),
        now,
    );
    assert!(matches!(
        runner.state().view,
        GameView::Tetris { paused: false }
    ));

    app.process_keyboard_frame(
        &mut runner,
        &input_frame_for_keys(&[VirtualKeyCode::Escape], &[VirtualKeyCode::Escape], &[]),
        now + Duration::from_millis(1),
    );
    assert!(matches!(
        runner.state().view,
        GameView::Tetris { paused: true }
    ));

    app.process_keyboard_frame(
        &mut runner,
        &input_frame_for_keys(&[VirtualKeyCode::Escape], &[VirtualKeyCode::Escape], &[]),
        now + Duration::from_millis(2),
    );
    assert!(matches!(
        runner.state().view,
        GameView::Tetris { paused: false }
    ));

    runner.state_mut().view = GameView::GameOver;
    app.process_keyboard_frame(
        &mut runner,
        &input_frame_for_keys(&[VirtualKeyCode::K], &[VirtualKeyCode::K], &[]),
        now + Duration::from_millis(3),
    );
    assert!(matches!(runner.state().view, GameView::SkillTree));
}

#[test]
fn horizontal_repeat_sync_uses_frame_down_and_release_sets() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    let now = Instant::now();

    app.sync_horizontal_repeat_from_frame(
        &mut runner,
        &input_frame_for_keys(&[VirtualKeyCode::Left], &[VirtualKeyCode::Left], &[]),
        now,
    );
    assert!(app.horizontal_repeat.left_down);
    assert_eq!(app.horizontal_repeat.active, Some(HorizontalDir::Left));

    app.sync_horizontal_repeat_from_frame(
        &mut runner,
        &input_frame_for_keys(&[], &[VirtualKeyCode::Left], &[]),
        now + Duration::from_millis(5),
    );
    assert!(app.horizontal_repeat.left_down);

    app.sync_horizontal_repeat_from_frame(
        &mut runner,
        &input_frame_for_keys(&[], &[], &[VirtualKeyCode::Left]),
        now + Duration::from_millis(10),
    );
    assert!(!app.horizontal_repeat.left_down);
    assert_eq!(app.horizontal_repeat.active, None);
}

#[test]
fn skilltree_drag_frame_polling_respects_threshold_then_pans() {
    let (mut app, mut runner) = make_test_headful(GameView::SkillTree);
    runner.state_mut().skilltree.def.nodes.clear();

    app.last_skilltree = SkillTreeLayout {
        grid_cell: 20,
        grid_cols: 12,
        grid_rows: 8,
        grid_cam_min_x: -6,
        grid_cam_min_y: 0,
        ..SkillTreeLayout::default()
    };
    app.skilltree_cam_input = SkillTreeCameraInput {
        left_down: true,
        drag_started: false,
        drag_started_in_view: true,
        down_x: 100,
        down_y: 100,
        last_x: 100,
        last_y: 100,
    };

    app.mouse_x = 102;
    app.mouse_y = 102;
    app.update_skilltree_drag_from_frame(&mut runner, true);
    assert!(!app.skilltree_cam_input.drag_started);

    app.mouse_x = 140;
    app.mouse_y = 102;
    app.update_skilltree_drag_from_frame(&mut runner, true);
    assert!(app.skilltree_cam_input.drag_started);
    assert!(runner.state().skilltree.camera.pan.x < 0.0);
}

#[test]
fn reset_path_zeroes_dig_camera_state() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });

    let initial_depth = runner.state().tetris.background_depth_rows();
    app.dig_camera.reset(initial_depth);
    force_depth_reveal(&mut runner.state_mut().tetris);
    app.update_dig_camera_state(&runner, Duration::from_millis(16));
    assert!(
        app.dig_camera.offset_y_px() > 0.0,
        "expected depth gain to create camera impulse before reset"
    );

    app.reset_active_run(&mut runner);

    assert_approx_eq(app.dig_camera.offset_y_px(), 0.0);
    assert_eq!(
        app.dig_camera.last_depth_rows(),
        runner.state().tetris.background_depth_rows()
    );
}

#[test]
fn paused_state_holds_dig_camera_motion_and_depth_impulses() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });

    app.dig_camera
        .reset(runner.state().tetris.background_depth_rows());
    force_depth_reveal(&mut runner.state_mut().tetris);
    app.update_dig_camera_state(&runner, Duration::from_millis(16));
    let offset_before_pause = app.dig_camera.offset_y_px();
    assert!(offset_before_pause > 0.0);

    runner.state_mut().view = GameView::Tetris { paused: true };
    force_depth_reveal(&mut runner.state_mut().tetris);
    app.update_dig_camera_state(&runner, Duration::from_secs(1));
    assert_approx_eq(app.dig_camera.offset_y_px(), offset_before_pause);
    assert_eq!(
        app.dig_camera.last_depth_rows(),
        runner.state().tetris.background_depth_rows()
    );

    runner.state_mut().view = GameView::Tetris { paused: false };
    app.update_dig_camera_state(&runner, Duration::from_millis(300));
    assert!(
        app.dig_camera.offset_y_px() < offset_before_pause,
        "expected resume to settle camera toward zero"
    );
}

#[test]
fn gravity_dt_integration_locks_after_expected_timing() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    setup_grounded_lock_delay_case(&mut runner);

    app.apply_gravity_steps(&mut runner, Duration::from_millis(400));
    assert_eq!(runner.state().tetris.current_piece_pos(), Vec2i::new(4, 1));
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 300);

    app.apply_gravity_steps(&mut runner, Duration::from_millis(100));
    assert_eq!(runner.state().tetris.current_piece_pos(), Vec2i::new(4, 1));
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 400);

    app.apply_gravity_steps(&mut runner, Duration::from_millis(100));
    assert_eq!(
        runner.state().tetris.current_piece_pos(),
        Vec2i::new(4, runner.state().tetris.board().len() as i32)
    );
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 0);
}

#[test]
fn paused_state_does_not_advance_lock_delay() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    setup_grounded_lock_delay_case(&mut runner);

    runner.state_mut().view = GameView::Tetris { paused: true };
    app.apply_gravity_steps(&mut runner, Duration::from_millis(500));
    assert_eq!(runner.state().tetris.current_piece_pos(), Vec2i::new(4, 1));
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 0);

    runner.state_mut().view = GameView::Tetris { paused: false };
    app.apply_gravity_steps(&mut runner, Duration::from_millis(400));
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 300);

    runner.state_mut().view = GameView::Tetris { paused: true };
    app.apply_gravity_steps(&mut runner, Duration::from_millis(1000));
    assert_eq!(runner.state().tetris.current_piece_pos(), Vec2i::new(4, 1));
    assert_eq!(runner.state().tetris.grounded_lock_ms(), 300);

    runner.state_mut().view = GameView::Tetris { paused: false };
    app.apply_gravity_steps(&mut runner, Duration::from_millis(200));
    assert_eq!(
        runner.state().tetris.current_piece_pos(),
        Vec2i::new(4, runner.state().tetris.board().len() as i32)
    );
}

#[test]
fn grounded_move_stalls_even_when_gravity_tick_is_due_same_frame() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    setup_grounded_lock_delay_case(&mut runner);

    {
        let state = runner.state_mut();
        state.gravity_interval = Duration::from_millis(500);
        state.gravity_elapsed = Duration::from_millis(499);
        for y in 0..=1 {
            for x in 0..BOARD_WIDTH {
                state.tetris.set_cell(x, y, 0);
            }
        }
        state.tetris.set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    }

    runner.step_profiled(InputAction::MoveRight, &mut app.debug_hud);
    app.apply_gravity_steps(&mut runner, Duration::from_millis(1));

    assert_eq!(runner.state().tetris.current_piece_pos(), Vec2i::new(5, 1));
    assert!(
        runner.state().tetris.current_piece().is_some(),
        "grounded move should preserve active piece even if gravity tick becomes due immediately"
    );
    assert_eq!(
        runner.state().tetris.board()[0][5],
        0,
        "stalling move should not lock piece into board on same frame"
    );
}

#[test]
fn debug_hud_timer_toggle_disables_timer_tick_and_timeout() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    runner.state_mut().round_timer = RoundTimer::new(Duration::from_millis(200));

    app.debug_hud.toggle_round_timer_disabled();
    app.update_round_timer_and_game_over(&mut runner, Duration::from_secs(1));
    assert!(
        matches!(runner.state().view, GameView::Tetris { paused: false }),
        "timer-disabled mode should not trigger timeout game over"
    );
    assert_eq!(
        runner.state().round_timer.elapsed(),
        Duration::ZERO,
        "timer-disabled mode should not advance timer elapsed"
    );

    app.debug_hud.toggle_round_timer_disabled();
    app.update_round_timer_and_game_over(&mut runner, Duration::from_millis(250));
    assert!(
        matches!(runner.state().view, GameView::GameOver),
        "re-enabled timer should restore timeout game over behavior"
    );
}

#[test]
fn gravity_timeline_is_deterministic_for_same_inputs() {
    fn run_timeline() -> (game::tetris_core::TetrisSnapshot, Duration, u32) {
        let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
        setup_grounded_lock_delay_case(&mut runner);

        app.apply_gravity_steps(&mut runner, Duration::from_millis(250));
        runner.step_profiled(InputAction::MoveRight, &mut app.debug_hud);
        app.apply_gravity_steps(&mut runner, Duration::from_millis(250));
        runner.state_mut().view = GameView::Tetris { paused: true };
        app.apply_gravity_steps(&mut runner, Duration::from_millis(500));
        runner.state_mut().view = GameView::Tetris { paused: false };
        app.apply_gravity_steps(&mut runner, Duration::from_millis(200));

        (
            runner.state().tetris.snapshot(),
            runner.state().gravity_elapsed,
            runner.state().tetris.grounded_lock_ms(),
        )
    }

    let first = run_timeline();
    let second = run_timeline();
    assert_eq!(first, second);
}

#[test]
fn line_clear_delay_progresses_with_frame_dt_instead_of_waiting_for_gravity_interval() {
    let (mut app, mut runner) = make_test_headful(GameView::Tetris { paused: false });
    {
        let state = runner.state_mut();
        state.view = GameView::Tetris { paused: false };
        state.gravity_interval = Duration::from_millis(500);
        state.gravity_elapsed = Duration::ZERO;
        state.tetris.set_available_pieces(vec![Piece::O]);
        state.tetris.set_line_clear_delay_ms(180);
        for y in 0..BOARD_HEIGHT {
            for x in 0..BOARD_WIDTH {
                state.tetris.set_cell(x, y, 0);
            }
        }
        for x in 0..BOARD_WIDTH {
            if x == 4 || x == 5 {
                continue;
            }
            state.tetris.set_cell(x, 0, 1);
        }
        state
            .tetris
            .set_current_piece_for_test(Piece::O, Vec2i::new(4, 1), 0);
    }

    runner.step_profiled(InputAction::HardDrop, &mut app.debug_hud);
    assert!(runner.state().tetris.is_line_clear_active());
    assert_eq!(runner.state().tetris.lines_cleared(), 0);

    // Even though 100ms is below the 500ms gravity interval, clear progress should advance.
    app.apply_gravity_steps(&mut runner, Duration::from_millis(100));
    let progress = runner.state().tetris.line_clear_progress();
    assert!(
        progress > 0.0 && progress < 1.0,
        "line clear progress should advance on frame dt; got {progress}"
    );
    assert!(runner.state().tetris.is_line_clear_active());
    assert_eq!(runner.state().tetris.lines_cleared(), 0);

    app.apply_gravity_steps(&mut runner, Duration::from_millis(80));
    assert!(!runner.state().tetris.is_line_clear_active());
    assert_eq!(runner.state().tetris.lines_cleared(), 1);
}
