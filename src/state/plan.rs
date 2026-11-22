//! Reviewing the tasks of a plan. A finished task waits in review; accepting it merges its branch
//! into the plan branch (so tasks that depend on it start from its work), sending it back resumes
//! its worker with the user's feedback.

use gpui::{App, AsyncApp};

use crate::backend::git::{self, Merged, Worktree};
use crate::backend::{gh, tasks};
use crate::i18n::t_args;
use crate::model::Project;
use crate::runtime;
use crate::state::Stores;
use crate::ui::toast;

/// What the worker is told when the user sends its task back.
pub fn feedback_prompt(task: &tasks::Task, feedback: &str) -> String {
    format!(
        "The reviewer sent task {id} back with this feedback: {feedback} Address it, commit your \
         changes on the current branch, then run `agentory tasks close {id} --summary TEXT` again.",
        id = task.id,
        feedback = one_line(feedback),
    )
}

/// What the worker is told when merging the plan branch into its branch conflicts.
pub fn conflict_prompt(task: &tasks::Task, plan_branch: &str, files: &[String]) -> String {
    format!(
        "Task {id} cannot be accepted yet: merging the plan branch {plan_branch} into this branch \
         stopped with conflicts in: {files}. Resolve every conflict and remove all conflict \
         markers, keeping the intent of both sides, commit, then run \
         `agentory tasks close {id} --summary TEXT` again.",
        id = task.id,
        files = files.join(", "),
    )
}

pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The worktree a write task's worker uses; it is named after the worker's session.
pub fn worktree_of(project: &Project, task: &tasks::Task) -> Option<Worktree> {
    let sid = task.assignee.as_deref()?;
    (task.write && project.is_git_repo()).then(|| Worktree {
        path: git::worktree_path(&project.name, sid),
        branch: git::branch_name(sid),
    })
}

/// Merges the plan branch into the task's branch and, when that is clean, moves the plan branch
/// up to it. The plan branch is not checked out anywhere, so it is moved with a compare and swap.
async fn integrate(
    repo: &std::path::Path,
    wt: &Worktree,
    plan_branch: &str,
    title: &str,
) -> Result<Merged, String> {
    let unresolved = git::unresolved_files(&wt.path).await?;
    if !unresolved.is_empty() {
        return Ok(Merged::Conflict(unresolved));
    }
    git::commit_all(&wt.path, title).await?;
    let before = git::rev_sha(repo, &format!("refs/heads/{plan_branch}")).await?;
    if let Merged::Conflict(files) = git::merge_branch(&wt.path, plan_branch).await? {
        return Ok(Merged::Conflict(files));
    }
    let tip = git::rev_sha(&wt.path, "HEAD").await?;
    git::advance_branch(repo, plan_branch, &tip, &before)
        .await
        .map_err(|e| format!("the plan branch moved, accept again. {e}"))?;
    Ok(Merged::Clean)
}

/// Accepts a task in review. A write task is merged into the plan branch first; when that
/// conflicts, its worker is asked to resolve the conflicts and the task goes back to claimed.
pub fn accept(stores: &Stores, project: &Project, task: tasks::Task, cx: &mut App) {
    let stores = stores.clone();
    let project = project.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let plan = tasks::load_plan(&project.path);
        let merge = match (worktree_of(&project, &task), plan.branch) {
            (Some(wt), Some(branch)) => {
                let (repo, title) = (project.path.clone(), task.title.clone());
                let result = runtime::run(async move {
                    integrate(&repo, &wt, &branch, &title)
                        .await
                        .map(|m| (m, branch))
                })
                .await;
                Some(result)
            }
            _ => None,
        };
        cx.update(|cx| match merge {
            Some(Err(e)) => toast::error(cx, t_args("office.review.failed", &[("error", &e)])),
            Some(Ok((Merged::Conflict(files), branch))) => {
                if tasks::send_back(&project.path, &task.id).is_err() {
                    return;
                }
                let (sid, prompt) = (
                    task.assignee.clone().unwrap_or_default(),
                    conflict_prompt(&task, &branch, &files),
                );
                stores
                    .sessions
                    .update(cx, |s, cx| s.submit(&sid, &prompt, cx));
                toast::error(cx, t_args("office.review.conflict", &[("task", &task.id)]));
            }
            Some(Ok((Merged::Clean, _))) | None => {
                if let Err(e) = tasks::accept(&project.path, &task.id) {
                    toast::error(
                        cx,
                        t_args("office.review.failed", &[("error", &format!("{e:?}"))]),
                    );
                }
            }
        });
    })
    .detach();
}

/// Sends a task in review back to its worker with the user's feedback.
pub fn send_back(
    stores: &Stores,
    project: &Project,
    task: &tasks::Task,
    feedback: &str,
    cx: &mut App,
) {
    if let Err(e) = tasks::send_back(&project.path, &task.id) {
        toast::error(
            cx,
            t_args("office.review.failed", &[("error", &format!("{e:?}"))]),
        );
        return;
    }
    let sid = task.assignee.clone().unwrap_or_default();
    let prompt = feedback_prompt(task, feedback);
    stores
        .sessions
        .update(cx, |s, cx| s.submit(&sid, &prompt, cx));
}

