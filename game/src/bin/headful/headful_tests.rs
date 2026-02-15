use super::*;
use game::tetris_core::{BOARD_WIDTH, DEFAULT_BOTTOMWELL_ROWS, TetrisCore};

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
    let target_y = DEFAULT_BOTTOMWELL_ROWS;
    let depth_before = core.background_depth_rows();
    for x in 0..BOARD_WIDTH {
        core.set_cell(x, target_y, 1);
    }
    let cleared = core.clear_lines();
    assert!(cleared >= 1, "expected at least one cleared line");
    assert!(
        core.background_depth_rows() > depth_before,
        "expected background depth to increase"
    );
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
