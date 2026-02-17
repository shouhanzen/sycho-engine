use engine::HeadlessRunner;
use engine::editor::{EditorSnapshot, EditorTimeline};
use tokio::sync::mpsc::error::TryRecvError;

use crate::headful_editor_api::{RemoteCmd, RemoteServer};
use crate::playtest::TetrisLogic;

pub fn drain_remote_commands(
    remote: Option<&mut RemoteServer>,
    runner: &mut HeadlessRunner<TetrisLogic>,
    mut reset_run: impl FnMut(&mut HeadlessRunner<TetrisLogic>),
) {
    let Some(remote) = remote else {
        return;
    };

    loop {
        let Some(cmd) = next_command(remote) else {
            break;
        };
        handle_remote_command(runner, cmd, &mut reset_run);
    }
}

fn next_command(remote: &mut RemoteServer) -> Option<RemoteCmd> {
    match remote.rx.try_recv() {
        Ok(cmd) => Some(cmd),
        Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
    }
}

fn handle_remote_command(
    runner: &mut HeadlessRunner<TetrisLogic>,
    cmd: RemoteCmd,
    reset_run: &mut impl FnMut(&mut HeadlessRunner<TetrisLogic>),
) {
    match cmd {
        RemoteCmd::GetState { respond } => {
            let _ = respond.send(snapshot(runner));
        }
        RemoteCmd::GetTimeline { respond } => {
            let _ = respond.send(timeline(runner));
        }
        RemoteCmd::Step { action_id, respond } => {
            match crate::editor_api::action_from_id(&action_id) {
                Some(action) => {
                    runner.step(action);
                    let _ = respond.send(Ok(snapshot(runner)));
                }
                None => {
                    let _ = respond.send(Err(format!("unknown actionId: {action_id}")));
                }
            }
        }
        RemoteCmd::Rewind { frames, respond } => {
            runner.rewind(frames);
            let _ = respond.send(snapshot(runner));
        }
        RemoteCmd::Forward { frames, respond } => {
            runner.forward(frames);
            let _ = respond.send(snapshot(runner));
        }
        RemoteCmd::Seek { frame, respond } => {
            runner.seek(frame);
            let _ = respond.send(snapshot(runner));
        }
        RemoteCmd::Reset { respond } => {
            reset_run(runner);
            let _ = respond.send(snapshot(runner));
        }
    }
}

fn snapshot(runner: &HeadlessRunner<TetrisLogic>) -> EditorSnapshot {
    let frame = runner.frame();
    crate::editor_api::snapshot_from_state(frame, runner.state())
}

fn timeline(runner: &HeadlessRunner<TetrisLogic>) -> EditorTimeline {
    let tm = runner.timemachine();
    EditorTimeline {
        frame: runner.frame(),
        history_len: runner.history().len(),
        can_rewind: tm.can_rewind(),
        can_forward: tm.can_forward(),
    }
}
