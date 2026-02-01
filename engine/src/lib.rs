pub mod agent;
pub mod editor;
pub mod render;
pub mod surface;
pub mod recording;
pub mod profiling;

#[derive(Debug)]
pub struct TimeMachine<State> {
    states: Vec<State>,
    frame: usize,
}

impl<State> TimeMachine<State> {
    pub fn new(initial_state: State) -> Self {
        Self {
            states: vec![initial_state],
            frame: 0,
        }
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    pub fn len(&self) -> usize {
        self.states.len()
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
}

pub trait GameLogic {
    type State;
    type Input;

    fn initial_state(&self) -> Self::State;
    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State;
}

#[derive(Debug)]
pub struct HeadlessRunner<G: GameLogic> {
    game: G,
    timemachine: TimeMachine<G::State>,
}

impl<G: GameLogic> HeadlessRunner<G> {
    pub fn new(game: G) -> Self {
        let initial_state = game.initial_state();
        Self {
            game,
            timemachine: TimeMachine::new(initial_state),
        }
    }

    pub fn frame(&self) -> usize {
        self.timemachine.frame()
    }

    pub fn state(&self) -> &G::State {
        self.timemachine.state()
    }

    pub fn history(&self) -> &[G::State] {
        self.timemachine.history()
    }

    pub fn timemachine(&self) -> &TimeMachine<G::State> {
        &self.timemachine
    }

    pub fn step(&mut self, input: G::Input) -> usize {
        let next_state = self.game.step(self.timemachine.state(), input);
        self.timemachine.record(next_state)
    }

    pub fn step_profiled<P: profiling::Profiler>(&mut self, input: G::Input, profiler: &mut P) -> usize {
        use std::time::Instant;

        let total_start = Instant::now();

        let step_start = Instant::now();
        let next_state = self.game.step(self.timemachine.state(), input);
        let step_dt = step_start.elapsed();

        let record_start = Instant::now();
        let frame = self.timemachine.record(next_state);
        let record_dt = record_start.elapsed();

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
        self.timemachine.rewind(frames)
    }

    pub fn forward(&mut self, frames: usize) -> usize {
        self.timemachine.forward(frames)
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
}
