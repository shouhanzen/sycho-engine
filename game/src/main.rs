use engine::{GameLogic, HeadlessRunner};

#[derive(Debug)]
struct CounterGame;

#[derive(Debug)]
struct CounterState {
    value: i32,
}

#[derive(Debug, Clone, Copy)]
enum CounterInput {
    Add(i32),
    Reset,
}

impl GameLogic for CounterGame {
    type State = CounterState;
    type Input = CounterInput;

    fn initial_state(&self) -> Self::State {
        CounterState { value: 0 }
    }

    fn step(&self, state: &Self::State, input: Self::Input) -> Self::State {
        match input {
            CounterInput::Add(delta) => CounterState {
                value: state.value + delta,
            },
            CounterInput::Reset => CounterState { value: 0 },
        }
    }
}

fn main() {
    let mut runner = HeadlessRunner::new(CounterGame);
    runner.run([
        CounterInput::Add(3),
        CounterInput::Add(4),
        CounterInput::Reset,
    ]);
    runner.rewind(1);
    runner.step(CounterInput::Add(10));

    println!(
        "frame {} value {} history_len {}",
        runner.frame(),
        runner.state().value,
        runner.history().len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_input_clears_state() {
        let game = CounterGame;
        let initial = game.initial_state();
        let stepped = game.step(&initial, CounterInput::Add(5));
        let reset = game.step(&stepped, CounterInput::Reset);

        assert_eq!(stepped.value, 5);
        assert_eq!(reset.value, 0);
    }
}
