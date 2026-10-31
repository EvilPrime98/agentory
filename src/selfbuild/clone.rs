//! Clone the public repo at the installed version's tag into the local source clone, create the
//! local `user` branch, then warm the shared build cache.

use std::path::Path;
use std::time::Duration;

use super::build::{self, Job};
use super::paths::Paths;
use super::preflight;
use super::run::{Log, Stage, stream};

pub const REPO_URL: &str = "https://github.com/EvilPrime98/agentory.git";
/// The branch that holds the user's changes.
pub const USER_BRANCH: &str = "user";

const GIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Release tag of the running version, e.g. `v0.1.0`.
pub fn version_tag() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

pub fn clone_args(url: &str, tag: &str, dir: &Path) -> Vec<String> {
    vec![
        "clone".into(),
        "--branch".into(),
        tag.into(),
        url.into(),
        dir.to_string_lossy().into_owned(),
    ]
}

pub fn is_cloned(paths: &Paths) -> bool {
    paths.clone_dir().join(".git").exists()
}

/// A finished enable leaves a clone and a built exe behind.
pub fn is_ready(paths: &Paths) -> bool {
    is_cloned(paths) && paths.built_exe().is_file()
}

fn git() -> String {
    preflight::which("git")
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "git".into())
}

/// Clone at the version tag and create `user`. Does nothing when the clone already exists, so an
/// interrupted or repeated enable continues instead of failing.
pub async fn clone(paths: &Paths, log: &mut Log) -> Result<(), String> {
    let dir = paths.clone_dir();
    if is_cloned(paths) {
        log.line(format!("source already cloned at {}", dir.display()));
        return Ok(());
    }
    if dir.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!(
            "{} exists and is not a git clone; move or delete it and try again",
            dir.display()
        ));
    }
    let parent = dir.parent().ok_or("bad clone path")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;

    let tag = version_tag();
    let args = clone_args(REPO_URL, &tag, &dir);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let exit = stream(&git(), &args, parent, &[], GIT_TIMEOUT, log).await?;
    if !exit.status.success() {
        // A failed clone can leave a partial directory that would block the retry.
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!(
            "git clone of tag {tag} failed ({}). Check the network connection and that the release tag exists.",
            exit.status
        ));
    }
    let exit = stream(&git(), &["checkout", "-b", USER_BRANCH], &dir, &[], GIT_TIMEOUT, log).await?;
    if !exit.status.success() {
        return Err(format!("could not create the {USER_BRANCH} branch ({})", exit.status));
    }
    Ok(())
}

/// Tell the installer that `active.exe` belongs to the user's builds now.
pub fn write_marker(paths: &Paths) -> Result<(), String> {
    let marker = paths.selfbuild_marker();
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&marker, "self-build enabled\n").map_err(|e| format!("could not write {}: {e}", marker.display()))
}

/// Clone (if needed), then the first `cargo build --release` into the shared target dir.
pub async fn enable(paths: &Paths, log: &mut Log) -> Result<(), String> {
    clone(paths, log).await?;
    write_marker(paths)?;
    let job = Job::for_tree(paths, paths.clone_dir()).await?;
    log.stage(Stage::Build);
    build::build(&job, log).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_matches_cargo_version() {
        assert_eq!(version_tag(), format!("v{}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn clone_args_pin_the_tag() {
        let args = clone_args("https://x/y.git", "v1.2.3", Path::new("D"));
        assert_eq!(args, ["clone", "--branch", "v1.2.3", "https://x/y.git", "D"]);
    }

    #[test]
    fn marker_is_written_where_the_installer_looks() {
        let root = std::env::temp_dir().join(format!("agentory-marker-{}", std::process::id()));
        let paths = Paths::in_dirs(root.join("r"), root.join("l"));
        write_marker(&paths).unwrap();
        assert!(paths.selfbuild_marker().is_file());
        assert_eq!(paths.selfbuild_marker().file_name().unwrap(), "selfbuild");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn readiness_needs_clone_and_exe() {
        let root = std::env::temp_dir().join(format!("agentory-clone-{}", std::process::id()));
        let paths = Paths::in_dirs(root.join("r"), root.join("l"));
        assert!(!is_cloned(&paths) && !is_ready(&paths));
        std::fs::create_dir_all(paths.clone_dir().join(".git")).unwrap();
        assert!(is_cloned(&paths) && !is_ready(&paths));
        std::fs::create_dir_all(paths.built_exe().parent().unwrap()).unwrap();
        std::fs::write(paths.built_exe(), "").unwrap();
        assert!(is_ready(&paths));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
