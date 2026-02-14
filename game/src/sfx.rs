/// Shared SFX volume constants (0.0..=1.0).
///
/// These are used by headful clients and validated by tests.
pub const MOVE_PIECE_SFX_VOLUME: f32 = 0.25;
pub const ACTION_SFX_VOLUME: f32 = 0.35;
pub const LINE_CLEAR_SFX_VOLUME: f32 = 0.45;

/// Default background music volume (0.0..=1.0).
///
/// Kept intentionally low so it sits under the gameplay SFX.
pub const MUSIC_VOLUME: f32 = 0.12;
