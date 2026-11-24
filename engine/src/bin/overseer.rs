use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use engine::reload::{MANAGED_ENV, RELOAD_EXIT_CODE, RELOAD_REJECTED, RELOAD_REQUEST};
use engine::timeline::session_directory;
use serde::Deserialize;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEBOUNCE: Duration = Duration::from_millis(300);

const USAGE: &str = "Usage: overseer --package NAME --bin NAME --watch PATH [--watch PATH ...]
                [--timeline DIR] [--fresh] [--release]

Watches the given files or source directories and runs a Sycho game.
The timeline defaults to target/timelines/<bin>. --fresh creates a separate timeline.";

struct Options {
    package: String,
    binary: String,
    timeline: PathBuf,
    watch: Vec<PathBuf>,
    release: bool,
}

impl Options {
    fn from_args() -> io::Result<Option<Self>> {
        let mut args = env::args_os().skip(1);
        let mut package = None;
        let mut binary = None;
        let mut timeline = None;
        let mut watch = Vec::new();
        let mut fresh = false;
        let mut release = false;
        while let Some(flag) = args.next() {
            let flag = flag.to_str().ok_or_else(|| invalid("invalid option"))?;
            let mut value = || {
                args.next()
                    .filter(|arg| !arg.to_string_lossy().starts_with('-'))
                    .ok_or_else(|| invalid(format!("{flag} requires a value")))
            };
            match flag {
                "--package" => {
                    package = Some(
                        value()?
                            .into_string()
                            .map_err(|_| invalid("invalid package name"))?,
                    )
                }
                "--bin" => {
                    binary = Some(
                        value()?
                            .into_string()
                            .map_err(|_| invalid("invalid binary name"))?,
                    )
                }
                "--timeline" => timeline = Some(PathBuf::from(value()?)),
                "--watch" => watch.push(PathBuf::from(value()?)),
                "--fresh" => fresh = true,
                "--release" => release = true,
                "--help" | "-h" => {
                    println!("{USAGE}");
                    return Ok(None);
                }
                _ => return Err(invalid(format!("unknown option {flag}"))),
            }
        }
        let package = package.ok_or_else(|| invalid("--package is required"))?;
        let binary = binary.ok_or_else(|| invalid("--bin is required"))?;
        if watch.is_empty() {
            return Err(invalid("at least one --watch path is required"));
        }
        let default = PathBuf::from("target/timelines").join(&binary);
        let timeline = session_directory(default, timeline, fresh)?;
        Ok(Some(Self {
            package,
            binary,
            timeline,
            watch,
            release,
        }))
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn main() -> io::Result<()> {
    if let Some(options) = Options::from_args()? {
        supervise(&options)?;
    }
    Ok(())
}

fn supervise(options: &Options) -> io::Result<()> {
    let staging = Staging::new()?;
    let mut game: Option<RunningGame> = None;
    let mut observed = scan_sources(&options.watch)?;
    let mut rebuild = Some(Instant::now());
    let mut generation = 0u64;

    println!(
        "watching {} source paths for {}",
        options.watch.len(),
        options.binary
    );
    loop {
        if let Some(running) = &mut game
            && let Some(status) = running.child.try_wait()?
        {
            return game_exit(status);
        }
        let current = scan_sources(&options.watch)?;
        if current != observed {
            observed = current;
            rebuild = Some(Instant::now());
        }
        if rebuild.is_some_and(|since| since.elapsed() >= DEBOUNCE) {
            rebuild = None;
            generation += 1;
            match build(options).and_then(|binary| staging.copy(&binary, generation)) {
                Ok(candidate) => {
                    // Let edits made during a build settle before replacing the live process.
                    if scan_sources(&options.watch)? != observed {
                        continue;
                    }
                    if let Some(running) = &mut game {
                        match running.handoff() {
                            Ok(Handoff::Ready) => {
                                println!("checkpoint committed, starting the new build")
                            }
                            Ok(Handoff::Rejected) => continue,
                            Ok(Handoff::Exited(status)) => return game_exit(status),
                            Err(err) => {
                                eprintln!("could not request a reload: {err}");
                                continue;
                            }
                        }
                    }
                    game = Some(RunningGame::launch(candidate, &options.timeline)?);
                }
                Err(err) => {
                    eprintln!("build failed: {err}. Waiting for another source change");
                }
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn game_exit(status: ExitStatus) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "game process exited with {status}"
        )))
    }
}

type SourceSnapshot = BTreeMap<PathBuf, (SystemTime, u64)>;

fn scan_sources(paths: &[PathBuf]) -> io::Result<SourceSnapshot> {
    let mut files = SourceSnapshot::new();
    for path in paths {
        scan_path(path, &mut files)?;
    }
    Ok(files)
}

fn scan_path(path: &Path, files: &mut SourceSnapshot) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if metadata.is_file() {
        files.insert(path.to_path_buf(), (metadata.modified()?, metadata.len()));
    } else if metadata.is_dir() {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            if name != "target" && !name.to_string_lossy().starts_with('.') {
                scan_path(&entry.path(), files)?;
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    executable: Option<PathBuf>,
    message: Option<CargoDiagnostic>,
}

#[derive(Deserialize)]
struct CargoDiagnostic {
    rendered: Option<String>,
}

fn build(options: &Options) -> io::Result<PathBuf> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command.args([
        "build",
        "--message-format=json-render-diagnostics",
        "--package",
        &options.package,
        "--bin",
        &options.binary,
    ]);
    if options.release {
        command.arg("--release");
    }
    let mut build = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = build.stdout.take().expect("Cargo stdout is piped");
    let mut executable = None;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        match serde_json::from_str::<CargoMessage>(&line) {
            Ok(CargoMessage {
                reason,
                executable: Some(path),
                ..
            }) if reason == "compiler-artifact" => {
                executable = Some(path);
            }
            Ok(CargoMessage {
                message:
                    Some(CargoDiagnostic {
                        rendered: Some(text),
                    }),
                ..
            }) => {
                eprint!("{text}");
            }
            Ok(_) => {}
            Err(_) => println!("{line}"),
        }
    }
    let status = build.wait()?;
    if !status.success() {
        return Err(io::Error::other(format!("Cargo exited with {status}")));
    }
    executable.ok_or_else(|| io::Error::other("Cargo did not report a game executable"))
}

