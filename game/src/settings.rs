use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AudioSettings {
    pub master_volume: f32,
    pub music_volume: f32,
    pub sfx_volume: f32,
    pub mute_all: bool,
    pub music_enabled: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            master_volume: 1.0,
            music_volume: 1.0,
            sfx_volume: 1.0,
            mute_all: false,
            music_enabled: true,
        }
    }
}

impl AudioSettings {
    pub fn clamp(mut self) -> Self {
        self.master_volume = self.master_volume.clamp(0.0, 1.0);
        self.music_volume = self.music_volume.clamp(0.0, 1.0);
        self.sfx_volume = self.sfx_volume.clamp(0.0, 1.0);
        self
    }

    pub fn effective_music_gain(self) -> f32 {
        if self.mute_all || !self.music_enabled {
            0.0
        } else {
            self.master_volume * self.music_volume
        }
    }

    pub fn effective_sfx_gain(self) -> f32 {
        if self.mute_all {
            0.0
        } else {
            self.master_volume * self.sfx_volume
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct GameplaySettings {
    pub show_round_timer: bool,
    pub auto_pause_on_focus_loss: bool,
}

impl Default for GameplaySettings {
    fn default() -> Self {
        Self {
            show_round_timer: true,
            auto_pause_on_focus_loss: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct VideoSettings {
    pub screen_shake_percent: u8,
    pub vsync: bool,
}

impl Default for VideoSettings {
    fn default() -> Self {
        Self {
            screen_shake_percent: 100,
            vsync: true,
        }
    }
}

impl VideoSettings {
    pub fn clamped_screen_shake(self) -> u8 {
        self.screen_shake_percent.min(100)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessibilitySettings {
    pub high_contrast_ui: bool,
    pub reduce_motion: bool,
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        Self {
            high_contrast_ui: false,
            reduce_motion: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerSettings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub gameplay: GameplaySettings,
    #[serde(default)]
    pub video: VideoSettings,
    #[serde(default)]
    pub accessibility: AccessibilitySettings,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            version: default_version(),
            audio: AudioSettings::default(),
            gameplay: GameplaySettings::default(),
            video: VideoSettings::default(),
            accessibility: AccessibilitySettings::default(),
        }
    }
}

impl PlayerSettings {
    pub fn sanitized(mut self) -> Self {
        self.version = default_version();
        self.audio = self.audio.clamp();
        self.video.screen_shake_percent = self.video.screen_shake_percent.min(100);
        self
    }
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn from_env() -> Self {
        if let Some(explicit) = std::env::var_os("ROLLOUT_SETTINGS_PATH") {
            return Self {
                path: PathBuf::from(explicit),
            };
        }

        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    let mut p = PathBuf::from(home);
                    p.push(".config");
                    p
                })
            })
            .unwrap_or_else(|| PathBuf::from("."));

        let mut path = base;
        path.push("sycho-engine");
        path.push("settings.json");
        Self { path }
    }

    pub fn load(&self) -> PlayerSettings {
        let Ok(bytes) = fs::read(&self.path) else {
            return PlayerSettings::default();
        };
        serde_json::from_slice::<PlayerSettings>(&bytes)
            .map(PlayerSettings::sanitized)
            .unwrap_or_else(|_| PlayerSettings::default())
    }

    pub fn save(&self, settings: &PlayerSettings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let text = serde_json::to_string_pretty(settings)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_effective_gains_respect_mute_flags() {
        let mut audio = AudioSettings::default();
        assert!((audio.effective_music_gain() - 1.0).abs() < 1e-6);
        assert!((audio.effective_sfx_gain() - 1.0).abs() < 1e-6);

        audio.mute_all = true;
        assert_eq!(audio.effective_music_gain(), 0.0);
        assert_eq!(audio.effective_sfx_gain(), 0.0);

        audio.mute_all = false;
        audio.music_enabled = false;
        assert_eq!(audio.effective_music_gain(), 0.0);
    }

    #[test]
    fn player_settings_sanitized_clamps_expected_fields() {
        let settings = PlayerSettings {
            version: 99,
            audio: AudioSettings {
                master_volume: 3.0,
                music_volume: -2.0,
                sfx_volume: 0.5,
                mute_all: false,
                music_enabled: true,
            },
            video: VideoSettings {
                screen_shake_percent: 200,
                vsync: true,
            },
            ..PlayerSettings::default()
        }
        .sanitized();

        assert_eq!(settings.version, 1);
        assert_eq!(settings.audio.master_volume, 1.0);
        assert_eq!(settings.audio.music_volume, 0.0);
        assert_eq!(settings.video.screen_shake_percent, 100);
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        let parsed: PlayerSettings =
            serde_json::from_str(r#"{"version":1,"audio":{"master_volume":0.5,"music_volume":0.5,"sfx_volume":0.5,"mute_all":false,"music_enabled":true}}"#)
                .expect("settings JSON should parse");
        assert_eq!(parsed.gameplay, GameplaySettings::default());
        assert_eq!(parsed.video, VideoSettings::default());
        assert_eq!(parsed.accessibility, AccessibilitySettings::default());
    }
}
