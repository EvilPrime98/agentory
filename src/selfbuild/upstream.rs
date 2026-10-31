//! Upstream updates: find release tags newer than the user's base and merge one into a worktree
//! branched from `user`. The merge result then goes through the normal review/apply flow.
//! Tags are treated as immutable: a fetch never force-updates one, so a rewritten tag fails loudly.

use std::cmp::Ordering;
use std::path::Path;

use super::clone::USER_BRANCH;
use crate::backend::git::{self, Merged, Worktree};

/// `vMAJOR.MINOR.PATCH` as numbers. Anything else (pre-releases, other tags) is not a release.
pub fn parse_tag(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

pub fn compare_tags(a: &str, b: &str) -> Ordering {
    parse_tag(a).cmp(&parse_tag(b))
}

/// The newest release tag in `tags` that is above `base`.
pub fn newest_after(tags: &[String], base: &str) -> Option<String> {
    let base = parse_tag(base)?;
    tags.iter()
        .filter(|t| parse_tag(t).is_some_and(|v| v > base))
        .max_by(|a, b| compare_tags(a, b))
        .cloned()
}

/// What the settings page shows about the user's build versus upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// The release `user` is based on.
    pub base: String,
    /// Commits on `user` that upstream does not have.
    pub local_changes: usize,
    /// A newer release, if there is one.
    pub available: Option<String>,
}

/// Fetch new tags (never overwriting existing ones) and compare them with the user's base.
pub async fn check(clone: &Path) -> Result<Status, String> {
    git::fetch_tags(clone).await?;
    let base = git::base_tag(clone, USER_BRANCH).await?;
    let tags = git::list_tags(clone).await?;
    let local_changes = git::count_commits(clone, &base, USER_BRANCH).await?;
    Ok(Status {
        available: newest_after(&tags, &base),
        base,
        local_changes,
    })
}

/// Merge `tag` into the worktree branch. A conflict is left in progress in the worktree.
pub async fn merge_tag(wt: &Worktree, tag: &str) -> Result<Merged, String> {
    git::merge_branch(&wt.path, tag).await
}

/// What the agent is told when merging the upstream release conflicts.
pub fn conflict_prompt(tag: &str, files: &[String]) -> String {
    format!(
        "Merging the official release {tag} into this worktree stopped with conflicts in: {}. \
         Resolve every conflict and remove all conflict markers, keeping both the release's changes and \
         the user's own customizations. Do not weaken or delete tests. Run cargo build and cargo test, \
         commit, then tell me it is ready to apply.",
        files.join(", ")
    )
}

