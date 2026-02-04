use super::*;

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
        repeat.next_repeat_action(t0 + HorizontalRepeat::REPEAT_DELAY + HorizontalRepeat::REPEAT_INTERVAL),
        Some(InputAction::MoveLeft)
    );
}
