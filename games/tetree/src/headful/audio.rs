use std::{io::Cursor, time::Duration};

use engine::audio::{MusicRuntime, Quantize, Scene, StepPattern, Track, Waveform};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

use crate::{settings::AudioSettings, sfx::MUSIC_VOLUME};

#[derive(Debug, Clone)]
struct BgMusic {
    sample_rate: u32,
    channels: u16,
    chan: u16,
    last_sample: f32,
    runtime: MusicRuntime,
}

impl BgMusic {
    fn new() -> Self {
        let notes = vec![
            Some(220.0),
            Some(261.63),
            Some(329.63),
            Some(261.63),
            Some(196.0),
            Some(246.94),
            Some(293.66),
            Some(246.94),
        ];
        let arp = Track::new(
            "arp",
            StepPattern::from_notes(notes, 0.5).with_envelope(0.04, 0.12),
        )
        .with_gain(0.14)
        .with_waveform(Waveform::Sine)
        .with_harmonic_mix(0.30);

        let bass = Track::new(
            "bass",
            StepPattern::from_notes(
                vec![
                    Some(110.0),
                    None,
                    Some(98.0),
                    None,
                    Some(123.47),
                    None,
                    Some(98.0),
                    None,
                ],
                0.5,
            )
            .with_envelope(0.02, 0.10),
        )
        .with_gain(0.10)
        .with_waveform(Waveform::Triangle);

        let kick = Track::new(
            "kick",
            StepPattern::from_notes(
                vec![
                    Some(55.0),
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(55.0),
                    None,
                ],
                0.25,
            )
            .with_envelope(0.001, 0.08),
        )
        .with_gain(0.16)
        .with_waveform(Waveform::Sine);

        let mut runtime = MusicRuntime::new(48_000, 120.0);
        let _ = runtime.add_scene(
            Scene::new("main")
                .with_track(arp)
                .with_track(bass)
                .with_track(kick),
        );
        let _ = runtime.schedule_scene_switch("main", Quantize::Immediate);

        Self {
            sample_rate: 48_000,
            channels: 2,
            chan: 0,
            last_sample: 0.0,
            runtime,
        }
    }
}

impl Iterator for BgMusic {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        // Reuse each mono sample for L/R channels.
        if self.chan == 0 {
            self.chan = 1;
            self.last_sample = self.runtime.next_mono_sample();
            Some(self.last_sample)
        } else {
            self.chan = 0;
            Some(self.last_sample)
        }
    }
}

impl rodio::Source for BgMusic {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[derive(Debug, Clone)]
struct GlassBreakNoise {
    sample_rate: u32,
    channels: u16,
    frame: u64,
    chan: u16,
    state: u32,
}

impl GlassBreakNoise {
    const DURATION_MS: u64 = 180;

    fn new(seed: u32) -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame: 0,
            chan: 0,
            state: seed.max(1),
        }
    }

    fn total_frames(&self) -> u64 {
        self.sample_rate as u64 * Self::DURATION_MS / 1_000
    }

    fn next_noise(&mut self) -> f32 {
        // Xorshift32 pseudo-noise for a light "glassy" burst.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        let unit = (x as f32) / (u32::MAX as f32);
        unit * 2.0 - 1.0
    }
}

impl Iterator for GlassBreakNoise {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.frame >= self.total_frames() {
            return None;
        }
        let t = self.frame as f32 / self.sample_rate as f32;
        let progress = self.frame as f32 / self.total_frames() as f32;
        let env = (1.0 - progress).max(0.0).powf(2.2);
        let chirp = (2.0 * std::f32::consts::PI * (1_800.0 * t + 900.0 * t * t)).sin();
        let noise = self.next_noise();
        let sample = (chirp * 0.35 + noise * 0.65) * env * 0.45;

        self.chan += 1;
        if self.chan >= self.channels {
            self.chan = 0;
            self.frame = self.frame.saturating_add(1);
        }
        Some(sample)
    }
}

impl rodio::Source for GlassBreakNoise {
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.total_frames().saturating_sub(self.frame) as usize)
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_millis(Self::DURATION_MS))
    }
}

pub struct GameAudio {
    _stream: OutputStream,
    handle: OutputStreamHandle,
    click_wav: &'static [u8],
    music_sink: Option<Sink>,
}

impl GameAudio {
    pub fn open() -> Option<Self> {
        let (stream, handle) = match OutputStream::try_default() {
            Ok(output) => output,
            Err(err) => {
                eprintln!("warning: audio disabled: {err}");
                if std::env::var_os("WSL_INTEROP").is_some()
                    || std::env::var_os("WSL_DISTRO_NAME").is_some()
                {
                    eprintln!(
                        "hint: in WSL install `libasound2-plugins pulseaudio-utils alsa-utils` so ALSA can route to WSLg PulseAudio"
                    );
                }
                return None;
            }
        };
        let music_sink = Sink::try_new(&handle).ok().map(|sink| {
            sink.set_volume(MUSIC_VOLUME);
            sink.append(BgMusic::new());
            sink
        });
        Some(Self {
            _stream: stream,
            handle,
            click_wav: include_bytes!("../../assets/sfx/click.wav"),
            music_sink,
        })
    }

    pub fn play_click(&self, volume: f32) {
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.set_volume(volume);

        let Ok(source) = Decoder::new(Cursor::new(self.click_wav)) else {
            return;
        };
        sink.append(source);
        sink.detach();
    }

    pub fn play_glass_break(&self, volume: f32) {
        let Ok(sink) = Sink::try_new(&self.handle) else {
            return;
        };
        sink.set_volume(volume);
        sink.append(GlassBreakNoise::new(0xA53F_91C7));
        sink.detach();
    }

    pub fn apply_audio_settings(&self, audio: AudioSettings) {
        let Some(sink) = self.music_sink.as_ref() else {
            return;
        };

        let gain = MUSIC_VOLUME * audio.effective_music_gain();
        sink.set_volume(gain);
        if audio.music_enabled && !audio.mute_all {
            sink.play();
        } else {
            sink.pause();
        }
    }
}
