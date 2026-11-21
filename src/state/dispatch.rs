//! Turns open tasks on a project's board into worker sessions, up to the worker cap.
//!
//! A worker handles one task: it is claimed for the session before the session starts, a write
//! task runs in its own worktree, and when the task is closed the session ends and the worktree
//! folder is removed (the branch stays, so the work is not lost).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use gpui::{App, AppContext as _, AsyncApp, Context};

use crate::backend::git::{self, Worktree};
use crate::backend::{gh, spm, tasks};
use crate::i18n::t_args;
use crate::model::{Project, SessionStatus, new_id};
use crate::runtime;
use crate::state::Stores;
use crate::state::sessions::SessionKind;
use crate::ui::toast;

const TICK: Duration = Duration::from_secs(2);

struct Worker {
    task: String,
    project: Project,
    /// Set once the session exists; until then the worker is being prepared.
    started: bool,
    worktree: Option<Worktree>,
}

#[derive(Default)]
struct Dispatcher {
    workers: HashMap<String, Worker>,
    /// Tasks whose worker failed to start; they are not retried until the app restarts.
    failed: HashSet<String>,
}

pub fn watch(stores: Stores, cx: &mut App) {
    let dispatcher = cx.new(|_| Dispatcher::default());
    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(TICK).await;
            let stores = stores.clone();
            cx.update(|cx| dispatcher.update(cx, |d, cx| d.tick(&stores, cx)));
        }
    })
    .detach();
}

pub fn worker_prompt(task: &tasks::Task) -> String {
    let commit = if task.write {
        " Commit your changes on the current branch before closing the task."
    } else {
        " This task is read-only: do not change files."
    };
    format!(
        "You are a worker on the project task board. Your task is {id}: {title}. Run \
         `agentory tasks show {id}` for the details and do the work.{commit} Then run \
         `agentory tasks close {id} --summary TEXT` with a short result. Do not claim other tasks.\r",
        id = task.id,
        title = task.title,
    )
}

/// Pushes the task's branch and opens a PR into `dev`. The local branch is deleted only once the
/// PR exists; on any failure it stays, so the work is not lost.
async fn publish_pr(repo: &Path, branch: &str, task: &tasks::Task) -> Result<(), String> {
    if gh::setup(repo).await != gh::Setup::Ready {
        return Err(format!("{}: GitHub is not set up", task.id));
    }
    if !git::remote_has_branch(repo, "dev").await {
        return Err(format!("{}: the remote has no dev branch", task.id));
    }
    git::push_branch(repo, branch)
        .await
        .map_err(|e| format!("{}: {e}", task.id))?;
    let body = format!("Task {}\n\n{}", task.id, task.summary);
    gh::pr_create_from(repo, branch, "dev", &task.title, &body)
        .await
        .map_err(|e| format!("{}: {e}", task.id))?;
    git::delete_local_branch(repo, branch).await
}

impl Dispatcher {
    fn tick(&mut self, stores: &Stores, cx: &mut Context<Self>) {
        let projects: Vec<Project> = stores
            .projects
            .read(cx)
            .items
            .iter()
            .filter(|p| !p.is_global())
            .cloned()
            .collect();
        self.reap(stores, &projects, cx);
        let cap = stores.prefs.read(cx).prefs.max_workers as usize;
        for project in projects {
            if self.workers.len() >= cap {
                break;
            }
            for task in tasks::load(&project.path) {
                if self.workers.len() >= cap {
                    break;
                }
                if task.status == tasks::TaskStatus::Open && !self.failed.contains(&task.id) {
                    self.start(stores, &project, task, cx);
                }
            }
        }
    }

