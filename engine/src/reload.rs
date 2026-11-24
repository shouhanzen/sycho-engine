//! The local protocol between the overseer and a game process.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver};

pub const MANAGED_ENV: &str = "SYCHO_RELOAD_MANAGED";
pub const RELOAD_EXIT_CODE: i32 = 75;
pub const RELOAD_REQUEST: &str = "reload";
pub const RELOAD_REJECTED: &str = "sycho: reload checkpoint failed";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessCommand {
    Reload,
    Stop,
}

pub fn listen_for_overseer() -> Option<Receiver<ProcessCommand>> {
    std::env::var_os(MANAGED_ENV)?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if line == RELOAD_REQUEST && tx.send(ProcessCommand::Reload).is_err() {
                return;
            }
        }
        let _ = tx.send(ProcessCommand::Stop);
    });
    Some(rx)
}

pub fn reject_reload(error: &io::Error) {
    eprintln!("could not commit the reload checkpoint: {error}");
    let _ = writeln!(io::stdout(), "{RELOAD_REJECTED}");
    let _ = io::stdout().flush();
}
