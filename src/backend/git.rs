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
    Ok(git(wt, &["merge-base", "HEAD", branch])
        .await?
        .trim()
        .to_string())
}

#[cfg(test)]
pub async fn head_sha(dir: &Path) -> Result<String, String> {
    Ok(git(dir, &["rev-parse", "--short=12", "HEAD"])
        .await?
        .trim()
        .to_string())
}

pub async fn current_branch(dir: &Path) -> Result<String, String> {
    Ok(git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .await?
        .trim()
        .to_string())
}

/// Stage everything, so new files show up in the diff and are committed with the rest.
pub async fn stage_all(wt: &Path) -> Result<(), String> {
    git(wt, &["add", "-A"]).await.map(|_| ())
}

/// Everything the worktree changed relative to `base` (a commit), staged or not, new files
/// included. Stages the worktree as a side effect.
pub async fn diff_since(wt: &Path, base: &str) -> Result<String, String> {
    stage_all(wt).await?;
    git(
        wt,
        &["diff", "--cached", "--no-color", "--no-ext-diff", base],
    )
    .await
}

/// Files in conflict (unmerged), if any.
pub async fn conflicted_files(wt: &Path) -> Result<Vec<String>, String> {
    let out = git(wt, &["diff", "--name-only", "--diff-filter=U"]).await?;
    Ok(out
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect())
}

/// Commit all changes in the worktree. `false` when there was nothing to commit. Completes a
/// merge the user resolved by hand.
pub async fn commit_all(wt: &Path, message: &str) -> Result<bool, String> {
    stage_all(wt).await?;
    let merging = git(wt, &["rev-parse", "-q", "--verify", "MERGE_HEAD"])
        .await
        .is_ok();
    if !merging && git(wt, &["diff", "--cached", "--quiet"]).await.is_ok() {
        return Ok(false);
    }
    // A machine without a git identity still has to be able to commit.
    let has_identity = git(wt, &["config", "user.email"])
        .await
        .is_ok_and(|s| !s.trim().is_empty());
    let mut args = Vec::new();
    if !has_identity {
        args.extend([
            "-c",
            "user.name=Agentory",
            "-c",
            "user.email=agentory@localhost",
        ]);
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
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// The nearest tag reachable from `rev`: the release the branch is based on.
pub async fn base_tag(repo: &Path, rev: &str) -> Result<String, String> {
    Ok(git(repo, &["describe", "--tags", "--abbrev=0", rev])
        .await?
        .trim()
        .to_string())
}

/// Commits reachable from `to` but not from `from`.
pub async fn count_commits(repo: &Path, from: &str, to: &str) -> Result<usize, String> {
    let range = format!("{from}..{to}");
    let out = git(repo, &["rev-list", "--count", &range]).await?;
    out.trim()
        .parse()
        .map_err(|_| format!("unexpected rev-list output: {out}"))
}

/// A worktree on a new branch started from `base` (e.g. `user`), for an upstream merge.
pub async fn add_worktree_from(
    repo: &Path,
    project_name: &str,
    session_id: &str,
    base: &str,
) -> Result<Worktree, String> {
    let path = worktree_path(project_name, session_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let branch = branch_name(session_id);
    let path_str = path.to_string_lossy().into_owned();
    capture(
        "git",
        &["worktree", "add", &path_str, "-b", &branch, base],
        repo,
    )
    .await?;
    Ok(Worktree { path, branch })
}

/// Delete the worktree folder and its branch. Missing pieces are fine, so a half-removed
/// worktree can be cleaned up again.
pub async fn remove_worktree(repo: &Path, wt: &Worktree) -> Result<(), String> {
    let path = wt.path.to_string_lossy().into_owned();
    let removed = git(repo, &["worktree", "remove", "--force", &path]).await;
    if wt.path.exists() {
        // `worktree remove` can fail on a locked file; fall back to deleting the folder.
        std::fs::remove_dir_all(&wt.path)
            .map_err(|e| removed.err().unwrap_or_else(|| e.to_string()))?;
    }
    let _ = git(repo, &["worktree", "prune"]).await;
    match git(repo, &["branch", "-D", &wt.branch]).await {
        Ok(_) => Ok(()),
        Err(e) if e.contains("not found") => Ok(()),
        Err(e) => Err(e),
    }
}

// ---- source control (Git page) -------------------------------------------------------------

/// How a file differs, as one letter of `git status --porcelain=v2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// The badge letter shown next to the file.
    pub fn letter(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Untracked => "U",
            Self::Conflicted => "!",
        }
    }

    fn from_code(c: char) -> Self {
        match c {
            'A' | 'C' => Self::Added,
            'D' => Self::Deleted,
            'R' => Self::Renamed,
            _ => Self::Modified,
        }
    }
}

/// One changed file in one group (staged, unstaged or conflicts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repo root, `/` separated.
    pub path: String,
    pub kind: ChangeKind,
    /// The previous path of a rename.
    pub from: Option<String>,
}

