pub mod app;
pub mod audio;
pub mod embedded;
pub mod gpu_timer;
pub mod graphics;
mod offscreen;
pub mod pixels_renderer;
pub mod profiling;
pub mod recording;
pub mod regression;
pub mod reload;
#[cfg(target_os = "macos")]
pub mod shared_surface;
pub mod slider;
pub mod surface;
pub mod timeline;
pub mod ui;

pub trait GameLogic {
    type State;
    type Input;

    fn initial_state(&self) -> Self::State;
    fn step(&self, state: &mut Self::State, input: Self::Input);
}

#[derive(Debug)]
pub struct HeadlessRunner<G: GameLogic> {
    game: G,
    state: G::State,
    frame: usize,
}

impl<G: GameLogic> HeadlessRunner<G> {
    pub fn new(game: G) -> Self {
        let initial_state = game.initial_state();
        Self::from_state(game, initial_state)
    }

    pub fn from_state(game: G, state: G::State) -> Self {
        Self {
            game,
            state,
            frame: 0,
        }
    }

    pub fn frame(&self) -> usize {
        self.frame
    }

    pub fn state(&self) -> &G::State {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut G::State {
        &mut self.state
    }

    pub fn step(&mut self, input: G::Input) -> usize {
        self.game.step(&mut self.state, input);
        self.frame = self.frame.saturating_add(1);
        self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Additive;

    impl GameLogic for Additive {
        type State = i32;
        type Input = i32;

        fn initial_state(&self) -> Self::State {
            0
        }

        fn step(&self, state: &mut Self::State, input: Self::Input) {
            *state += input;
        }
    }

    #[test]
    fn runner_steps_from_restored_state() {
        let mut runner = HeadlessRunner::from_state(Additive, 5);
        assert_eq!(runner.frame(), 0);
        assert_eq!(runner.step(2), 1);
        assert_eq!(runner.step(3), 2);
        assert_eq!(runner.state(), &10);
    }
}
