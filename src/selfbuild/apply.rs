//! The apply sequence: bring the worktree up to the `user` tip, build and gate it, stage the exe,
//! then fast-forward `user` to exactly what was tested. `user` only moves at the very end, so any
//! earlier failure leaves it (and the running app) untouched.

use std::path::{Path, PathBuf};

use super::build::{self, Job};
use super::clone::USER_BRANCH;
use super::paths::Paths;
use super::review;
use super::run::{Events, Log, Outcome, Stage};
use crate::backend::git::{self, Merged, Worktree};

const COMMIT_MESSAGE: &str = "Agentory self-build change";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub exe: PathBuf,
    pub build_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The worktree conflicts with `user`. The merge is left in progress in the worktree for an
    /// agent session to resolve; nothing was built.
    Conflict(Vec<String>),
    /// The build, the tests or the smoke launch failed.
    Failed(Outcome),
    /// Git or file trouble. `user` was not changed.
    Error(String),
    /// `user` now holds the tested change and the exe waits at `Staged::exe`.
    Ready(Staged),
}

/// Run the whole sequence. Progress goes to `events`.
pub async fn apply(paths: &Paths, wt: &Worktree, events: Events) -> Applied {
    let mut log = Log::new(events.clone());
    match merge_step(paths, wt, &mut log).await {
        Ok(None) => {}
        Ok(Some(files)) => return Applied::Conflict(files),
        Err(e) => return Applied::Error(e),
    }

    let job = match Job::for_tree(paths, wt.path.clone()).await {
        Ok(j) => j,
        Err(e) => return Applied::Error(e),
    };
    let outcome = build::run(&job, events).await;
    if !outcome.passed() {
        return Applied::Failed(outcome);
    }

    match stage_and_merge(paths, wt, &job, &mut log).await {
        Ok(staged) => Applied::Ready(staged),
        Err(e) => {
            let _ = std::fs::remove_file(paths.staged_exe());
            Applied::Error(e)
        }
    }
}

/// Stage 1: commit what the agent left, then merge the current `user` tip into the worktree.
/// `Some(files)` means a conflict.
async fn merge_step(
    paths: &Paths,
    wt: &Worktree,
    log: &mut Log,
) -> Result<Option<Vec<String>>, String> {
    log.stage(Stage::Merge);
    let clone = paths.clone_dir();
    let on = git::current_branch(&clone).await?;
    if on != USER_BRANCH {
        return Err(format!(
            "the source clone is on branch {on}, expected {USER_BRANCH}"
        ));
    }

    // A conflict from an earlier attempt that has not been resolved yet.
    let open = unresolved(&wt.path, git::conflicted_files(&wt.path).await?);
    if !open.is_empty() {
        log.line(format!("unresolved conflicts: {}", open.join(", ")));
        return Ok(Some(open));
    }
    if git::commit_all(&wt.path, COMMIT_MESSAGE).await? {
        log.line("committed the agent's changes");
    }
    match git::merge_branch(&wt.path, USER_BRANCH).await? {
        Merged::Conflict(files) => {
            log.line(format!("merge conflicts in: {}", files.join(", ")));
            return Ok(Some(files));
        }
        Merged::Clean => log.line(format!("{} is up to date with {USER_BRANCH}", wt.branch)),
    }

    // A "resolution" that kept the markers would build only by luck.
    let base = git::merge_base(&wt.path, USER_BRANCH).await?;
    let diff = git::diff_since(&wt.path, &base).await?;
    if review::has_conflict_markers(&diff) {
        let msg = "the change still contains conflict markers";
        log.line(msg);
        return Err(msg.into());
    }
    Ok(None)
}

/// Files git still lists as unmerged are resolved once they no longer hold conflict markers
/// (git only forgets them after `add`, which the commit step does).
fn unresolved(wt: &Path, files: Vec<String>) -> Vec<String> {
    files
        .into_iter()
        .filter(|f| match std::fs::read_to_string(wt.join(f)) {
            Ok(text) => text
                .lines()
                .any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> ")),
            Err(_) => true,
        })
        .collect()
}