/// Shown when the merge was clean, so the user knows what the session is for.
pub fn clean_prompt(tag: &str) -> String {
    format!(
        "The official release {tag} was merged into this worktree without conflicts. Review the change and apply it from the review page."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_release_tags_parse() {
        assert_eq!(parse_tag("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_tag("v1.2"), None);
        assert_eq!(parse_tag("v1.2.3-rc1"), None);
        assert_eq!(parse_tag("1.2.3"), None);
        assert_eq!(parse_tag("v1.2.3.4"), None);
    }

    #[test]
    fn newest_tag_is_numeric_not_lexical() {
        let tags: Vec<String> = ["v0.9.0", "v0.10.0", "v0.1.0", "nightly"]
            .map(String::from)
            .into();
        assert_eq!(newest_after(&tags, "v0.1.0").as_deref(), Some("v0.10.0"));
        assert_eq!(newest_after(&tags, "v0.10.0"), None);
        assert_eq!(newest_after(&tags, "weird"), None);
    }

    use crate::backend::sh;
    use std::fs;
    use std::path::PathBuf;

    async fn commit(dir: &Path, file: &str, text: &str) {
        fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", "-A"]).await;
        sh(dir, &["commit", "-m", "c"]).await;
    }

    /// An "origin" tagged v0.1.0 and a clone with a `user` branch holding one local commit.
    async fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("agentory-up-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let origin = root.join("origin");
        fs::create_dir_all(&origin).unwrap();
        sh(&origin, &["init", "-b", "main"]).await;
        commit(&origin, "a.txt", "one\n").await;
        sh(&origin, &["tag", "v0.1.0"]).await;
        let clone = root.join("clone");
        let (o, c) = (
            origin.to_string_lossy().into_owned(),
            clone.to_string_lossy().into_owned(),
        );
        sh(&root, &["clone", "--branch", "v0.1.0", &o, &c]).await;
        sh(&clone, &["checkout", "-b", USER_BRANCH]).await;
        commit(&clone, "mine.txt", "mine\n").await;
        (root, origin, clone)
    }

    #[tokio::test]
    async fn check_reports_base_local_changes_and_new_release() {
        let (root, origin, clone) = setup("check").await;
        let s = check(&clone).await.unwrap();
        assert_eq!(
            s,
            Status {
                base: "v0.1.0".into(),
                local_changes: 1,
                available: None
            }
        );

        commit(&origin, "b.txt", "two\n").await;
        sh(&origin, &["tag", "v0.2.0"]).await;
        let s = check(&clone).await.unwrap();
        assert_eq!(s.available.as_deref(), Some("v0.2.0"));
        assert_eq!(s.base, "v0.1.0");
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn clean_merge_moves_the_base_and_keeps_local_changes() {
        let (root, origin, clone) = setup("merge").await;
        commit(&origin, "b.txt", "two\n").await;
        sh(&origin, &["tag", "v0.2.0"]).await;
        check(&clone).await.unwrap();

        let wt = git::add_worktree_from(
            &clone,
            "T",
            &format!("u{}", std::process::id()),
            USER_BRANCH,
        )
        .await
        .unwrap();
        assert_eq!(merge_tag(&wt, "v0.2.0").await.unwrap(), Merged::Clean);
        git::fast_forward(&clone, &wt.branch).await.unwrap();
        let s = check(&clone).await.unwrap();
        assert_eq!((s.base.as_str(), s.available), ("v0.2.0", None));
        assert!(clone.join("mine.txt").exists() && clone.join("b.txt").exists());
        let _ = git::remove_worktree(&clone, &wt).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn conflicting_release_is_left_in_progress() {
        let (root, origin, clone) = setup("conflict").await;
        commit(&clone, "a.txt", "mine\n").await;
        commit(&origin, "a.txt", "theirs\n").await;
        sh(&origin, &["tag", "v0.2.0"]).await;
        check(&clone).await.unwrap();

        let wt = git::add_worktree_from(
            &clone,
            "T",
            &format!("c{}", std::process::id()),
            USER_BRANCH,
        )
        .await
        .unwrap();
        assert_eq!(
            merge_tag(&wt, "v0.2.0").await.unwrap(),
            Merged::Conflict(vec!["a.txt".into()])
        );
        let _ = git::remove_worktree(&clone, &wt).await;
        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_moved_tag_is_not_accepted() {
        let (root, origin, clone) = setup("moved").await;
        commit(&origin, "b.txt", "two\n").await;
        sh(&origin, &["tag", "-f", "v0.1.0"]).await;
        let err = check(&clone).await;
        // Either the fetch refuses, or the old tag stays: the merge base never silently changes.
        let tags = git::list_tags(&clone).await.unwrap();
        assert!(err.is_err() || tags == ["v0.1.0"]);
        let base = git::base_tag(&clone, USER_BRANCH).await.unwrap();
        assert_eq!(base, "v0.1.0");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn prompts_name_the_release_and_files() {
        let p = conflict_prompt("v0.2.0", &["a.rs".into(), "b.rs".into()]);
        assert!(p.contains("v0.2.0") && p.contains("a.rs, b.rs") && !p.contains('\r'));
        assert!(clean_prompt("v0.2.0").contains("v0.2.0"));
    }
}