/// The working tree state: current branch, how far it is from its upstream and the changed files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// `None` while HEAD is detached.
    pub branch: Option<String>,
    pub has_upstream: bool,
    pub ahead: u32,
    pub behind: u32,
    pub staged: Vec<Change>,
    /// Modified, deleted and untracked files that are not staged.
    pub unstaged: Vec<Change>,
    pub conflicts: Vec<Change>,
}

impl Status {
    pub fn total(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.conflicts.len()
    }
}

/// Parse `git status --porcelain=v2 --branch -z --untracked-files=all`.
pub fn parse_status(out: &str) -> Status {
    let mut st = Status::default();
    let mut records = out.split('\0');
    while let Some(rec) = records.next() {
        if let Some(h) = rec.strip_prefix("# ") {
            if let Some(name) = h.strip_prefix("branch.head ") {
                st.branch = (name != "(detached)").then(|| name.to_string());
            } else if h.starts_with("branch.upstream ") {
                st.has_upstream = true;
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                for part in ab.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        st.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        st.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            continue;
        }
        let (tag, rest) = match rec.split_once(' ') {
            Some(p) => p,
            None => continue,
        };
        match tag {
            "1" | "2" => {
                // 1 XY sub mH mI mW hH hI path        2 XY sub mH mI mW hH hI score path \0 orig
                let fields = if tag == "1" { 8 } else { 9 };
                let mut it = rest.splitn(fields, ' ');
                let xy: Vec<char> = it.next().unwrap_or("").chars().collect();
                let Some(path) = it.last() else { continue };
                let from = (tag == "2").then(|| records.next().unwrap_or("").to_string());
                if xy.len() != 2 {
                    continue;
                }
                let make = |c: char| Change {
                    path: path.to_string(),
                    kind: ChangeKind::from_code(c),
                    from: from.clone(),
                };
                if xy[0] != '.' {
                    st.staged.push(make(xy[0]));
                }
                if xy[1] != '.' {
                    st.unstaged.push(make(xy[1]));
                }
            }
            "u" => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                if let Some(path) = rest.splitn(10, ' ').last() {
                    st.conflicts.push(Change {
                        path: path.to_string(),
                        kind: ChangeKind::Conflicted,
                        from: None,
                    });
                }
            }
            "?" => st.unstaged.push(Change {
                path: rest.to_string(),
                kind: ChangeKind::Untracked,
                from: None,
            }),
            _ => {}
        }
    }
    st
}

/// `git` with literal pathspecs, so a file named `a[1].txt` is that file and not a glob.
async fn gitl(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut a = vec!["--literal-pathspecs"];
    a.extend_from_slice(args);
    git(root, &a).await
}

/// The top level of the repo `dir` is inside, or `None` when it is not in one. Every other
/// function here takes this root, since status paths are relative to it.
pub async fn repo_root(dir: &Path) -> Result<Option<PathBuf>, String> {
    match git(dir, &["rev-parse", "--show-toplevel"]).await {
        Ok(out) => Ok(Some(PathBuf::from(out.trim()))),
        Err(e) if e.contains("not a git repository") => Ok(None),
        Err(e) => Err(e),
    }
}

