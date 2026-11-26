use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use engine::embedded::{ADDRESS_ENV, DISPLAY_ENV, GPU_PREVIEW_EXIT_CODE, SURFACE_ENV};
use engine::launch::{RecordingOptions, migrate_timeline, project_timeline};
use engine::reload::{MANAGED_ENV, RELOAD_EXIT_CODE, RELOAD_REJECTED, RELOAD_REQUEST};
use engine::surface::DisplayMetrics;
use serde::Deserialize;

#[path = "overseer/editor.rs"]
mod editor;
#[path = "overseer/project.rs"]
mod project;

use project::Project;

use editor::{Bridge, Lifecycle, Phase};

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const DEBOUNCE: Duration = Duration::from_millis(300);
const USAGE: &str = "Usage: overseer --editor [--timeline DIR | --fresh] [--release]
       overseer --package NAME --bin NAME [--manifest-path PATH] [--watch PATH ...]
                [--no-recording | --timeline DIR] [--fresh] [--release]";

fn engine_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn tetree() -> Project {
    Project {
        manifest: engine_root().join("../games/tetree/Cargo.toml"),
        package: "tetree".into(),
        binary: "headful".into(),
    }
}

fn cargo() -> std::ffi::OsString {
    env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

struct Options {
    manifest: Option<PathBuf>,
    package: String,
    binary: String,
    timeline: Option<PathBuf>,
    runtime: Option<PathBuf>,
    watch: Vec<PathBuf>,
    release: bool,
}

enum Launch {
    Editor {
        release: bool,
        timeline: Option<PathBuf>,
    },
    Watch(Options),
}

impl Launch {
    fn from_args() -> io::Result<Option<Self>> {
        let mut args = env::args_os().skip(1);
        let mut manifest = None;
        let mut package = None;
        let mut binary = None;
        let mut recording = RecordingOptions::default();
        let mut watch = Vec::new();
        let mut release = false;
        let mut editor = false;
        let mut check_launch = false;
        while let Some(flag) = args.next() {
            let flag = flag.to_str().ok_or_else(|| invalid("invalid option"))?;
            if recording.parse_arg(flag, &mut args)? {
                continue;
            }
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
                "--manifest-path" => manifest = Some(PathBuf::from(value()?).canonicalize()?),
                "--watch" => watch.push(PathBuf::from(value()?)),
                "--playback" => {
                    return Err(invalid(
                        "--playback requires an unwatched standalone launch",
                    ));
                }
                "--release" => release = true,
                "--editor" => editor = true,
                "--check-launch" => check_launch = true,
                "--help" | "-h" => {
                    println!("{USAGE}");
                    return Ok(None);
                }
                _ => return Err(invalid(format!("unknown option {flag}"))),
            }
        }
        recording.validate(editor)?;
        if check_launch {
            return Ok(None);
        }
        if editor {
            let example = tetree();
            let default = project_timeline(example.root(), &example.binary);
            let timeline = if recording.uses_default() {
                None
            } else {
                recording
                    .directory(default)?
                    .map(absolute_path)
                    .transpose()?
            };
            return Ok(Some(Self::Editor { release, timeline }));
        }
        let package = package.ok_or_else(|| invalid("--package is required"))?;
        let binary = binary.ok_or_else(|| invalid("--bin is required"))?;
        let default = match &manifest {
            Some(manifest) => project_timeline(manifest.parent().unwrap(), &binary),
            None => PathBuf::from("target/timelines").join(&binary),
        };
        if recording.uses_default()
            && package == "tetree"
            && let Some(path) = &manifest
            && path == &tetree().manifest.canonicalize()?
        {
            migrate_tetree(&default)?;
        }
        let timeline = recording
            .directory(default)?
            .map(absolute_path)
            .transpose()?;
        if watch.is_empty() {
            let project = Project {
                manifest: manifest
                    .clone()
                    .ok_or_else(|| invalid("provide --manifest-path or --watch"))?,
                package: package.clone(),
                binary: binary.clone(),
            };
            watch = project.options(release, None)?.watch;
        }
        Ok(Some(Self::Watch(Options {
            manifest,
            package,
            binary,
            timeline,
            runtime: None,
            watch,
            release,
        })))
    }
}

fn absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    Ok(env::current_dir()?.join(path))
}

