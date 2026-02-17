pub mod agent;
pub mod audio;
pub mod app;
pub mod editor;
pub mod graphics;
pub mod pixels_renderer;
pub mod profiling;
pub mod recording;
pub mod regression;
pub mod render;
pub mod slider;
pub mod surface;
pub mod ui;
pub mod ui_tree;
pub mod view_tree;

use std::{
    fs,
    io::{self, BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(Debug, Serialize, Deserialize)]
pub struct TimeMachine<State> {
    states: Vec<State>,
    frame: usize,
    #[serde(default = "default_record_every_n_frames")]
    record_every_n_frames: usize,
}

impl<State> TimeMachine<State> {
    pub fn new(initial_state: State) -> Self {
        Self {
            states: vec![initial_state],
            frame: 0,
            record_every_n_frames: default_record_every_n_frames(),
        }
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    pub fn record_every_n_frames(&self) -> usize {
        self.record_every_n_frames.max(1)
    }

    pub fn set_record_every_n_frames(&mut self, frames: usize) {
        self.record_every_n_frames = frames.max(1);
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn state(&self) -> &State {
        &self.states[self.frame]
    }

    pub fn state_at(&self, frame: usize) -> Option<&State> {
        self.states.get(frame)
    }

    pub fn history(&self) -> &[State] {
        &self.states
    }

    pub fn can_rewind(&self) -> bool {
        self.frame > 0
    }

    pub fn can_forward(&self) -> bool {
        self.frame + 1 < self.states.len()
    }

    pub fn seek(&mut self, frame: usize) -> usize {
        if self.states.is_empty() {
            self.frame = 0;
            return 0;
        }
        let max_frame = self.states.len().saturating_sub(1);
        self.frame = frame.min(max_frame);
        self.frame
    }

    pub fn rewind(&mut self, frames: usize) -> usize {
        self.frame = self.frame.saturating_sub(frames);
        self.frame
    }

    pub fn forward(&mut self, frames: usize) -> usize {
        let max_frame = self.states.len().saturating_sub(1);
        self.frame = (self.frame + frames).min(max_frame);
        self.frame
    }

    pub fn record(&mut self, state: State) -> usize {
        if self.frame + 1 < self.states.len() {
            self.states.truncate(self.frame + 1);
        }
        self.states.push(state);
        self.frame += 1;
        self.frame
    }

    pub fn save_json_file(&self, path: impl AsRef<Path>) -> io::Result<()>
    where
        State: Serialize,
    {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let file = fs::File::create(path)?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, self)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        writer.flush()?;
        Ok(())
    }

    pub fn load_json_file(path: impl AsRef<Path>) -> io::Result<Self>
    where
        State: DeserializeOwned,
    {
        let path = path.as_ref();
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let mut tm: Self =
            serde_json::from_reader(reader).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        if tm.states.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "timemachine recording has no states",
            ));
        }
        if tm.frame >= tm.states.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "timemachine recording frame {} out of bounds (len {})",
                    tm.frame,
                    tm.states.len()
                ),
            ));
        }

        if tm.record_every_n_frames == 0 {
            tm.record_every_n_frames = default_record_every_n_frames();
        }

        Ok(tm)
    }
}

fn default_record_every_n_frames() -> usize {
    1
}

pub trait GameLogic {
    type State: Clone;
    type Input;

    fn initial_state(&self) -> Self::State;
    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State;
}

pub trait RecordableState {
    fn recording_frame(&self) -> usize;
    fn save_recording(&self, path: &Path) -> io::Result<()>;
}

pub trait ReplayableState: Sized {
    fn replay_frame(&self) -> usize;
    fn replay_len(&self) -> usize;
    fn replay_seek(&mut self, frame: usize);
    fn replay_forward(&mut self, frames: usize);
    fn replay_rewind(&mut self, frames: usize);
    fn replay_load(&self, path: &Path) -> io::Result<Self>;
}

#[derive(Debug)]
pub struct HeadlessRunner<G: GameLogic> {
    game: G,
    timemachine: TimeMachine<G::State>,
    state: G::State,
    absolute_frame: usize,
}