struct Staging(PathBuf);

impl Staging {
    fn new() -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        let directory = env::temp_dir().join(format!(
            "sycho-overseer-{}-{}",
            std::process::id(),
            stamp.as_nanos()
        ));
        fs::create_dir(&directory)?;
        Ok(Self(directory))
    }

    fn copy(&self, binary: &Path, generation: u64) -> io::Result<StagedGame> {
        let path = self
            .0
            .join(format!("game-{generation}{}", env::consts::EXE_SUFFIX));
        fs::copy(binary, &path)?;
        Ok(StagedGame(path))
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct StagedGame(PathBuf);

impl Drop for StagedGame {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

struct RunningGame {
    child: Child,
    handoff_failed: Receiver<()>,
    _executable: StagedGame,
}

enum Handoff {
    Ready,
    Rejected,
    Exited(ExitStatus),
}

impl RunningGame {
    fn launch(executable: StagedGame, timeline: &Path) -> io::Result<Self> {
        let mut child = Command::new(&executable.0)
            .arg("--timeline")
            .arg(timeline)
            .env(MANAGED_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("game stdout is piped");
        let (tx, handoff_failed) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = Vec::new();
            while reader.read_until(b'\n', &mut line).is_ok_and(|len| len > 0) {
                if String::from_utf8_lossy(&line).trim_end() == RELOAD_REJECTED {
                    let _ = tx.send(());
                } else {
                    let _ = io::stdout().write_all(&line);
                }
                line.clear();
            }
        });
        println!("started game process {}", child.id());
        Ok(Self {
            child,
            handoff_failed,
            _executable: executable,
        })
    }

    fn handoff(&mut self) -> io::Result<Handoff> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(Handoff::Exited(status));
        }
        let stdin = self.child.stdin.as_mut().expect("game stdin is piped");
        writeln!(stdin, "{RELOAD_REQUEST}")?;
        stdin.flush()?;
        let mut slow = false;
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(if status.code() == Some(RELOAD_EXIT_CODE) {
                    Handoff::Ready
                } else {
                    Handoff::Exited(status)
                });
            }
            if self.handoff_failed.try_recv().is_ok() {
                eprintln!("reload deferred because the running game could not commit its state");
                return Ok(Handoff::Rejected);
            }
            if !slow && start.elapsed() >= Duration::from_secs(5) {
                slow = true;
                eprintln!("waiting for the running game to commit its state");
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}
