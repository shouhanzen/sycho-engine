use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::{Options, Staging, cargo, invalid};
use engine::launch::project_timeline;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub manifest: PathBuf,
    pub package: String,
    pub binary: String,
}

impl Project {
    pub fn label(&self) -> &str {
        if self.binary == "headful" || self.binary == self.package {
            &self.package
        } else {
            &self.binary
        }
    }

    pub fn root(&self) -> &Path {
        self.manifest
            .parent()
            .expect("project manifest has a parent")
    }

    pub fn instance_root(&self, instance: &str) -> PathBuf {
        self.root()
            .join(".sycho/instances")
            .join(&self.binary)
            .join(instance)
    }

    pub fn reserve_instance(&self) -> io::Result<String> {
        let base = self.root().join(".sycho/instances").join(&self.binary);
        fs::create_dir_all(&base)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for sequence in 0_u64.. {
            let instance = format!("{now:x}-{sequence:x}");
            match fs::create_dir(self.instance_root(&instance)) {
                Ok(()) => return Ok(instance),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
        }
        unreachable!()
    }

    pub fn options(&self, release: bool, timeline: Option<PathBuf>) -> io::Result<Options> {
        let timeline = timeline.unwrap_or_else(|| project_timeline(self.root(), &self.binary));
        Ok(Options {
            manifest: Some(self.manifest.clone()),
            package: self.package.clone(),
            binary: self.binary.clone(),
            timeline: Some(timeline),
            runtime: None,
            watch: watch_paths(&self.manifest)?,
            release,
        })
    }
}

#[derive(Deserialize)]
struct Metadata {
    workspace_root: PathBuf,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    default_run: Option<String>,
    targets: Vec<Target>,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Target {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

#[derive(Deserialize)]
struct Dependency {
    name: String,
    path: Option<PathBuf>,
}

fn metadata(manifest: &Path) -> io::Result<Metadata> {
    let output = Command::new(cargo())
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(manifest)
        .current_dir(manifest.parent().expect("manifest has a parent"))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn watch_paths(manifest: &Path) -> io::Result<Vec<PathBuf>> {
    let metadata = metadata(manifest)?;
    let mut watch = BTreeSet::new();
    let mut visited = BTreeSet::new();
    watch_package(manifest, metadata, &mut visited, &mut watch)?;
    Ok(watch.into_iter().collect())
}

fn watch_package(
    manifest: &Path,
    metadata: Metadata,
    visited: &mut BTreeSet<PathBuf>,
    watch: &mut BTreeSet<PathBuf>,
) -> io::Result<()> {
    if !visited.insert(manifest.to_path_buf()) {
        return Ok(());
    }
    let root = manifest.parent().expect("manifest has a parent");
    watch.extend([
        manifest.to_path_buf(),
        root.join("src"),
        root.join("assets"),
        metadata.workspace_root.join("Cargo.toml"),
        metadata.workspace_root.join("Cargo.lock"),
    ]);
    if let Some(package) = metadata
        .packages
        .into_iter()
        .find(|p| p.manifest_path == manifest)
    {
        watch.extend(package.targets.into_iter().map(|target| target.src_path));
        for dependency in package.dependencies.into_iter().filter_map(|dep| dep.path) {
            let dependency = dependency.join("Cargo.toml").canonicalize()?;
            if !visited.contains(&dependency) {
                let dependency_metadata = self::metadata(&dependency)?;
                watch_package(&dependency, dependency_metadata, visited, watch)?;
            }
        }
    }
    Ok(())
}

pub fn discover(path: &Path) -> io::Result<Vec<Project>> {
    let manifest = if path.is_dir() {
        path.join("Cargo.toml")
    } else {
        path.to_path_buf()
    }
    .canonicalize()?;
    let metadata = metadata(&manifest)?;
    let has_package = metadata
        .packages
        .iter()
        .any(|package| package.manifest_path == manifest);
    let mut projects = Vec::new();
    for package in metadata
        .packages
        .into_iter()
        .filter(|package| !has_package || package.manifest_path == manifest)
    {
        if !package
            .dependencies
            .iter()
            .any(|dependency| dependency.name == "engine")
        {
            continue;
        }
        let mut targets: Vec<_> = package
            .targets
            .into_iter()
            .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
            .collect();
        if let Some(default) = package.default_run {
            targets.sort_by_key(|target| target.name != default);
        }
        projects.extend(targets.into_iter().map(|target| Project {
            manifest: package.manifest_path.clone(),
            package: package.name.clone(),
            binary: target.name,
        }));
    }
    if projects.is_empty() {
        return Err(invalid(
            "Cargo found no game binary with a Sycho engine dependency",
        ));
    }
    Ok(projects)
}

pub fn scaffold(destination: &Path, engine: &Path) -> io::Result<Project> {
    let name = destination.file_name().and_then(|name| name.to_str())
        .filter(|name| name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
        .ok_or_else(|| invalid("Use a folder name that starts with a letter and contains only letters, numbers, hyphens or underscores"))?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    let destination = parent.join(name);
    let engine = engine.canonicalize()?;
    let engine_path = engine
        .to_str()
        .ok_or_else(|| invalid("The engine path must be valid UTF-8"))?;
    if destination.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Choose a new folder for the game",
        ));
    }

    // Stage beside the destination so publishing the completed project is atomic.
    let staging = Staging::in_directory(&parent)?;
    let manifest = format!(
        "[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n\n[dependencies]\nengine = {{ path = {} }}\nserde = {{ version = \"1\", features = [\"derive\"] }}\n",
        serde_json::to_string(engine_path)?
    );
    fs::create_dir(staging.0.join("src"))?;
    fs::write(staging.0.join("Cargo.toml"), manifest)?;
    fs::write(
        staging.0.join("src/main.rs"),
        include_str!("starter.rs.txt"),
    )?;
    fs::write(staging.0.join(".gitignore"), "/target/\n/.sycho/\n")?;
    fs::rename(&staging.0, &destination)?;

    Ok(Project {
        manifest: destination.join("Cargo.toml"),
        package: name.into(),
        binary: name.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_is_an_independent_cargo_project_and_declares_the_engine() {
        let staging = Staging::new().unwrap();
        let destination = staging.0.join("first-game");
        let game = scaffold(&destination, crate::engine_root()).unwrap();
        assert_eq!(discover(&destination).unwrap(), vec![game.clone()]);
        assert_eq!(discover(&game.manifest).unwrap(), vec![game.clone()]);
        assert!(scaffold(&destination, crate::engine_root()).is_err());
        let options = game.options(false, None).unwrap();
        assert!(options.watch.contains(&crate::engine_root().join("src")));
        assert_eq!(
            options.timeline,
            Some(project_timeline(game.root(), &game.binary))
        );
        let bin = destination.join("src/bin");
        fs::create_dir(bin.clone()).unwrap();
        fs::write(bin.join("second.rs"), "fn main() {}").unwrap();
        assert_eq!(
            discover(&destination)
                .unwrap()
                .into_iter()
                .map(|game| game.binary)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["first-game".to_owned(), "second".to_owned()])
        );
    }

    #[test]
    fn workspace_discovery_does_not_offer_the_editor_as_a_game() {
        let projects = discover(&crate::engine_root().join("..")).unwrap();
        assert!(
            projects
                .iter()
                .any(|project| project.package == "tetree" && project.binary == "headful")
        );
        assert!(projects.iter().all(|project| project.binary != "overseer"));
    }
}