pub async fn init(dir: &Path) -> Result<(), String> {
    git(dir, &["init"]).await.map(|_| ())
}

/// `--no-optional-locks` keeps a status poll from rewriting the index.
pub async fn status(root: &Path) -> Result<Status, String> {
    let out = git(
        root,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ],
    )
    .await?;
    Ok(parse_status(&out))
}

/// Files bigger than this are not read for an untracked-file preview.
const MAX_PREVIEW_BYTES: u64 = 1_000_000;

/// Unified diff of one file. `staged` selects index vs HEAD, otherwise working tree vs index.
/// Untracked files have no git diff, so theirs is built by hand with every line added.
pub async fn file_diff(root: &Path, change: &Change, staged: bool) -> Result<String, String> {
    if change.kind == ChangeKind::Untracked {
        return Ok(untracked_diff(&root.join(&change.path)));
    }
    let mut args = vec!["--no-optional-locks", "diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    if let Some(from) = &change.from {
        args.push(from);
    }
    args.push(&change.path);
    gitl(root, &args).await
}

fn untracked_diff(path: &Path) -> String {
    let too_big = std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_PREVIEW_BYTES);
    if too_big {
        return "Binary files differ\n".to_string();
    }
    match std::fs::read(path) {
        Ok(bytes) if bytes.contains(&0) => "Binary files differ\n".to_string(),
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = text.lines().collect();
            let mut out = format!("@@ -0,0 +1,{} @@\n", lines.len());
            for l in lines {
                out.push('+');
                out.push_str(l);
                out.push('\n');
            }
            out
        }
        Err(e) => format!("could not read the file: {e}\n"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Hunk,
    Add,
    Del,
    Context,
    /// "Binary files differ", "\ No newline at end of file".
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub text: String,
}

/// `(old start, new start)` of a `@@ -a,b +c,d @@` header.
fn hunk_starts(header: &str) -> Option<(u32, u32)> {
    let mut it = header.split_whitespace().skip(1);
    let old = it.next()?.strip_prefix('-')?;
    let new = it.next()?.strip_prefix('+')?;
    let start = |s: &str| s.split(',').next()?.parse().ok();
    Some((start(old)?, start(new)?))
}

/// Turn a unified diff into numbered lines. The file header (`diff --git`, `index`, `---`, `+++`)
/// is dropped: the page already names the file.
pub fn parse_diff(diff: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    for line in diff.lines() {
        if line.starts_with("@@") {
            in_hunk = true;
            if let Some((o, n)) = hunk_starts(line) {
                (old, new) = (o, n);
            }
            out.push(DiffLine {
                kind: LineKind::Hunk,
                old: None,
                new: None,
                text: line.to_string(),
            });
        } else if !in_hunk {
            if line.starts_with("Binary files") || line.starts_with("GIT binary patch") {
                out.push(DiffLine {
                    kind: LineKind::Note,
                    old: None,
                    new: None,
                    text: line.to_string(),
                });
            }
        } else if let Some(t) = line.strip_prefix('+') {
            out.push(DiffLine {
                kind: LineKind::Add,
                old: None,
                new: Some(new),
                text: t.to_string(),
            });
            new += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            out.push(DiffLine {
                kind: LineKind::Del,
                old: Some(old),
                new: None,
                text: t.to_string(),
            });
            old += 1;
        } else if line.starts_with('\\') {
            out.push(DiffLine {
                kind: LineKind::Note,
                old: None,
                new: None,
                text: line.to_string(),
            });
        } else {
            let t = line.strip_prefix(' ').unwrap_or(line);
            out.push(DiffLine {
                kind: LineKind::Context,
                old: Some(old),
                new: Some(new),
                text: t.to_string(),
            });
            old += 1;
            new += 1;
        }
    }
    out
}

pub async fn stage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    gitl(root, &args).await.map(|_| ())
}

pub async fn stage_everything(root: &Path) -> Result<(), String> {
    stage_all(root).await
}