/// What a task changed, for the review: its branch against where it left the plan branch.
pub async fn diff(project: &Project, task: &tasks::Task) -> Result<String, String> {
    let Some(wt) = worktree_of(project, task) else {
        return Ok(String::new());
    };
    let plan = tasks::load_plan(&project.path);
    let base = match plan.branch {
        Some(branch) => git::merge_base(&wt.path, &branch).await?,
        None => git::merge_base(&wt.path, "HEAD").await?,
    };
    git::diff_since(&wt.path, &base).await
}

/// Pushes the plan branch and opens one PR into `dev` with everything accepted. The local branch
/// is dropped once the PR exists; the work is safe on the remote then.
pub async fn publish(
    repo: &std::path::Path,
    branch: &str,
    done: &[tasks::Task],
) -> Result<(), String> {
    if gh::setup(repo).await != gh::Setup::Ready {
        return Err("GitHub is not set up".into());
    }
    if !git::remote_has_branch(repo, "dev").await {
        return Err("the remote has no dev branch".into());
    }
    git::push_branch(repo, branch).await?;
    let (title, body) = pr_text(done);
    gh::pr_create_from(repo, branch, "dev", &title, &body).await?;
    if let Err(e) = git::delete_local_branch(repo, branch).await {
        tracing::warn!("could not delete the published plan branch {branch}: {e}");
    }
    Ok(())
}

fn pr_text(done: &[tasks::Task]) -> (String, String) {
    let title = match done {
        [] => "Agentory plan".to_string(),
        [one] => one.title.clone(),
        [first, rest @ ..] => format!("{} (+{} more tasks)", first.title, rest.len()),
    };
    let body = done
        .iter()
        .map(|t| format!("- {} {}: {}", t.id, t.title, t.summary))
        .collect::<Vec<_>>()
        .join("\n");
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, title: &str) -> tasks::Task {
        tasks::Task {
            id: id.into(),
            title: title.into(),
            summary: "did it".into(),
            ..Default::default()
        }
    }

    #[test]
    fn prompts_name_the_task_and_never_submit_themselves() {
        let t = task("t2", "Fix");
        let p = feedback_prompt(&t, "use a map\nnot a vec");
        assert!(p.contains("t2") && p.contains("use a map not a vec") && !p.contains('\n'));
        let c = conflict_prompt(&t, "agentory/plan-1", &["a.rs".into(), "b.rs".into()]);
        assert!(c.contains("agentory/plan-1") && c.contains("a.rs, b.rs") && !c.contains('\r'));
    }

    #[test]
    fn a_pr_lists_every_accepted_task() {
        let (title, body) = pr_text(&[task("t1", "One"), task("t2", "Two")]);
        assert_eq!(title, "One (+1 more tasks)");
        assert!(body.contains("- t1 One: did it") && body.contains("- t2 Two: did it"));
        assert_eq!(pr_text(&[task("t1", "One")]).0, "One");
    }

    fn sh(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn worktree(root: &std::path::Path, name: &str) -> Worktree {
        let path = root.join(name);
        sh(
            root.join("repo").as_path(),
            &[
                "worktree",
                "add",
                path.to_str().unwrap(),
                "-b",
                name,
                "plan",
            ],
        );
        Worktree {
            path,
            branch: name.into(),
        }
    }

    #[tokio::test]
    async fn accepting_merges_into_the_plan_branch_and_stops_on_conflicts() {
        let root = std::env::temp_dir().join(format!("agentory-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-b", "dev"]);
        sh(&repo, &["config", "user.email", "t@example.com"]);
        sh(&repo, &["config", "user.name", "t"]);
        std::fs::write(
            repo.join("a.txt"),
            "one
",
        )
        .unwrap();
        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-m", "init"]);
        sh(&repo, &["branch", "plan"]);
        let (w1, w2) = (worktree(&root, "w1"), worktree(&root, "w2"));

        std::fs::write(
            w1.path.join("a.txt"),
            "first
",
        )
        .unwrap();
        let merged = integrate(&repo, &w1, "plan", "t1").await.unwrap();
        assert_eq!(merged, Merged::Clean);
        assert_eq!(
            sh(&repo, &["rev-parse", "plan"]),
            sh(&w1.path, &["rev-parse", "HEAD"])
        );

        std::fs::write(
            w2.path.join("a.txt"),
            "second
",
        )
        .unwrap();
        let blocked = integrate(&repo, &w2, "plan", "t2").await.unwrap();
        assert_eq!(blocked, Merged::Conflict(vec!["a.txt".into()]));
        let again = integrate(&repo, &w2, "plan", "t2").await.unwrap();
        assert_eq!(
            again,
            Merged::Conflict(vec!["a.txt".into()]),
            "unresolved files still block"
        );

        std::fs::write(
            w2.path.join("a.txt"),
            "both
",
        )
        .unwrap();
        assert_eq!(
            integrate(&repo, &w2, "plan", "t2").await.unwrap(),
            Merged::Clean
        );
        assert_eq!(sh(&repo, &["show", "plan:a.txt"]), "both");
        let _ = std::fs::remove_dir_all(&root);
    }
}
