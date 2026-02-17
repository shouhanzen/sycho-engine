use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    bpm: f32,
    beats_per_bar: u8,
    sample_rate: u32,
    sample_clock: u64,
    playing: bool,
}

impl Transport {
    pub fn new(sample_rate: u32, bpm: f32) -> Self {
        Self {
            bpm: bpm.max(1.0),
            beats_per_bar: 4,
            sample_rate: sample_rate.max(1),
            sample_clock: 0,
            playing: true,
        }
    }

    pub fn with_signature(mut self, beats_per_bar: u8) -> Self {
        self.beats_per_bar = beats_per_bar.max(1);
        self
    }

    pub fn set_bpm(&mut self, bpm: f32) {
        self.bpm = bpm.max(1.0);
    }

    pub fn bpm(&self) -> f32 {
        self.bpm
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn beats_per_bar(&self) -> u8 {
        self.beats_per_bar
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    pub fn beat_position(&self) -> f64 {
        self.sample_clock as f64 / self.samples_per_beat()
    }

    pub fn advance(&mut self, samples: u64) {
        if self.playing {
            self.sample_clock = self.sample_clock.saturating_add(samples);
        }
    }

    pub fn quantized_delay_samples(&self, quantize: Quantize) -> u64 {
        let beat = self.beat_position();
        let target = match quantize {
            Quantize::Immediate => return 0,
            Quantize::Beat(step) => next_multiple_exclusive(beat, step.max(1) as f64),
            Quantize::Bar(step) => {
                let unit = self.beats_per_bar.max(1) as f64 * step.max(1) as f64;
                next_multiple_exclusive(beat, unit)
            }
        };
        let beats_until = (target - beat).max(0.0);
        (beats_until * self.samples_per_beat()).round() as u64
    }

    fn samples_per_beat(&self) -> f64 {
        (self.sample_rate as f64 * 60.0) / self.bpm as f64
    }
}

fn next_multiple_exclusive(value: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return value;
    }
    let rem = value % step;
    if rem.abs() < 1e-9 {
        value + step
    } else {
        value + (step - rem)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantize {
    Immediate,
    Beat(u32),
    Bar(u32),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MusicMix {
    pub energy: f32,
    pub tension: f32,
    pub depth: f32,
}

impl MusicMix {
    pub fn clamped(self) -> Self {
        Self {
            energy: self.energy.clamp(0.0, 1.0),
            tension: self.tension.clamp(0.0, 1.0),
            depth: self.depth.clamp(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Sine,
    Triangle,
    Square,
    Saw,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepPattern {
    notes_hz: Vec<Option<f32>>,
    step_beats: f32,
    attack: f32,
    release: f32,
}

impl StepPattern {
    pub fn from_notes(notes_hz: impl Into<Vec<Option<f32>>>, step_beats: f32) -> Self {
        let notes_hz = notes_hz.into();
        assert!(!notes_hz.is_empty(), "step pattern must have at least one step");
        Self {
            notes_hz,
            step_beats: step_beats.max(0.0625),
            attack: 0.05,
            release: 0.15,
        }
    }

    pub fn with_envelope(mut self, attack: f32, release: f32) -> Self {
        self.attack = attack.clamp(0.0, 0.49);
        self.release = release.clamp(0.0, 0.49);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: String,
    pub gain: f32,
    pub waveform: Waveform,
    pub harmonic_mix: f32,
    pub energy_gain: f32,
    pub tension_gain: f32,
    pub depth_gain: f32,
    pattern: StepPattern,
    phase: f32,
}

impl Track {
    pub fn new(id: impl Into<String>, pattern: StepPattern) -> Self {
        Self {
            id: id.into(),
            gain: 0.2,
            waveform: Waveform::Sine,
            harmonic_mix: 0.0,
            energy_gain: 0.0,
            tension_gain: 0.0,
            depth_gain: 0.0,
            pattern,
            phase: 0.0,
        }
    }

    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain.max(0.0);
        self
    }

    pub fn with_waveform(mut self, waveform: Waveform) -> Self {
        self.waveform = waveform;
        self
    }

    pub fn with_harmonic_mix(mut self, harmonic_mix: f32) -> Self {
        self.harmonic_mix = harmonic_mix.clamp(0.0, 1.0);
        self
    }

    pub fn with_mix_response(mut self, energy: f32, tension: f32, depth: f32) -> Self {
        self.energy_gain = energy;
        self.tension_gain = tension;
        self.depth_gain = depth;
        self
    }

    fn next_sample(&mut self, transport: &Transport, mix: MusicMix) -> f32 {
        let step_pos = (transport.beat_position() as f32) / self.pattern.step_beats;
        let step_index = (step_pos.floor() as usize) % self.pattern.notes_hz.len();
        let in_step = step_pos.fract();
        let env = envelope(in_step, self.pattern.attack, self.pattern.release);
        let Some(freq_hz) = self.pattern.notes_hz[step_index] else {
            return 0.0;
        };

        let phase_delta = TAU * freq_hz / transport.sample_rate() as f32;
        self.phase = (self.phase + phase_delta) % TAU;

        let base = waveform_sample(self.waveform, self.phase);
        let harmonic = waveform_sample(self.waveform, (self.phase * 2.0) % TAU) * self.harmonic_mix;
        let reactive = 1.0
            + mix.energy * self.energy_gain
            + mix.tension * self.tension_gain
            + mix.depth * self.depth_gain;
        (base + harmonic) * self.gain * reactive.max(0.0) * env
    }
}

fn waveform_sample(wave: Waveform, phase: f32) -> f32 {
    match wave {
        Waveform::Sine => phase.sin(),
        Waveform::Triangle => (2.0 / std::f32::consts::PI) * phase.sin().asin(),
        Waveform::Square => {
            if phase.sin() >= 0.0 {
                1.0
            } else {
                -1.0
            }
        }
        Waveform::Saw => 2.0 * (phase / TAU) - 1.0,
    }
}

fn envelope(in_step: f32, attack: f32, release: f32) -> f32 {
    if attack > 0.0 && in_step < attack {
        return in_step / attack;
    }
    if release > 0.0 && in_step > (1.0 - release) {
        return ((1.0 - in_step) / release).max(0.0);
    }
    1.0
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub id: String,
    tracks: Vec<Track>,
}

impl Scene {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            tracks: Vec::new(),
        }
    }

    pub fn with_track(mut self, track: Track) -> Self {
        self.tracks.push(track);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneError {
    DuplicateSceneId(String),
    UnknownScene(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSceneSwitch {
    target_index: usize,
    samples_until_switch: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MusicRuntime {
    transport: Transport,
    mix: MusicMix,
    scenes: Vec<Scene>,
    active_scene: Option<usize>,
    pending: Option<PendingSceneSwitch>,
}

impl MusicRuntime {
    pub fn new(sample_rate: u32, bpm: f32) -> Self {
        Self {
            transport: Transport::new(sample_rate, bpm),
            mix: MusicMix::default(),
            scenes: Vec::new(),
            active_scene: None,
            pending: None,
        }
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn transport_mut(&mut self) -> &mut Transport {
        &mut self.transport
    }

    pub fn set_mix(&mut self, mix: MusicMix) {
        self.mix = mix.clamped();
    }

    pub fn add_scene(&mut self, scene: Scene) -> Result<(), SceneError> {
        if self.scenes.iter().any(|s| s.id == scene.id) {
            return Err(SceneError::DuplicateSceneId(scene.id));
        }
        self.scenes.push(scene);
        Ok(())
    }

    pub fn schedule_scene_switch(
        &mut self,
        scene_id: &str,
        quantize: Quantize,
    ) -> Result<(), SceneError> {
        let Some(target_index) = self.scenes.iter().position(|s| s.id == scene_id) else {
            return Err(SceneError::UnknownScene(scene_id.to_string()));
        };
        let delay = self.transport.quantized_delay_samples(quantize);
        if delay == 0 {
            self.active_scene = Some(target_index);
            self.pending = None;
        } else {
            self.pending = Some(PendingSceneSwitch {
                target_index,
                samples_until_switch: delay,
            });
        }
        Ok(())
    }

    pub fn active_scene_id(&self) -> Option<&str> {
        self.active_scene
            .and_then(|i| self.scenes.get(i))
            .map(|s| s.id.as_str())
    }

    pub fn next_mono_sample(&mut self) -> f32 {
        if let Some(mut pending) = self.pending.take() {
            if pending.samples_until_switch == 0 {
                self.active_scene = Some(pending.target_index);
            } else {
                pending.samples_until_switch = pending.samples_until_switch.saturating_sub(1);
                self.pending = Some(pending);
            }
        }

        let mut sample = 0.0f32;
        if let Some(active) = self.active_scene {
            if let Some(scene) = self.scenes.get_mut(active) {
                for track in &mut scene.tracks {
                    sample += track.next_sample(&self.transport, self.mix);
                }
            }
        }

        self.transport.advance(1);
        // Soft-limit to avoid clipping.
        sample.tanh() * 0.9
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_quantizes_to_next_bar() {
        let mut transport = Transport::new(48_000, 120.0).with_signature(4);
        transport.advance(48_000); // 1 second => 2 beats @120bpm
        let delay = transport.quantized_delay_samples(Quantize::Bar(1));
        // Need 2 more beats => 1 second.
        assert_eq!(delay, 48_000);
    }

    #[test]
    fn runtime_switches_scene_immediately() {
        let mut runtime = MusicRuntime::new(48_000, 120.0);
        runtime
            .add_scene(Scene::new("a").with_track(Track::new(
                "lead",
                StepPattern::from_notes(vec![Some(220.0)], 1.0),
            )))
            .unwrap();
        runtime
            .add_scene(Scene::new("b").with_track(Track::new(
                "lead",
                StepPattern::from_notes(vec![Some(330.0)], 1.0),
            )))
            .unwrap();
        runtime
            .schedule_scene_switch("a", Quantize::Immediate)
            .unwrap();
        assert_eq!(runtime.active_scene_id(), Some("a"));

        runtime
            .schedule_scene_switch("b", Quantize::Immediate)
            .unwrap();
        assert_eq!(runtime.active_scene_id(), Some("b"));
    }

    #[test]
    fn runtime_switches_scene_when_quantized_delay_elapses() {
        let mut runtime = MusicRuntime::new(48_000, 120.0);
        runtime
            .add_scene(Scene::new("intro").with_track(Track::new(
                "lead",
                StepPattern::from_notes(vec![Some(220.0)], 1.0),
            )))
            .unwrap();
        runtime
            .add_scene(Scene::new("combat").with_track(Track::new(
                "lead",
                StepPattern::from_notes(vec![Some(330.0)], 1.0),
            )))
            .unwrap();

        runtime
            .schedule_scene_switch("intro", Quantize::Immediate)
            .unwrap();
        // At beat 0, switch to next bar means 4 beats = 2 seconds at 120bpm.
        runtime
            .schedule_scene_switch("combat", Quantize::Bar(1))
            .unwrap();
        assert_eq!(runtime.active_scene_id(), Some("intro"));

        for _ in 0..(48_000 * 2 + 5) {
            let _ = runtime.next_mono_sample();
        }
        assert_eq!(runtime.active_scene_id(), Some("combat"));
    }
}