pub async fn unstage(root: &Path, paths: &[String]) -> Result<(), String> {
    let mut args = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    if gitl(root, &args).await.is_ok() {
        return Ok(());
    }
    // A repo with no commit yet has no HEAD to restore from.
    let mut args = vec!["rm", "--cached", "-r", "-q", "--"];
    args.extend(paths.iter().map(String::as_str));
    gitl(root, &args).await.map(|_| ())
}

pub async fn unstage_everything(root: &Path) -> Result<(), String> {
    unstage(root, &[".".to_string()]).await
}

/// Throw away the working-tree change of one file: restore a tracked one, delete an untracked one.
pub async fn discard(root: &Path, change: &Change) -> Result<(), String> {
    if change.kind == ChangeKind::Untracked {
        gitl(root, &["clean", "-f", "-q", "--", &change.path])
            .await
            .map(|_| ())
    } else {
        gitl(root, &["restore", "--", &change.path])
            .await
            .map(|_| ())
    }
}

/// Throw away every unstaged change, untracked files included.
pub async fn discard_everything(root: &Path) -> Result<(), String> {
    gitl(root, &["restore", "--", "."]).await?;
    gitl(root, &["clean", "-f", "-d", "-q"]).await.map(|_| ())
}

/// Commit what is staged. Hooks run. A machine without a git identity still commits.
pub async fn commit(root: &Path, message: &str) -> Result<(), String> {
    let has_identity = git(root, &["config", "user.email"])
        .await
        .is_ok_and(|s| !s.trim().is_empty());
    let mut args = Vec::new();
    if !has_identity {
        args.extend([
            "-c",
            "user.name=Agentory",
            "-c",
            "user.email=agentory@localhost",
        ]);
    }
    args.extend(["commit", "-m", message]);
    git(root, &args).await.map(|_| ())
}

pub async fn fetch(root: &Path) -> Result<(), String> {
    git(root, &["fetch"]).await.map(|_| ())
}

/// Pull, then push (publishing the branch when it has no upstream yet).
pub async fn sync(root: &Path, status: &Status) -> Result<(), String> {
    if status.has_upstream {
        git(root, &["pull"]).await?;
        git(root, &["push"]).await.map(|_| ())
    } else {
        git(root, &["push", "--set-upstream", "origin", "HEAD"])
            .await
            .map(|_| ())
    }
}

/// One branch the switcher can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// What `git switch` takes: the local name, or for a remote-only branch the name without the
    /// remote, which makes git create a tracking branch.
    pub name: String,
    /// The remote of a remote-only branch.
    pub remote: Option<String>,
    pub current: bool,
}

/// Parse `git for-each-ref --format=%(HEAD)%(refname) refs/heads refs/remotes`: local branches
/// first, then remote-only ones (a remote branch that has a local twin is left out).
pub fn parse_branches(out: &str) -> Vec<Branch> {
    let mut local = Vec::new();
    let mut remote: Vec<Branch> = Vec::new();
    for line in out.lines() {
        let current = line.starts_with('*');
        let Some(full) = line.get(1..) else { continue };
        if let Some(name) = full.strip_prefix("refs/heads/") {
            local.push(Branch {
                name: name.to_string(),
                remote: None,
                current,
            });
        } else if let Some(rest) = full.strip_prefix("refs/remotes/") {
            let Some((r, name)) = rest.split_once('/') else {
                continue;
            };
            if name == "HEAD" {
                continue;
            }
            remote.push(Branch {
                name: name.to_string(),
                remote: Some(r.to_string()),
                current: false,
            });
        }
    }
    let mut seen: Vec<String> = local.iter().map(|b| b.name.clone()).collect();
    for b in remote {
        if !seen.contains(&b.name) {
            seen.push(b.name.clone());
            local.push(b);
        }
    }
    local
}

