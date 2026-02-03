use game::sfx::{ACTION_SFX_VOLUME, LINE_CLEAR_SFX_VOLUME, MOVE_PIECE_SFX_VOLUME, MUSIC_VOLUME};

#[test]
fn move_piece_sfx_is_softer_than_other_actions() {
    assert!(
        MOVE_PIECE_SFX_VOLUME < ACTION_SFX_VOLUME,
        "expected MOVE_PIECE_SFX_VOLUME < ACTION_SFX_VOLUME (move should be softer)"
    );
}

#[test]
fn music_is_softer_than_gameplay_sfx() {
    assert!(
        MUSIC_VOLUME < MOVE_PIECE_SFX_VOLUME,
        "expected MUSIC_VOLUME < MOVE_PIECE_SFX_VOLUME (music should sit under gameplay SFX)"
    );
}

#[test]
fn sfx_volumes_are_in_valid_range() {
    for (name, v) in [
        ("move_piece", MOVE_PIECE_SFX_VOLUME),
        ("action", ACTION_SFX_VOLUME),
        ("line_clear", LINE_CLEAR_SFX_VOLUME),
        ("music", MUSIC_VOLUME),
    ] {
        assert!(v > 0.0, "{name} volume must be > 0.0, got {v}");
        assert!(v <= 1.0, "{name} volume must be <= 1.0, got {v}");
    }
}

