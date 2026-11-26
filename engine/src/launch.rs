use std::{
    env,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use crate::timeline::session_directory;

pub const RUNTIME_DIRECTORY_ENV: &str = "SYCHO_RUNTIME_DIR";

/// Instance-local storage for mutable game files.
pub fn runtime_directory() -> Option<PathBuf> {
    env::var_os(RUNTIME_DIRECTORY_ENV).map(PathBuf::from)
}

/// Durable recording location shared by the standalone game and the editor.
pub fn project_timeline(project: &Path, binary: &str) -> PathBuf {
    project.join(".sycho/timelines").join(binary)
}

/// Relocate a legacy default recording only while no process is using it.
pub fn migrate_timeline(old: &Path, new: &Path) -> io::Result<()> {
    if new.exists() || !old.is_dir() {
        return Ok(());
    }
    let lock = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(old.join("writer.lock"))
    {
        Ok(lock) => lock,
        Err(err) if err.kind() == io::ErrorKind::NotFound && new.is_dir() => return Ok(()),
        Err(err) => return Err(err),
    };
    lock.try_lock().map_err(|err| {
        io::Error::other(format!(
            "close the game before moving the recording at {}: {err}",
            old.display()
        ))
    })?;
    if !new.exists() {
        fs::create_dir_all(new.parent().expect("project timeline has a parent"))?;
        fs::rename(old, new)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct TimelineConfig {
    pub directory: PathBuf,
    pub playback: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RecordingOptions {
    disabled: bool,
    path: Option<PathBuf>,
    fresh: bool,
}

impl RecordingOptions {
    pub fn parse_arg(
        &mut self,
        arg: &str,
        args: &mut impl Iterator<Item = OsString>,
    ) -> io::Result<bool> {
        match arg {
            "--no-recording" => self.disabled = true,
            "--timeline" => self.path = Some(path_arg(arg, args)?),
            "--fresh" => self.fresh = true,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn validate(&self, editor: bool) -> io::Result<()> {
        if self.disabled && editor {
            return Err(invalid_launch("the editor requires timeline recording"));
        }
        if self.disabled && (self.path.is_some() || self.fresh) {
            return Err(invalid_launch(
                "--no-recording cannot be combined with --timeline or --fresh",
            ));
        }
        Ok(())
    }

    pub fn uses_default(&self) -> bool {
        !self.disabled && !self.fresh && self.path.is_none()
    }

    pub fn directory(&self, default: PathBuf) -> io::Result<Option<PathBuf>> {
        if self.disabled {
            return Ok(None);
        }
        session_directory(default, self.path.clone(), self.fresh).map(Some)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TimelineCli {
    pub help: bool,
    pub check_launch: bool,
    pub export_video: Option<PathBuf>,
    profile: bool,
    recording: RecordingOptions,
    playback: bool,
}

/// Options supplied by the host when launching a game.
pub enum RunConfig {
    Live { timeline: Option<TimelineConfig> },
    Profile,
}

impl TimelineCli {
    pub fn uses_default_timeline(&self) -> bool {
        !self.profile && self.recording.uses_default()
    }

    pub fn timeline_path(&self) -> Option<&Path> {
        self.recording.path.as_deref()
    }

    pub fn run_config(&self, default_directory: PathBuf) -> io::Result<RunConfig> {
        if self.profile {
            return Ok(RunConfig::Profile);
        }
        let timeline = self
            .recording
            .directory(default_directory)?
            .map(|directory| TimelineConfig {
                directory,
                playback: self.playback,
            });
        Ok(RunConfig::Live { timeline })
    }
}

fn invalid_launch(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn path_arg(arg: &str, args: &mut impl Iterator<Item = OsString>) -> io::Result<PathBuf> {
    args.next()
        .filter(|path| !path.to_string_lossy().starts_with('-'))
        .map(PathBuf::from)
        .ok_or_else(|| invalid_launch(format!("{arg} requires a path")))
}

pub fn parse_timeline_cli() -> io::Result<TimelineCli> {
    parse_timeline_args(env::args_os().skip(1))
}

fn parse_timeline_args(mut args: impl Iterator<Item = OsString>) -> io::Result<TimelineCli> {
    let mut cli = TimelineCli::default();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        if cli.recording.parse_arg(&arg, &mut args)? {
            continue;
        }
        match arg.as_ref() {
            "-h" | "--help" => cli.help = true,
            "--profile" => cli.profile = true,
            "--check-launch" => cli.check_launch = true,
            "--playback" => cli.playback = true,
            "--export-video" => cli.export_video = Some(path_arg(&arg, &mut args)?),
            _ => {
                return Err(invalid_launch(format!(
                    "unknown argument: {arg} (try --help)"
                )));
            }
        }
    }
    cli.recording.validate(false)?;
    let fresh = cli.recording.fresh;
    let export = cli.export_video.is_some();
    if (fresh && cli.playback) || (export && (fresh || cli.playback)) {
        return Err(invalid_launch(
            "--fresh, --playback, and --export-video are mutually exclusive",
        ));
    }
    if cli.profile && (fresh || cli.playback || cli.timeline_path().is_some() || export) {
        return Err(invalid_launch(
            "--profile cannot be combined with timeline options",
        ));
    }
    if cli.recording.disabled && (cli.playback || export || cli.profile) {
        return Err(invalid_launch(
            "--no-recording cannot be combined with --playback, --export-video, or --profile",
        ));
    }
    Ok(cli)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_migration_preserves_contents_and_requires_an_idle_recording() {
        let root = env::temp_dir().join(format!(
            "sycho-timeline-migration-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let old = root.join("target/timelines/headful");
        let new = project_timeline(&root.join("games/tetree"), "headful");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("head.json"), b"checkpoint").unwrap();
        let lock = fs::File::create(old.join("writer.lock")).unwrap();
        lock.try_lock().unwrap();
        assert!(migrate_timeline(&old, &new).is_err());
        assert!(old.is_dir() && !new.exists());
        drop(lock);
        migrate_timeline(&old, &new).unwrap();
        assert!(!old.exists());
        assert_eq!(fs::read(new.join("head.json")).unwrap(), b"checkpoint");
        fs::create_dir_all(&old).unwrap();
        migrate_timeline(&old, &new).unwrap();
        assert!(old.is_dir());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn recording_defaults_on_and_can_be_disabled_for_standalone_play() {
        let default = PathBuf::from("target/timelines/example");
        let cli = parse_timeline_args(std::iter::empty()).unwrap();
        let RunConfig::Live {
            timeline: Some(timeline),
        } = cli.run_config(default.clone()).unwrap()
        else {
            panic!("recording should be enabled by default");
        };
        assert_eq!(timeline.directory, default);

        let cli = parse_timeline_args(["--no-recording"].into_iter().map(OsString::from)).unwrap();
        assert!(matches!(
            cli.run_config(default.clone()).unwrap(),
            RunConfig::Live { timeline: None }
        ));

        let cli = parse_timeline_args(["--profile"].into_iter().map(OsString::from)).unwrap();
        assert!(matches!(
            cli.run_config(default).unwrap(),
            RunConfig::Profile
        ));

        for conflict in [
            "--fresh",
            "--playback",
            "--timeline",
            "--export-video",
            "--profile",
        ] {
            let args = ["--no-recording", conflict]
                .into_iter()
                .chain(matches!(conflict, "--timeline" | "--export-video").then_some("example"))
                .map(OsString::from);
            assert_eq!(
                parse_timeline_args(args).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