fn migrate_tetree(destination: &Path) -> io::Result<()> {
    migrate_timeline(
        &engine_root().join("../target/timelines/headful"),
        destination,
    )
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn main() -> io::Result<()> {
    match Launch::from_args()? {
        Some(Launch::Editor { release, timeline }) => {
            editor::run(release, timeline).map_err(|err| io::Error::other(err.to_string()))
        }
        Some(Launch::Watch(options)) => supervise(&options, None, &AtomicBool::new(false)),
        None => Ok(()),
    }
}

pub(crate) struct Supervisor {
    stop: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}

impl Supervisor {
    fn start(
        project: Project,
        bridge: Bridge,
        release: bool,
        timeline: Option<PathBuf>,
        runtime: Option<PathBuf>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = thread::spawn(move || {
            let run = || {
                let mut options = project.options(release, timeline.clone())?;
                options.runtime.clone_from(&runtime);
                if project.manifest == tetree().manifest.canonicalize().unwrap_or_default()
                    && project.binary == "headful"
                    && timeline.is_none()
                {
                    migrate_tetree(options.timeline.as_deref().unwrap())?;
                }
                supervise(&options, Some(&bridge), &worker_stop)
            };
            let mut can_start = bridge.lifecycle() == Lifecycle::Enabled;
            while !worker_stop.load(Ordering::Relaxed) {
                match bridge.lifecycle() {
                    Lifecycle::Disabling => {
                        bridge.finish_disable(None, None);
                        can_start = false;
                    }
                    Lifecycle::Enabling => {
                        bridge.finish_enable();
                        can_start = true;
                    }
                    _ => {}
                }
                if can_start {
                    match run() {
                        Ok(()) => break,
                        Err(err) => {
                            bridge.fail(err.to_string());
                            eprintln!("{}: {err}", project.label());
                            can_start = false;
                        }
                    }
                }
                thread::sleep(POLL_INTERVAL);
            }
            bridge.disconnect(Phase::Exited);
        });
        Self { stop, worker }
    }

    fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn finish(self) {
        self.request_stop();
        let _ = self.worker.join();
    }
}

fn supervise(options: &Options, editor: Option<&Bridge>, stop: &AtomicBool) -> io::Result<()> {
    let staging = Staging::new()?;
    let mut game: Option<RunningGame> = None;
    let mut observed = None;
    let mut watch = options.watch.clone();
    let mut rebuild = None;
    let mut generation = editor.map_or(0, Bridge::last_generation);
    let enabled = || editor.is_none_or(|bridge| bridge.lifecycle() == Lifecycle::Enabled);

    while !stop.load(Ordering::Relaxed) {
        if let Some(bridge) = editor {
            match bridge.lifecycle() {
                Lifecycle::Disabling => {
                    rebuild = None;
                    if disable_game(bridge, &mut game, stop) {
                        observed = None;
                    }
                    continue;
                }
                Lifecycle::Disabled => {
                    thread::sleep(POLL_INTERVAL);
                    continue;
                }
                Lifecycle::Enabling => {
                    bridge.finish_enable();
                }
                Lifecycle::Enabled => {}
            }
        }
        if let Some(running) = &mut game
            && let Some(status) = running.child.try_wait()?
        {
            if let Some(editor) = editor {
                if status.code() == Some(GPU_PREVIEW_EXIT_CODE) {
                    editor.disconnect(Phase::GpuFailed);
                } else {
                    editor.disconnect(Phase::Exited);
                }
                if let Err(err) = game_exit(status) {
                    eprintln!("{err}");
                }
                game = None;
            } else {
                return game_exit(status);
            }
        }
        let current = scan_sources(&watch)?;
        if observed.as_ref() != Some(&current) {
            if observed.is_none() {
                println!(
                    "watching {} source paths for {}",
                    watch.len(),
                    options.binary
                );
            }
            observed = Some(current);
            rebuild = Some(Instant::now());
        }
        if rebuild.is_some_and(|since| since.elapsed() >= DEBOUNCE) {
            rebuild = None;
            if editor.is_some()
                && let Some(manifest) = &options.manifest
                && let Ok(paths) = project::watch_paths(manifest)
                && paths != watch
            {
                // Cargo.toml can gain new local dependencies while the game is open.
                watch = paths;
                observed = Some(scan_sources(&watch)?);
            }
            generation += 1;
            if let Some(editor) = editor {
                editor.set_phase(Phase::Building);
            }
            match build(options, || stop.load(Ordering::Relaxed) || !enabled())
                .and_then(|binary| staging.copy(&binary, generation))
            {
                Ok(candidate) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if !enabled() {
                        continue;
                    }
                    // Let edits made during a build settle before replacing the live process.
                    if observed.as_ref() != Some(&scan_sources(&watch)?) {
                        continue;
                    }
                    if let Some(running) = &mut game {
                        match running.checkpoint_and_exit(stop) {
                            Ok(Handoff::Ready) => {
                                game = None;
                                if options.timeline.is_some() {
                                    println!("checkpoint committed, starting the new build");
                                } else {
                                    println!("starting the new build without a checkpoint");
                                }
                            }
                            Ok(Handoff::Rejected) => {
                                eprintln!("reload deferred because the game could not checkpoint");
                                if let Some(editor) = editor {
                                    editor.set_phase(Phase::ReloadFailed);
                                }
                                continue;
                            }
                            Ok(Handoff::Exited(status)) if editor.is_none() => {
                                return game_exit(status);
                            }
                            Ok(Handoff::Exited(status)) => {
                                game = None;
                                if let Err(err) = game_exit(status) {
                                    eprintln!("{err}");
                                }
                            }
                            Err(err) => {
                                if !stop.load(Ordering::Relaxed) {
                                    eprintln!("could not request a reload: {err}");
                                    if let Some(editor) = editor {
                                        editor.set_phase(Phase::ReloadFailed);
                                    }
                                }
                                continue;
                            }
                        }
                    }
                    let address = editor
                        .map(|bridge| bridge.endpoint(generation))
                        .transpose()?;
                    if !enabled() {
                        continue;
                    }
                    match RunningGame::launch(
                        candidate,
                        options.timeline.as_deref(),
                        address,
                        options.manifest.as_deref().and_then(Path::parent),
                        options.runtime.as_deref(),
                    ) {
                        Ok(child) => game = Some(child),
                        Err(err) => {
                            if let Some(editor) = editor {
                                editor.disconnect(Phase::Failed);
                                game = None;
                                eprintln!("could not start the game: {err}");
                            } else {
                                return Err(err);
                            }
                        }
                    }
                }
                Err(_) if !enabled() => continue,
                Err(err) if !stop.load(Ordering::Relaxed) => {
                    eprintln!(
                        "{}: build failed: {err}. Waiting for another source change",
                        options.package
                    );
                    if let Some(editor) = editor {
                        editor.set_phase(Phase::Failed);
                    }
                }
                Err(_) => break,
            }
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}

fn disable_game(bridge: &Bridge, game: &mut Option<RunningGame>, stop: &AtomicBool) -> bool {
    let frame = match bridge.freeze_preview() {
        Ok(frame) => frame,
        Err(err) => {
            eprintln!("could not capture the disabled preview: {err}");
            bridge.reject_disable();
            return false;
        }
    };
    let frozen_state = frame
        .as_ref()
        .and_then(|frame| frame.stamp)
        .and_then(|stamp| {
            bridge
                .capture_state(stamp)
                .map_err(|err| eprintln!("could not save the frozen preview state: {err}"))
                .ok()
        });
    let handoff = game.as_mut().map_or(Ok(Handoff::Ready), |running| {
        running.checkpoint_and_exit(stop)
    });
    match handoff {
        Ok(Handoff::Ready) => {
            *game = None;
            bridge.finish_disable(frame, frozen_state);
            true
        }
        Ok(Handoff::Rejected) => {
            eprintln!("disable failed because the game could not checkpoint");
            bridge.reject_disable();
            false
        }
        Ok(Handoff::Exited(status)) => {
            eprintln!("the game exited during disable with {status}");
            *game = None;
            bridge.disconnect(Phase::DisableFailed);
            bridge.reject_disable();
            false
        }
        Err(err) => {
            eprintln!("could not stop the game: {err}");
            bridge.reject_disable();
            false
        }
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

fn build(options: &Options, canceled: impl Fn() -> bool) -> io::Result<PathBuf> {
    let mut command = Command::new(cargo());
    command.args([
        "build",
        "--message-format=json-render-diagnostics",
        "--package",
        &options.package,
        "--bin",
        &options.binary,
    ]);
    if let Some(manifest) = &options.manifest {
        command.arg("--manifest-path").arg(manifest);
        command.current_dir(manifest.parent().unwrap());
    }
    if options.release {
        command.arg("--release");
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut build = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = build.stdout.take().expect("Cargo stdout is piped");
    let messages = thread::spawn(move || read_build_messages(stdout));
    let status = loop {
        if canceled() {
            #[cfg(unix)]
            // The Cargo child leads its own process group, including any active rustc jobs.
            unsafe {
                libc::kill(-(build.id() as i32), libc::SIGKILL);
            }
            let _ = build.kill();
            let _ = build.wait();
            return Err(io::Error::new(io::ErrorKind::Interrupted, "build canceled"));
        }
        if let Some(status) = build.try_wait()? {
            break status;
        }
        thread::sleep(POLL_INTERVAL);
    };
    let executable = messages
        .join()
        .map_err(|_| io::Error::other("Cargo output reader stopped"))??;
    if !status.success() {
        return Err(io::Error::other(format!("Cargo exited with {status}")));
    }
    executable.ok_or_else(|| io::Error::other("Cargo did not report a game executable"))
}

fn read_build_messages(stdout: impl io::Read) -> io::Result<Option<PathBuf>> {
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
    Ok(executable)
}

struct Staging(PathBuf);

impl Staging {
    fn new() -> io::Result<Self> {
        Self::in_directory(&env::temp_dir())
    }

    fn in_directory(parent: &Path) -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_STAGE.fetch_add(1, Ordering::Relaxed);
        let directory = parent.join(format!(
            "sycho-overseer-{}-{}-{sequence}",
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
    fn launch(
        executable: StagedGame,
        timeline: Option<&Path>,
        address: Option<(SocketAddr, Option<String>, DisplayMetrics)>,
        project_root: Option<&Path>,
        runtime: Option<&Path>,
    ) -> io::Result<Self> {
        let mut command = Command::new(&executable.0);
        if let Some(root) = project_root {
            command.current_dir(root);
        }
        if let Some(timeline) = timeline {
            command.arg("--timeline").arg(timeline);
        } else {
            command.arg("--no-recording");
        }
        if let Some(runtime) = runtime {
            command.env(engine::launch::RUNTIME_DIRECTORY_ENV, runtime);
        }
        command.env(MANAGED_ENV, "1");
        command.env_remove(SURFACE_ENV);
        if let Some((address, surface, display)) = address {
            command.env(ADDRESS_ENV, address.to_string());
            command.env(DISPLAY_ENV, serde_json::to_string(&display)?);
            if let Some(surface) = surface {
                command.env(SURFACE_ENV, surface);
            }
        } else {
            command.env_remove(ADDRESS_ENV);
            command.env_remove(DISPLAY_ENV);
        }
        let mut child = command
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
        println!(
            "started game process {} ({})",
            child.id(),
            project_root.map_or_else(|| "standalone".into(), |root| root.display().to_string())
        );
        Ok(Self {
            child,
            handoff_failed,
            _executable: executable,
        })
    }

    fn checkpoint_and_exit(&mut self, stop: &AtomicBool) -> io::Result<Handoff> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(Handoff::Exited(status));
        }
        let stdin = self.child.stdin.as_mut().expect("game stdin is piped");
        writeln!(stdin, "{RELOAD_REQUEST}")?;
        stdin.flush()?;
        let mut slow = false;
        let start = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "checkpoint canceled",
                ));
            }
            if let Some(status) = self.child.try_wait()? {
                return Ok(if status.code() == Some(RELOAD_EXIT_CODE) {
                    Handoff::Ready
                } else {
                    Handoff::Exited(status)
                });
            }
            if self.handoff_failed.try_recv().is_ok() {
                return Ok(Handoff::Rejected);
            }
            if !slow && start.elapsed() >= Duration::from_secs(5) {
                slow = true;
                eprintln!("waiting for the running game to commit its state");
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn stop(&mut self) {
        // Closing the supervisor pipe asks the game to commit its final state and exit.
        self.child.stdin.take();
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(POLL_INTERVAL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for RunningGame {
    fn drop(&mut self) {
        self.stop();
    }
}