/// Copy the tested exe out of the target dir, then move `user`. A failed copy leaves `user`
/// alone; a failed fast-forward means `user` moved since the merge, and the next Apply starts
/// from the new tip.
async fn stage_and_merge(
    paths: &Paths,
    wt: &Worktree,
    job: &Job,
    log: &mut Log,
) -> Result<Staged, String> {
    let staged = paths.staged_exe();
    if let Some(dir) = staged.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::copy(job.exe(), &staged)
        .map_err(|e| format!("could not stage {}: {e}", staged.display()))?;
    log.line(format!("staged {}", staged.display()));

    git::fast_forward(&paths.clone_dir(), &wt.branch)
        .await
        .map_err(|e| {
            format!(
                "{USER_BRANCH} changed while building; apply again to rebuild on the new tip ({e})"
            )
        })?;
    log.line(format!("{USER_BRANCH} now at {}", job.build_id));
    Ok(Staged {
        exe: staged,
        build_id: job.build_id.clone(),
    })
}

/// Remove the worktree and its branch, from the clone that owns them.
pub async fn discard(paths: &Paths, wt: &Worktree) -> Result<(), String> {
    git::remove_worktree(&paths.clone_dir(), wt).await
}

/// `agentory-helper swap` arguments for a staged build.
pub fn swap_args(pid: u32, staged: &Staged) -> Vec<String> {
    vec![
        "swap".into(),
        "--pid".into(),
        pid.to_string(),
        "--new".into(),
        staged.exe.to_string_lossy().into_owned(),
        "--build-id".into(),
        staged.build_id.clone(),
    ]
}

/// Where the helper lives: the env var it sets when it launches the app, else next to this exe
/// (an installed app that was not started through the helper).
pub fn helper_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(HELPER_ENV)
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Some(p);
    }
    let name = format!("agentory-helper{}", std::env::consts::EXE_SUFFIX);
    let sibling = std::env::current_exe().ok()?.with_file_name(name);
    sibling.is_file().then_some(sibling)
}

/// Set by `agentory-helper` on the app it launches.
pub const HELPER_ENV: &str = "AGENTORY_HELPER";

/// Start the helper to swap in `staged` once this process exits. The caller then quits.
pub fn start_swap(staged: &Staged) -> Result<(), String> {
    let helper = helper_exe().ok_or("agentory-helper was not found; reinstall Agentory")?;
    spawn_detached(&helper, &swap_args(std::process::id(), staged))
}