pub async fn branches(root: &Path) -> Result<Vec<Branch>, String> {
    let out = git(
        root,
        &[
            "--no-optional-locks",
            "for-each-ref",
            "--format=%(HEAD)%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )
    .await?;
    Ok(parse_branches(&out))
}

/// Plain `git switch`: when changes conflict, git refuses and its output is the error.
pub async fn switch(root: &Path, name: &str) -> Result<(), String> {
    git(root, &["switch", name]).await.map(|_| ())
}

/// Create a branch at HEAD and switch to it. Git validates the name.
pub async fn create_branch(root: &Path, name: &str) -> Result<(), String> {
    git(root, &["switch", "-c", name]).await.map(|_| ())
}

/// One entry of the stash list; `index` is the `N` of `stash@{N}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stash {
    pub index: usize,
    pub message: String,
}

pub async fn stash_list(root: &Path) -> Result<Vec<Stash>, String> {
    let out = git(
        root,
        &["--no-optional-locks", "stash", "list", "--format=%gs"],
    )
    .await?;
    Ok(out
        .lines()
        .enumerate()
        .map(|(index, l)| Stash {
            index,
            message: l.to_string(),
        })
        .collect())
}

/// Stash staged, unstaged and untracked changes. An empty message uses git's default.
pub async fn stash_push(root: &Path, message: &str) -> Result<(), String> {
    let has_identity = git(root, &["config", "user.email"])
        .await
        .is_ok_and(|s| !s.trim().is_empty());
    let mut args = Vec::new();
    if !has_identity {
        args.extend([
            "-c",
            "user.name=Agentory",
            "-c",
            "user.email=agentory@localhost",
        ]);
    }
    args.extend(["stash", "push", "-u"]);
    let message = message.trim();
    if !message.is_empty() {
        args.extend(["-m", message]);
    }
    git(root, &args).await.map(|_| ())
}

async fn stash_cmd(root: &Path, verb: &str, index: usize) -> Result<(), String> {
    let reference = format!("stash@{{{index}}}");
    git(root, &["stash", verb, &reference]).await.map(|_| ())
}

pub async fn stash_apply(root: &Path, index: usize) -> Result<(), String> {
    stash_cmd(root, "apply", index).await
}

pub async fn stash_pop(root: &Path, index: usize) -> Result<(), String> {
    stash_cmd(root, "pop", index).await
}

pub async fn stash_drop(root: &Path, index: usize) -> Result<(), String> {
    stash_cmd(root, "drop", index).await
}

#[cfg(test)]
mod source_control_tests {
    use super::*;
    use crate::backend::sh;
    use std::fs;

    #[test]
    fn parses_status_groups_and_branch() {
        let out = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 MM N... 100644 100644 100644 a b src/both.rs\0\
1 .D N... 100644 100644 000000 a b gone.rs\0\
1 A. N... 000000 100644 100644 a b new.rs\0\
2 R. N... 100644 100644 100644 a b R100 renamed.rs\0old.rs\0\
u UU N... 1 2 3 4 a b c clash.rs\0\
? loose file.txt\0";
        let st = parse_status(out);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert!(st.has_upstream);
        assert_eq!((st.ahead, st.behind), (2, 1));
        let paths = |v: &[Change]| v.iter().map(|c| c.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&st.staged), ["src/both.rs", "new.rs", "renamed.rs"]);
        assert_eq!(
            paths(&st.unstaged),
            ["src/both.rs", "gone.rs", "loose file.txt"]
        );
        assert_eq!(paths(&st.conflicts), ["clash.rs"]);
        assert_eq!(st.staged[2].kind, ChangeKind::Renamed);
        assert_eq!(st.staged[2].from.as_deref(), Some("old.rs"));
        assert_eq!(st.unstaged[1].kind, ChangeKind::Deleted);
        assert_eq!(st.unstaged[2].kind, ChangeKind::Untracked);
        assert_eq!(st.total(), 7);
    }

    #[test]
    fn branches_list_local_then_remote_only() {
        let out = " refs/heads/dev
*refs/heads/main
 refs/remotes/origin/HEAD
 refs/remotes/origin/main
 refs/remotes/origin/feat/x
";
        let b = parse_branches(out);
        let names: Vec<_> = b.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["dev", "main", "feat/x"]);
        assert!(b[1].current && !b[0].current);
        assert_eq!(b[2].remote.as_deref(), Some("origin"));
    }

    #[tokio::test]
    async fn branch_and_stash_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentory-bs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        init(&dir).await.unwrap();
        let root = repo_root(&dir).await.unwrap().unwrap();
        fs::write(root.join("a.txt"), "one").unwrap();
        stage(&root, &["a.txt".into()]).await.unwrap();
        commit(&root, "first").await.unwrap();
        create_branch(&root, "topic").await.unwrap();
        assert_eq!(
            status(&root).await.unwrap().branch.as_deref(),
            Some("topic")
        );
        let bs = branches(&root).await.unwrap();
        assert!(bs.iter().any(|b| b.name == "topic" && b.current));

        fs::write(root.join("a.txt"), "two").unwrap();
        fs::write(root.join("new.txt"), "n").unwrap();
        stash_push(&root, "wip").await.unwrap();
        assert_eq!(status(&root).await.unwrap().total(), 0);
        let list = stash_list(&root).await.unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].message.contains("wip"));
        stash_apply(&root, 0).await.unwrap();
        assert_eq!(status(&root).await.unwrap().total(), 2);
        discard_everything(&root).await.unwrap();
        stash_pop(&root, 0).await.unwrap();
        assert!(stash_list(&root).await.unwrap().is_empty());
        discard_everything(&root).await.unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detached_head_has_no_branch() {
        let st = parse_status("# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(st.branch, None);
        assert!(!st.has_upstream);
    }

    #[test]
    fn numbers_diff_lines() {
        let diff = "diff --git a/f b/f\nindex 1..2 100644\n--- a/f\n+++ b/f\n@@ -3,3 +3,3 @@ fn x\n keep\n-old\n+new\n tail\n\\ No newline at end of file\n";
        let lines = parse_diff(diff);
        let kinds: Vec<_> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                LineKind::Hunk,
                LineKind::Context,
                LineKind::Del,
                LineKind::Add,
                LineKind::Context,
                LineKind::Note
            ]
        );
        assert_eq!((lines[1].old, lines[1].new), (Some(3), Some(3)));
        assert_eq!((lines[2].old, lines[2].new), (Some(4), None));
        assert_eq!((lines[3].old, lines[3].new), (None, Some(4)));
        assert_eq!((lines[4].old, lines[4].new), (Some(5), Some(5)));
        assert_eq!(lines[3].text, "new");
    }

    #[test]
    fn binary_diff_is_a_note() {
        let lines = parse_diff("diff --git a/x b/x\nBinary files a/x and b/x differ\n");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].kind, LineKind::Note);
    }

    #[tokio::test]
    async fn stage_commit_discard_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agentory-sc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(repo_root(&dir).await.unwrap(), None);
        init(&dir).await.unwrap();
        let root = repo_root(&dir).await.unwrap().expect("repo after init");

        fs::write(dir.join("a.txt"), "one\n").unwrap();
        fs::write(dir.join("w[1].txt"), "x\n").unwrap();
        let st = status(&root).await.unwrap();
        assert_eq!(st.unstaged.len(), 2);
        assert!(st.unstaged.iter().all(|c| c.kind == ChangeKind::Untracked));

        stage(&root, &["a.txt".to_string()]).await.unwrap();
        let st = status(&root).await.unwrap();
        assert_eq!(st.staged.len(), 1);
        assert_eq!(st.unstaged.len(), 1);
        let staged_diff = file_diff(&root, &st.staged[0], true).await.unwrap();
        assert!(
            parse_diff(&staged_diff)
                .iter()
                .any(|l| l.kind == LineKind::Add && l.text == "one")
        );

        unstage(&root, &["a.txt".to_string()]).await.unwrap();
        assert!(status(&root).await.unwrap().staged.is_empty());

        stage_everything(&root).await.unwrap();
        commit(&root, "first").await.unwrap();
        assert_eq!(status(&root).await.unwrap().total(), 0);

        fs::write(dir.join("a.txt"), "two\n").unwrap();
        fs::write(dir.join("fresh.txt"), "new\n").unwrap();
        let st = status(&root).await.unwrap();
        assert_eq!(st.unstaged.len(), 2);
        for c in &st.unstaged {
            discard(&root, c).await.unwrap();
        }
        // autocrlf may restore CRLF on Windows.
        assert_eq!(
            fs::read_to_string(dir.join("a.txt")).unwrap().trim_end(),
            "one"
        );
        assert!(!dir.join("fresh.txt").exists());
        assert_eq!(status(&root).await.unwrap().total(), 0);

        // `sh` fixtures commit with a throwaway identity; make sure the helper still agrees.
        sh(&dir, &["status"]).await;
        let _ = fs::remove_dir_all(&dir);
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
        fs::write(
            main.join("a.txt"),
            "one
",
        )
        .unwrap();
        sh(&main, &["add", "-A"]).await;
        sh(&main, &["commit", "-m", "init"]).await;
        let wt = Worktree {
            path: root.join("wt"),
            branch: "agentory/x".into(),
        };
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
        fs::write(
            wt.path.join("a.txt"),
            "two
",
        )
        .unwrap();
        fs::write(
            wt.path.join("new.txt"),
            "fresh
",
        )
        .unwrap();
        let base = merge_base(&wt.path, "user").await.unwrap();
        let diff = diff_since(&wt.path, &base).await.unwrap();
        assert!(diff.contains("+two") && diff.contains("+fresh"), "{diff}");
        cleanup(&main);
    }

    #[tokio::test]
    async fn clean_merge_then_fast_forward_moves_user() {
        let (main, wt) = repo("ff").await;
        fs::write(
            main.join("b.txt"),
            "other
",
        )
        .unwrap();
        sh(&main, &["add", "-A"]).await;
        sh(&main, &["commit", "-m", "user moved"]).await;
        fs::write(
            wt.path.join("a.txt"),
            "agent
",
        )
        .unwrap();
        assert!(commit_all(&wt.path, "change").await.unwrap());
        assert_eq!(merge_branch(&wt.path, "user").await.unwrap(), Merged::Clean);
        fast_forward(&main, &wt.branch).await.unwrap();
        assert_eq!(
            head_sha(&main).await.unwrap(),
            head_sha(&wt.path).await.unwrap()
        );
        assert_eq!(
            fs::read_to_string(main.join("a.txt")).unwrap().trim(),
            "agent"
        );
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
        fs::write(
            main.join("a.txt"),
            "user side
",
        )
        .unwrap();
        sh(&main, &["commit", "-am", "user edit"]).await;
        let before = head_sha(&main).await.unwrap();
        fs::write(
            wt.path.join("a.txt"),
            "agent side
",
        )
        .unwrap();
        commit_all(&wt.path, "agent edit").await.unwrap();
        let merged = merge_branch(&wt.path, "user").await.unwrap();
        assert_eq!(merged, Merged::Conflict(vec!["a.txt".into()]));
        // Resolving by hand and committing completes the merge.
        fs::write(
            wt.path.join("a.txt"),
            "both
",
        )
        .unwrap();
        assert!(commit_all(&wt.path, "resolve").await.unwrap());
        assert!(conflicted_files(&wt.path).await.unwrap().is_empty());
        assert_eq!(head_sha(&main).await.unwrap(), before);
        cleanup(&main);
    }

    #[tokio::test]
    async fn fast_forward_fails_when_user_moved_after_the_merge() {
        let (main, wt) = repo("stale").await;
        fs::write(
            wt.path.join("a.txt"),
            "agent
",
        )
        .unwrap();
        commit_all(&wt.path, "change").await.unwrap();
        fs::write(
            main.join("b.txt"),
            "late
",
        )
        .unwrap();
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
        fs::write(
            wt.path.join("a.txt"),
            "dirty
",
        )
        .unwrap();
        remove_worktree(&main, &wt).await.unwrap();
        assert!(!wt.path.exists());
        let branches = capture("git", &["branch", "--list", &wt.branch], &main)
            .await
            .unwrap();
        assert!(branches.trim().is_empty());
        let list = capture("git", &["worktree", "list"], &main).await.unwrap();
        assert_eq!(list.lines().count(), 1, "{list}");
        // Doing it again is harmless.
        remove_worktree(&main, &wt).await.unwrap();
        cleanup(&main);
    }
}
