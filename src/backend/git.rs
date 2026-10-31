//! Git worktrees for isolated sessions.

use std::path::{Path, PathBuf};

use super::capture;

/// A worktree created for a session.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Branch/folder name for a new session: `agentory/<slug>-<id>`.
pub fn branch_name(session_id: &str) -> String {
    format!("agentory/{session_id}")
}

/// `<data dir>/Agentory/worktrees/<project>/<session id>`, outside the repo so it never shows up
/// in the project's own status.
pub fn worktree_path(project_name: &str, session_id: &str) -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Agentory")
        .join("worktrees")
        .join(sanitize(project_name))
        .join(session_id)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `git worktree add <path> -b <branch>` from `repo`'s current HEAD.
pub async fn add_worktree(
    repo: &Path,
    project_name: &str,
    session_id: &str,
) -> Result<Worktree, String> {
    let path = worktree_path(project_name, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let branch = branch_name(session_id);
    let path_str = path.to_string_lossy().into_owned();
    capture("git", &["worktree", "add", &path_str, "-b", &branch], repo).await?;
    Ok(Worktree { path, branch })
}

// ---- review and apply (self-build) ---------------------------------------------------------

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    capture("git", args, dir).await
}

/// The commit `branch` and the worktree's HEAD last had in common.
pub async fn merge_base(wt: &Path, branch: &str) -> Result<String, String> {
    Ok(git(wt, &["merge-base", "HEAD", branch]).await?.trim().to_string())
}

#[cfg(test)]
pub async fn head_sha(dir: &Path) -> Result<String, String> {
    Ok(git(dir, &["rev-parse", "--short=12", "HEAD"]).await?.trim().to_string())
}

pub async fn current_branch(dir: &Path) -> Result<String, String> {
    Ok(git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).await?.trim().to_string())
}

/// Stage everything, so new files show up in the diff and are committed with the rest.
pub async fn stage_all(wt: &Path) -> Result<(), String> {
    git(wt, &["add", "-A"]).await.map(|_| ())
}

/// Everything the worktree changed relative to `base` (a commit), staged or not, new files
/// included. Stages the worktree as a side effect.
pub async fn diff_since(wt: &Path, base: &str) -> Result<String, String> {
    stage_all(wt).await?;
    git(wt, &["diff", "--cached", "--no-color", "--no-ext-diff", base]).await
}