fn spawn_detached(helper: &Path, args: &[String]) -> Result<(), String> {
    let mut cmd = std::process::Command::new(helper);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: outlives this process.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start {}: {e}", helper.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{capture, sh};
    use crate::selfbuild::run::channel;
    use std::fs;

    /// Mirrors the real `build.rs`: the git sha of HEAD becomes the build id.
    const BUILD_RS: &str = r#"
use std::process::Command;
fn git(args: &[&str]) -> String {
    let o = Command::new("git").args(args).output().unwrap();
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}
fn main() {
    println!("cargo:rustc-env=AGENTORY_BUILD_ID={}", git(&["rev-parse", "--short=12", "HEAD"]));
    println!("cargo:rerun-if-changed={}", git(&["rev-parse", "--git-path", "HEAD"]));
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    println!("cargo:rerun-if-changed={}", git(&["rev-parse", "--git-path", &branch]));
}
"#;
    const MAIN_RS: &str = r#"
fn main() {
    if let Ok(p) = std::env::var("AGENTORY_READY_FILE") {
        std::fs::write(p, format!("{}\n{}\n", std::process::id(), env!("AGENTORY_BUILD_ID"))).unwrap();
    }
}
// MARK
#[cfg(test)] mod t { #[test] fn ok() {} }
"#;

    fn main_with(mark: &str) -> String {
        MAIN_RS.replace("// MARK", mark)
    }

    struct Fixture {
        root: PathBuf,
        paths: Paths,
        wt: Worktree,
    }

    impl Fixture {
        fn clone_dir(&self) -> PathBuf {
            self.paths.clone_dir()
        }
        async fn user_sha(&self) -> String {
            git::head_sha(&self.clone_dir()).await.unwrap()
        }
        /// A commit on `user`, as if the user had changed the clone themselves.
        async fn user_commits(&self, file: &str, text: &str) {
            fs::write(self.clone_dir().join(file), text).unwrap();
            sh(&self.clone_dir(), &["add", "-A"]).await;
            sh(&self.clone_dir(), &["commit", "-m", "user change"]).await;
        }
        fn agent_writes(&self, file: &str, text: &str) {
            fs::write(self.wt.path.join(file), text).unwrap();
        }
        async fn apply(&self) -> Applied {
            let (tx, _rx) = channel();
            apply(&self.paths, &self.wt, tx).await
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    /// A tiny crate named like the app, as the `user` branch of a clone, plus an agent worktree.
    async fn fixture(name: &str) -> Fixture {
        let root =
            std::env::temp_dir().join(format!("agentory-apply-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let paths = Paths::in_dirs(root.join("r"), root.join("l"));
        let clone = paths.clone_dir();
        fs::create_dir_all(clone.join("src")).unwrap();
        let manifest = format!(
            "[package]\nname = \"{}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[workspace]\n",
            env!("CARGO_PKG_NAME")
        );
        fs::write(clone.join("Cargo.toml"), manifest).unwrap();
        fs::write(clone.join("build.rs"), BUILD_RS).unwrap();
        fs::write(clone.join("src/main.rs"), main_with("// base")).unwrap();
        fs::write(clone.join(".gitignore"), "target\n").unwrap();
        sh(&clone, &["init", "-b", USER_BRANCH]).await;
        sh(&clone, &["add", "-A"]).await;
        sh(&clone, &["commit", "-m", "init"]).await;
        let wt = Worktree {
            path: root.join("wt"),
            branch: "agentory/m1".into(),
        };
        sh(
            &clone,
            &[
                "worktree",
                "add",
                &wt.path.to_string_lossy(),
                "-b",
                &wt.branch,
            ],
        )
        .await;
        Fixture { root, paths, wt }
    }

    #[tokio::test]
    async fn passing_change_moves_user_and_stages_the_tested_exe() {
        let f = fixture("pass").await;
        let before = f.user_sha().await;
        f.agent_writes("src/main.rs", &main_with("// changed"));
        let Applied::Ready(staged) = f.apply().await else {
            panic!("expected Ready")
        };
        assert!(staged.exe.is_file());
        assert_ne!(f.user_sha().await, before);
        // What is installed is what was tested: user sits at the built commit.
        assert_eq!(f.user_sha().await, staged.build_id);
        assert!(
            fs::read_to_string(f.clone_dir().join("src/main.rs"))
                .unwrap()
                .contains("// changed")
        );
    }

    #[tokio::test]
    async fn failing_gate_leaves_user_and_staging_untouched() {
        let f = fixture("fail").await;
        let before = f.user_sha().await;
        f.agent_writes(
            "src/main.rs",
            &main_with("// x").replace("fn ok() {}", "fn ok() { panic!(\"nope\"); }"),
        );
        let out = f.apply().await;
        assert!(
            matches!(out, Applied::Failed(Outcome::TestsFailed(_))),
            "{out:?}"
        );
        assert_eq!(f.user_sha().await, before);
        assert!(!f.paths.staged_exe().exists());
    }

    #[tokio::test]
    async fn broken_build_leaves_user_untouched() {
        let f = fixture("nobuild").await;
        let before = f.user_sha().await;
        f.agent_writes("src/main.rs", "fn main() { let x: u32 = \"no\"; }");
        assert!(matches!(
            f.apply().await,
            Applied::Failed(Outcome::BuildFailed(_))
        ));
        assert_eq!(f.user_sha().await, before);
    }

    #[tokio::test]
    async fn stale_worktree_is_brought_up_to_the_user_tip_first() {
        let f = fixture("stale").await;
        f.user_commits("extra.txt", "x").await;
        f.agent_writes("src/main.rs", &main_with("// agent"));
        let Applied::Ready(staged) = f.apply().await else {
            panic!("expected Ready")
        };
        assert_eq!(f.user_sha().await, staged.build_id);
        assert!(
            f.clone_dir().join("extra.txt").exists(),
            "the user's own commit survives"
        );
    }

    #[tokio::test]
    async fn conflict_stops_before_building_and_resolution_then_applies() {
        let f = fixture("conflict").await;
        f.user_commits("src/main.rs", &main_with("// user")).await;
        let tip = f.user_sha().await;
        f.agent_writes("src/main.rs", &main_with("// agent"));

        let conflict = Applied::Conflict(vec!["src/main.rs".into()]);
        assert_eq!(f.apply().await, conflict);
        assert_eq!(f.user_sha().await, tip);
        // Still unresolved on the next click: the same answer, never a silent resolution.
        assert_eq!(f.apply().await, conflict);

        f.agent_writes("src/main.rs", &main_with("// both"));
        let Applied::Ready(staged) = f.apply().await else {
            panic!("expected Ready after resolving")
        };
        assert_eq!(f.user_sha().await, staged.build_id);
    }

    #[tokio::test]
    async fn committed_conflict_markers_are_rejected() {
        let f = fixture("markers").await;
        f.user_commits("src/main.rs", &main_with("// user")).await;
        let tip = f.user_sha().await;
        f.agent_writes("src/main.rs", &main_with("// agent"));
        assert!(matches!(f.apply().await, Applied::Conflict(_)));
        // Leaving the markers in the file is still a conflict.
        let marked = format!(
            "<<<<<<< HEAD\n{}=======\n>>>>>>> user\n",
            main_with("// agent")
        );
        f.agent_writes("src/main.rs", &marked);
        assert!(matches!(f.apply().await, Applied::Conflict(_)));
        // Staging them (as `git add` would) must not smuggle them in either.
        sh(&f.wt.path, &["add", "-A"]).await;
        let out = f.apply().await;
        assert!(
            matches!(&out, Applied::Error(e) if e.contains("conflict markers")),
            "{out:?}"
        );
        assert_eq!(f.user_sha().await, tip);
    }

    #[tokio::test]
    async fn clone_on_another_branch_is_an_error() {
        let f = fixture("branch").await;
        sh(&f.clone_dir(), &["checkout", "-b", "other"]).await;
        f.agent_writes("src/main.rs", &main_with("// c"));
        let out = f.apply().await;
        assert!(
            matches!(&out, Applied::Error(e) if e.contains("expected user")),
            "{out:?}"
        );
    }

    #[tokio::test]
    async fn discard_removes_worktree_and_branch() {
        let f = fixture("discard").await;
        f.agent_writes("src/main.rs", "// scratch");
        discard(&f.paths, &f.wt).await.unwrap();
        assert!(!f.wt.path.exists());
        let b = capture("git", &["branch", "--list", &f.wt.branch], &f.clone_dir())
            .await
            .unwrap();
        assert!(b.trim().is_empty());
    }

    #[test]
    fn swap_args_match_the_helper_cli() {
        let staged = Staged {
            exe: PathBuf::from("S"),
            build_id: "abc".into(),
        };
        assert_eq!(
            swap_args(7, &staged),
            ["swap", "--pid", "7", "--new", "S", "--build-id", "abc"]
        );
    }
}
