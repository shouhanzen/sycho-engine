use std::time::Duration;

use engine::surface::SurfaceSize;

use game::settings::PlayerSettings;
use game::state::GameState;
use game::tetris_core::{Piece, TetrisCore};
use game::view::GameView;

#[test]
fn game_state_round_trip_preserves_state() {
    let mut core = TetrisCore::new(0);
    core.set_available_pieces(vec![Piece::O]);
    core.initialize_game();

    let mut state = GameState::new(core);
    state.view = GameView::SkillTree;
    state
        .round_timer
        .tick_if_running(Duration::from_secs(3), true);
    state.gravity_interval = Duration::from_millis(400);
    state.gravity_elapsed = Duration::from_millis(120);
    state.skilltree.progress.money = 25;
    state.round_timer_disabled = true;
    state.display.viewport = SurfaceSize::new(800, 600);
    state.settings.values.audio.master_volume = 0.25;
    state.settings.values.gameplay.show_round_timer = false;
    state.settings.values.video.screen_shake_percent = 25;
    state.display.dig_camera.update(2, Duration::ZERO, false);

    let json = serde_json::to_string(&state).expect("serialize game state");
    let restored: GameState = serde_json::from_str(&json).expect("deserialize game state");

    assert_eq!(restored.view, state.view);
    assert_eq!(restored.tetris.snapshot(), state.tetris.snapshot());
    assert_eq!(restored.round_timer, state.round_timer);
    assert!(restored.round_timer_disabled);
    assert_eq!(restored.display, state.display);
    assert_eq!(restored.settings.values, state.settings.values);
    assert!(!restored.settings.open);
    let shake = state.display.dig_camera.offset_y_px();
    assert!(shake > 0.0);
    assert_eq!(restored.world_offset_y_px(), (shake * 0.25).round() as i32);
    assert_eq!(restored.gravity_interval, state.gravity_interval);
    assert_eq!(restored.gravity_elapsed, state.gravity_elapsed);
    assert_eq!(
        restored.skilltree.to_snapshot(),
        state.skilltree.to_snapshot()
    );
    assert!(restored.skilltree.is_unlocked("start"));
}

#[test]
fn old_snapshots_decode_display_and_menu_preferences() {
    let state = GameState::new(TetrisCore::new(0));
    let legacy_preferences = PlayerSettings {
        audio: game::settings::AudioSettings {
            master_volume: 0.3,
            ..state.settings.values.audio
        },
        ..PlayerSettings::default()
    };
    let mut snapshot = serde_json::to_value(state).unwrap();
    let fields = snapshot.as_object_mut().unwrap();
    fields.remove("settings");
    fields.remove("settings_open");
    assert!(snapshot["display"].get("screen_shake_percent").is_none());
    assert!(snapshot["display"].get("show_round_timer").is_none());
    snapshot["display"]["screen_shake_percent"] = 25.into();
    snapshot["display"]["show_round_timer"] = false.into();
    let closed: GameState = serde_json::from_value(snapshot.clone()).unwrap();
    assert!(!closed.settings.open);
    assert_eq!(
        closed.settings.values.audio,
        PlayerSettings::default().audio
    );
    assert_eq!(closed.settings.values.video.screen_shake_percent, 25);
    assert!(!closed.settings.values.gameplay.show_round_timer);

    snapshot["settings_overlay"] = serde_json::to_value(&legacy_preferences).unwrap();
    let open: GameState = serde_json::from_value(snapshot).unwrap();
    assert!(open.settings.open);
    assert_eq!(open.settings.values, legacy_preferences);
}