/// Files in conflict (unmerged), if any.
pub async fn conflicted_files(wt: &Path) -> Result<Vec<String>, String> {
    let out = git(wt, &["diff", "--name-only", "--diff-filter=U"]).await?;
    Ok(out.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
}

/// Commit all changes in the worktree. `false` when there was nothing to commit. Completes a
/// merge the user resolved by hand.
pub async fn commit_all(wt: &Path, message: &str) -> Result<bool, String> {
    stage_all(wt).await?;
    let merging = git(wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_ok();
    if !merging && git(wt, &["diff", "--cached", "--quiet"]).await.is_ok() {
        return Ok(false);
    }
    // A machine without a git identity still has to be able to commit.
    let has_identity = git(wt, &["config", "user.email"]).await.is_ok_and(|s| !s.trim().is_empty());
    let mut args = Vec::new();
    if !has_identity {
        args.extend(["-c", "user.name=Agentory", "-c", "user.email=agentory@localhost"]);
    }
    args.extend(["commit", "-m", message]);
    git(wt, &args).await.map(|_| true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    Clean,
    /// The merge stopped on these files and is left in progress in the worktree.
    Conflict(Vec<String>),
}

/// Merge `branch` (the `user` tip) into the worktree's branch.
pub async fn merge_branch(wt: &Path, branch: &str) -> Result<Merged, String> {
    match git(wt, &["merge", "--no-edit", branch]).await {
        Ok(_) => Ok(Merged::Clean),
        Err(e) => match conflicted_files(wt).await {
            Ok(files) if !files.is_empty() => Ok(Merged::Conflict(files)),
            _ => Err(e),
        },
    }
}

/// Move the checked-out branch of `repo` to `branch` without creating a merge commit.
pub async fn fast_forward(repo: &Path, branch: &str) -> Result<(), String> {
    git(repo, &["merge", "--ff-only", branch]).await.map(|_| ())
}

/// Fetch tags from `origin` without overwriting local ones: a tag upstream moved makes this fail
/// instead of silently changing the merge base.
pub async fn fetch_tags(repo: &Path) -> Result<(), String> {
    git(repo, &["fetch", "origin", "--tags"]).await.map(|_| ())
}

pub async fn list_tags(repo: &Path) -> Result<Vec<String>, String> {
    let out = git(repo, &["tag", "--list"]).await?;
    Ok(out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// The nearest tag reachable from `rev`: the release the branch is based on.
pub async fn base_tag(repo: &Path, rev: &str) -> Result<String, String> {
    Ok(git(repo, &["describe", "--tags", "--abbrev=0", rev]).await?.trim().to_string())
}

/// Commits reachable from `to` but not from `from`.
pub async fn count_commits(repo: &Path, from: &str, to: &str) -> Result<usize, String> {
    let range = format!("{from}..{to}");
    let out = git(repo, &["rev-list", "--count", &range]).await?;
    out.trim().parse().map_err(|_| format!("unexpected rev-list output: {out}"))
}

/// A worktree on a new branch started from `base` (e.g. `user`), for an upstream merge.
pub async fn add_worktree_from(repo: &Path, project_name: &str, session_id: &str, base: &str) -> Result<Worktree, String> {
    let path = worktree_path(project_name, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let branch = branch_name(session_id);
    let path_str = path.to_string_lossy().into_owned();
    capture("git", &["worktree", "add", &path_str, "-b", &branch, base], repo).await?;
    Ok(Worktree { path, branch })
}

/// Delete the worktree folder and its branch. Missing pieces are fine, so a half-removed
/// worktree can be cleaned up again.
pub async fn remove_worktree(repo: &Path, wt: &Worktree) -> Result<(), String> {
    let path = wt.path.to_string_lossy().into_owned();
    let removed = git(repo, &["worktree", "remove", "--force", &path]).await;
    if wt.path.exists() {
        // `worktree remove` can fail on a locked file; fall back to deleting the folder.
        std::fs::remove_dir_all(&wt.path).map_err(|e| removed.err().unwrap_or_else(|| e.to_string()))?;
    }
    let _ = git(repo, &["worktree", "prune"]).await;
    match git(repo, &["branch", "-D", &wt.branch]).await {
        Ok(_) => Ok(()),
        Err(e) if e.contains("not found") => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::sh;

    #[test]
    fn names_are_filesystem_safe() {
        assert_eq!(sanitize("my proj/é"), "my_proj_é");
        assert_eq!(branch_name("s1"), "agentory/s1");
    }

    // ---- against a real repo ---------------------------------------------------------------

    use std::fs;

    /// A repo on branch `user` with one commit, plus a worktree on a new branch.
    async fn repo(name: &str) -> (PathBuf, Worktree) {
        let root = std::env::temp_dir().join(format!("agentory-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        sh(&main, &["init", "-b", "user"]).await;
        fs::write(main.join("a.txt"), "one
").unwrap();
        sh(&main, &["add", "-A"]).await;
        sh(&main, &["commit", "-m", "init"]).await;
        let wt = Worktree { path: root.join("wt"), branch: "agentory/x".into() };
        let p = wt.path.to_string_lossy().into_owned();
        sh(&main, &["worktree", "add", &p, "-b", &wt.branch]).await;
        (main, wt)
    }

    fn cleanup(main: &Path) {
        let _ = fs::remove_dir_all(main.parent().unwrap());
    }

    #[tokio::test]
    async fn diff_includes_new_and_edited_files() {
        let (main, wt) = repo("diff").await;
        fs::write(wt.path.join("a.txt"), "two
").unwrap();
        fs::write(wt.path.join("new.txt"), "fresh
").unwrap();
        let base = merge_base(&wt.path, "user").await.unwrap();
        let diff = diff_since(&wt.path, &base).await.unwrap();
        assert!(diff.contains("+two") && diff.contains("+fresh"), "{diff}");
        cleanup(&main);
    }

    #[tokio::test]
    async fn clean_merge_then_fast_forward_moves_user() {
        let (main, wt) = repo("ff").await;
        fs::write(main.join("b.txt"), "other
").unwrap();
        sh(&main, &["add", "-A"]).await;
        sh(&main, &["commit", "-m", "user moved"]).await;
        fs::write(wt.path.join("a.txt"), "agent
").unwrap();
        assert!(commit_all(&wt.path, "change").await.unwrap());
        assert_eq!(merge_branch(&wt.path, "user").await.unwrap(), Merged::Clean);
        fast_forward(&main, &wt.branch).await.unwrap();
        assert_eq!(head_sha(&main).await.unwrap(), head_sha(&wt.path).await.unwrap());
        assert_eq!(fs::read_to_string(main.join("a.txt")).unwrap().trim(), "agent");
        cleanup(&main);
    }

    #[tokio::test]
    async fn nothing_to_commit_is_false() {
        let (main, wt) = repo("empty").await;
        assert!(!commit_all(&wt.path, "x").await.unwrap());
        cleanup(&main);
    }

    #[tokio::test]
    async fn conflict_is_reported_and_user_is_untouched() {
        let (main, wt) = repo("conflict").await;
        fs::write(main.join("a.txt"), "user side
").unwrap();
        sh(&main, &["commit", "-am", "user edit"]).await;
        let before = head_sha(&main).await.unwrap();
        fs::write(wt.path.join("a.txt"), "agent side
").unwrap();
        commit_all(&wt.path, "agent edit").await.unwrap();
        let merged = merge_branch(&wt.path, "user").await.unwrap();
        assert_eq!(merged, Merged::Conflict(vec!["a.txt".into()]));
        // Resolving by hand and committing completes the merge.
        fs::write(wt.path.join("a.txt"), "both
").unwrap();
        assert!(commit_all(&wt.path, "resolve").await.unwrap());
        assert!(conflicted_files(&wt.path).await.unwrap().is_empty());
        assert_eq!(head_sha(&main).await.unwrap(), before);
        cleanup(&main);
    }

    #[tokio::test]
    async fn fast_forward_fails_when_user_moved_after_the_merge() {
        let (main, wt) = repo("stale").await;
        fs::write(wt.path.join("a.txt"), "agent
").unwrap();
        commit_all(&wt.path, "change").await.unwrap();
        fs::write(main.join("b.txt"), "late
").unwrap();
        sh(&main, &["add", "-A"]).await;
        sh(&main, &["commit", "-m", "late"]).await;
        let before = head_sha(&main).await.unwrap();
        assert!(fast_forward(&main, &wt.branch).await.is_err());
        assert_eq!(head_sha(&main).await.unwrap(), before);
        cleanup(&main);
    }

    #[tokio::test]
    async fn removing_leaves_no_worktree_or_branch() {
        let (main, wt) = repo("remove").await;
        fs::write(wt.path.join("a.txt"), "dirty
").unwrap();
        remove_worktree(&main, &wt).await.unwrap();
        assert!(!wt.path.exists());
        let branches = capture("git", &["branch", "--list", &wt.branch], &main).await.unwrap();
        assert!(branches.trim().is_empty());
        let list = capture("git", &["worktree", "list"], &main).await.unwrap();
        assert_eq!(list.lines().count(), 1, "{list}");
        // Doing it again is harmless.
        remove_worktree(&main, &wt).await.unwrap();
        cleanup(&main);
    }
}
