use crate::{GameLogic, HeadlessRunner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand<I> {
    Step(I),
    Reset,
    GetState,
    GetHistory,
    Rewind { frames: usize },
    Forward { frames: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentResponse<S> {
    State { frame: usize, state: S },
    History { frame: usize, history: Vec<S> },
}

pub struct AgentHost<G>
where
    G: GameLogic + Clone,
{
    game: G,
    runner: HeadlessRunner<G>,
}

impl<G> AgentHost<G>
where
    G: GameLogic + Clone,
    G::State: Clone,
{
    pub fn new(game: G) -> Self {
        let runner = HeadlessRunner::new(game.clone());
        Self { game, runner }
    }

    pub fn handle(&mut self, command: AgentCommand<G::Input>) -> AgentResponse<G::State> {
        match command {
            AgentCommand::Step(input) => {
                let frame = self.runner.step(input);
                AgentResponse::State {
                    frame,
                    state: self.runner.state().clone(),
                }
            }
            AgentCommand::Reset => {
                self.runner = HeadlessRunner::new(self.game.clone());
                AgentResponse::State {
                    frame: self.runner.frame(),
                    state: self.runner.state().clone(),
                }
            }
            AgentCommand::GetState => AgentResponse::State {
                frame: self.runner.frame(),
                state: self.runner.state().clone(),
            },
            AgentCommand::GetHistory => AgentResponse::History {
                frame: self.runner.frame(),
                history: self.runner.history().to_vec(),
            },
            AgentCommand::Rewind { frames } => {
                let frame = self.runner.rewind(frames);
                AgentResponse::State {
                    frame,
                    state: self.runner.state().clone(),
                }
            }
            AgentCommand::Forward { frames } => {
                let frame = self.runner.forward(frames);
                AgentResponse::State {
                    frame,
                    state: self.runner.state().clone(),
                }
            }
        }
    }

    pub fn runner(&self) -> &HeadlessRunner<G> {
        &self.runner
    }
}