impl<G: GameLogic> HeadlessRunner<G> {
    pub fn new(game: G) -> Self {
        let initial_state = game.initial_state();
        Self {
            game,
            timemachine: TimeMachine::new(initial_state.clone()),
            state: initial_state,
            absolute_frame: 0,
        }
    }

    pub fn from_timemachine(game: G, timemachine: TimeMachine<G::State>) -> Self {
        let state = timemachine.state().clone();
        let absolute_frame = timemachine
            .frame()
            .saturating_mul(timemachine.record_every_n_frames());
        Self {
            game,
            timemachine,
            state,
            absolute_frame,
        }
    }

    pub fn frame(&self) -> usize {
        self.timemachine.frame()
    }

    pub fn absolute_frame(&self) -> usize {
        self.absolute_frame
    }

    pub fn record_every_n_frames(&self) -> usize {
        self.timemachine.record_every_n_frames()
    }

    pub fn set_record_every_n_frames(&mut self, frames: usize) {
        self.timemachine.set_record_every_n_frames(frames);
    }

    pub fn state(&self) -> &G::State {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut G::State {
        &mut self.state
    }

    pub fn history(&self) -> &[G::State] {
        self.timemachine.history()
    }

    pub fn timemachine(&self) -> &TimeMachine<G::State> {
        &self.timemachine
    }

    pub fn step(&mut self, input: G::Input) -> usize {
        let next_state = self.game.step(&self.state, input);
        self.state = next_state.clone();
        self.absolute_frame = self.absolute_frame.saturating_add(1);

        if self.absolute_frame % self.timemachine.record_every_n_frames() == 0 {
            self.timemachine.record(next_state)
        } else {
            self.timemachine.frame()
        }
    }

    pub fn step_profiled<P: profiling::Profiler>(
        &mut self,
        input: G::Input,
        profiler: &mut P,
    ) -> usize {
        use std::time::Instant;

        let total_start = Instant::now();

        let step_start = Instant::now();
        let next_state = self.game.step(&self.state, input);
        let step_dt = step_start.elapsed();

        let record_start = Instant::now();
        self.state = next_state.clone();
        self.absolute_frame = self.absolute_frame.saturating_add(1);
        let frame = if self.absolute_frame % self.timemachine.record_every_n_frames() == 0 {
            self.timemachine.record(next_state)
        } else {
            self.timemachine.frame()
        };
        let record_dt = if self.absolute_frame % self.timemachine.record_every_n_frames() == 0 {
            record_start.elapsed()
        } else {
            std::time::Duration::ZERO
        };

        let total_dt = total_start.elapsed();
        profiler.on_step(
            frame,
            profiling::StepTimings {
                step: step_dt,
                record: record_dt,
                total: total_dt,
            },
        );

        frame
    }

    pub fn run<I>(&mut self, inputs: I) -> usize
    where
        I: IntoIterator<Item = G::Input>,
    {
        let mut last_frame = self.frame();
        for input in inputs {
            last_frame = self.step(input);
        }
        last_frame
    }

    pub fn rewind(&mut self, frames: usize) -> usize {
        let frame = self.timemachine.rewind(frames);
        self.state = self.timemachine.state().clone();
        self.absolute_frame = frame.saturating_mul(self.timemachine.record_every_n_frames());
        frame
    }

    pub fn forward(&mut self, frames: usize) -> usize {
        let frame = self.timemachine.forward(frames);
        self.state = self.timemachine.state().clone();
        self.absolute_frame = frame.saturating_mul(self.timemachine.record_every_n_frames());
        frame
    }

    pub fn seek(&mut self, frame: usize) -> usize {
        let frame = self.timemachine.seek(frame);
        self.state = self.timemachine.state().clone();
        self.absolute_frame = frame.saturating_mul(self.timemachine.record_every_n_frames());
        frame
    }
}

impl<G> RecordableState for HeadlessRunner<G>
where
    G: GameLogic,
    G::State: Serialize,
{
    fn recording_frame(&self) -> usize {
        self.frame()
    }

    fn save_recording(&self, path: &Path) -> io::Result<()> {
        self.timemachine.save_json_file(path)
    }
}

impl<G> ReplayableState for HeadlessRunner<G>
where
    G: GameLogic + Clone,
    G::State: Serialize + DeserializeOwned,
{
    fn replay_frame(&self) -> usize {
        self.frame()
    }

    fn replay_len(&self) -> usize {
        self.history().len()
    }

    fn replay_seek(&mut self, frame: usize) {
        let _ = self.seek(frame);
    }

    fn replay_forward(&mut self, frames: usize) {
        let _ = self.forward(frames);
    }

    fn replay_rewind(&mut self, frames: usize) {
        let _ = self.rewind(frames);
    }

    fn replay_load(&self, path: &Path) -> io::Result<Self> {
        let tm = TimeMachine::<G::State>::load_json_file(path)?;
        Ok(HeadlessRunner::from_timemachine(self.game.clone(), tm))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiling::{Profiler, StepTimings};

    #[test]
    fn timemachine_rewind_and_branch() {
        let mut tm = TimeMachine::new(0);
        tm.record(1);
        tm.record(2);
        assert_eq!(tm.state(), &2);

        tm.rewind(1);
        assert_eq!(tm.state(), &1);

        tm.record(99);
        assert_eq!(tm.history(), &[0, 1, 99]);
        assert_eq!(tm.frame(), 2);
    }

    #[test]
    fn runner_steps_and_seeks() {
        struct Additive;

        impl GameLogic for Additive {
            type State = i32;
            type Input = i32;

            fn initial_state(&self) -> Self::State {
                0
            }

            fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
                *state + input
            }
        }

        let mut runner = HeadlessRunner::new(Additive);
        runner.run([1, 2, 3]);
        assert_eq!(runner.frame(), 3);
        assert_eq!(runner.state(), &6);

        runner.rewind(2);
        assert_eq!(runner.state(), &1);

        runner.forward(1);
        assert_eq!(runner.state(), &3);
    }

    #[test]
    fn runner_step_profiled_calls_profiler_hook() {
        struct Additive;

        impl GameLogic for Additive {
            type State = i32;
            type Input = i32;

            fn initial_state(&self) -> Self::State {
                0
            }

            fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
                *state + input
            }
        }

        #[derive(Default)]
        struct Capture {
            frames: Vec<usize>,
            timings: Vec<StepTimings>,
        }

        impl Profiler for Capture {
            fn on_step(&mut self, frame: usize, timings: StepTimings) {
                self.frames.push(frame);
                self.timings.push(timings);
            }
        }

        let mut runner = HeadlessRunner::new(Additive);
        let mut capture = Capture::default();

        let frame = runner.step_profiled(1, &mut capture);
        assert_eq!(frame, 1);
        assert_eq!(runner.state(), &1);
        assert_eq!(capture.frames, vec![1]);

        let t = capture.timings[0];
        assert!(t.total >= t.step);
        assert!(t.total >= t.record);
    }

    #[test]
    fn runner_records_every_n_frames() {
        struct Additive;

        impl GameLogic for Additive {
            type State = i32;
            type Input = i32;

            fn initial_state(&self) -> Self::State {
                0
            }

            fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
                *state + input
            }
        }

        let mut runner = HeadlessRunner::new(Additive);
        runner.set_record_every_n_frames(2);
        assert_eq!(runner.record_every_n_frames(), 2);

        runner.step(1);
        assert_eq!(runner.state(), &1);
        assert_eq!(runner.frame(), 0);
        assert_eq!(runner.history().len(), 1);

        runner.step(1);
        assert_eq!(runner.state(), &2);
        assert_eq!(runner.frame(), 1);
        assert_eq!(runner.history().len(), 2);

        runner.step(1);
        assert_eq!(runner.state(), &3);
        assert_eq!(runner.frame(), 1);
        assert_eq!(runner.history().len(), 2);
    }
}