    /// Ends the workers whose task is closed and frees the ones whose session died.
    fn reap(&mut self, stores: &Stores, projects: &[Project], cx: &mut Context<Self>) {
        let ids: Vec<String> = self.workers.keys().cloned().collect();
        for sid in ids {
            let Some(w) = self.workers.get(&sid) else {
                continue;
            };
            if !w.started {
                continue;
            }
            let task = tasks::load(&w.project.path)
                .into_iter()
                .find(|t| t.id == w.task);
            let state = task.as_ref().map(|t| t.status);
            let alive = stores
                .sessions
                .read(cx)
                .get(&sid)
                .is_some_and(|s| s.status != SessionStatus::Exited);
            let done = state == Some(tasks::TaskStatus::Done) || state.is_none();
            if !done && alive {
                continue;
            }
            let w = self.workers.remove(&sid).expect("checked above");
            if !done {
                // The session ended without closing its task: put it back, but not in a loop.
                let _ = tasks::release(&w.project.path, &w.task, &sid);
                self.failed.insert(w.task.clone());
            }
            stores.sessions.update(cx, |s, cx| s.close(&sid, cx));
            if let Some(wt) = w.worktree {
                let repo = w.project.path.clone();
                // Only finished work becomes a PR; a failed worker's branch is left as it is.
                let publish = done.then_some(task).flatten();
                cx.spawn(async move |_, cx: &mut AsyncApp| {
                    let outcome = runtime::run(async move {
                        if let Err(e) = git::remove_worktree_keep_branch(&repo, &wt).await {
                            tracing::warn!("could not remove the worktree of a closed task: {e}");
                        }
                        match publish {
                            Some(task) => Some(publish_pr(&repo, &wt.branch, &task).await),
                            None => None,
                        }
                    })
                    .await;
                    if let Some(Err(e)) = outcome {
                        cx.update(|cx| {
                            toast::error(cx, t_args("office.pr.failed", &[("error", &e)]))
                        });
                    }
                })
                .detach();
            }
        }
        // A project removed from the app takes its workers with it.
        let gone: Vec<String> = self
            .workers
            .iter()
            .filter(|(_, w)| !projects.iter().any(|p| p.id == w.project.id))
            .map(|(sid, _)| sid.clone())
            .collect();
        for sid in gone {
            self.workers.remove(&sid);
        }
    }

    fn start(
        &mut self,
        stores: &Stores,
        project: &Project,
        task: tasks::Task,
        cx: &mut Context<Self>,
    ) {
        let sid = new_id("w");
        match tasks::claim(&project.path, Some(&task.id), &sid) {
            Ok(Some(_)) => {}
            _ => return,
        }
        let write = task.write && project.is_git_repo();
        self.workers.insert(
            sid.clone(),
            Worker {
                task: task.id.clone(),
                project: project.clone(),
                started: !write,
                worktree: None,
            },
        );
        let prompt = worker_prompt(&task);
        if !write {
            Self::spawn(stores, project, sid, None, prompt, cx);
            return;
        }
        // The worktree is made off the UI thread; `reap` ignores the worker until it is started.
        let (stores, project, task_id) = (stores.clone(), project.clone(), task.id.clone());
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let (p, s) = (project.clone(), sid.clone());
            let result = runtime::run(async move {
                let wt = git::add_worktree(&p.path, &p.name, &s).await?;
                if p.path.join("spm.json").exists() {
                    if let Err(e) = spm::install(&wt.path, &[]).await {
                        tracing::warn!("spm install in the worktree failed: {e}");
                    }
                }
                Ok::<_, String>(wt)
            })
            .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(wt) => {
                    if let Some(w) = this.workers.get_mut(&sid) {
                        w.worktree = Some(wt.clone());
                        w.started = true;
                    }
                    Self::spawn(&stores, &project, sid, Some(wt), prompt, cx);
                }
                Err(e) => {
                    let _ = tasks::release(&project.path, &task_id, &sid);
                    this.workers.remove(&sid);
                    this.failed.insert(task_id);
                    toast::error(cx, t_args("sessions.worktree.failed", &[("error", &e)]));
                }
            });
        })
        .detach();
    }

    fn spawn(
        stores: &Stores,
        project: &Project,
        sid: String,
        worktree: Option<Worktree>,
        prompt: String,
        cx: &mut App,
    ) {
        let prefs = stores.prefs.read(cx).prefs.clone();
        let result = stores.sessions.update(cx, |s, cx| {
            s.start(
                project,
                sid.clone(),
                worktree,
                SessionKind::Agent,
                &prefs,
                Some(prompt),
                cx,
            )
        });
        if let Err(e) = result {
            tracing::error!("could not start a worker: {e}");
            toast::error(cx, e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_names_the_task_and_submits() {
        let task = tasks::Task {
            id: "t3".into(),
            title: "Fix it".into(),
            write: true,
            ..Default::default()
        };
        let p = worker_prompt(&task);
        assert!(p.contains("t3") && p.contains("Fix it") && p.contains("Commit"));
        assert!(p.ends_with('\r'));
        let ro = worker_prompt(&tasks::Task {
            id: "t4".into(),
            ..Default::default()
        });
        assert!(ro.contains("read-only"));
    }
}
