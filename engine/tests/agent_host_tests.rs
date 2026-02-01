use engine::agent::{AgentCommand, AgentHost, AgentResponse};
use engine::GameLogic;

#[derive(Clone)]
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

#[test]
fn agent_host_steps_and_reports_state() {
    let mut host = AgentHost::new(Additive);

    let response = host.handle(AgentCommand::Step(5));
    match response {
        AgentResponse::State { frame, state } => {
            assert_eq!(frame, 1);
            assert_eq!(state, 5);
        }
        _ => panic!("expected state response"),
    }
}

#[test]
fn agent_host_reset_restores_initial_state() {
    let mut host = AgentHost::new(Additive);
    host.handle(AgentCommand::Step(3));

    let response = host.handle(AgentCommand::Reset);
    match response {
        AgentResponse::State { frame, state } => {
            assert_eq!(frame, 0);
            assert_eq!(state, 0);
        }
        _ => panic!("expected state response"),
    }
}

#[test]
fn agent_host_returns_history() {
    let mut host = AgentHost::new(Additive);
    host.handle(AgentCommand::Step(1));
    host.handle(AgentCommand::Step(2));

    let response = host.handle(AgentCommand::GetHistory);
    match response {
        AgentResponse::History { frame, history } => {
            assert_eq!(frame, 2);
            assert_eq!(history, vec![0, 1, 3]);
        }
        _ => panic!("expected history response"),
    }
}
