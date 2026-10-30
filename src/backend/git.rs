//! Git worktrees for isolated sessions.

use std::path::{Path, PathBuf};

use super::capture;

/// A worktree created for a session.
#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    #[allow(dead_code)] // kept for the worktree cleanup that is not built yet
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_filesystem_safe() {
        assert_eq!(sanitize("my proj/é"), "my_proj_é");
        assert_eq!(branch_name("s1"), "agentory/s1");
    }
}
