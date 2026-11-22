//! Turns open tasks on a project's board into worker sessions, up to the worker cap.
//!
//! A worker handles one task: it is claimed for the session before the session starts and a write
//! task runs in its own worktree, branched off the plan branch. A task starts once the tasks it
//! depends on are accepted. When the worker closes its task it parks in review, session and
//! worktree alive, until the user accepts it (`crate::state::plan`) or sends it back. Once a task
//! is accepted the session ends and the worktree folder is removed. When every task of the plan
//! is accepted, the plan branch is published as one PR.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{App, AppContext as _, AsyncApp, Context};

use crate::backend::git::{self, Worktree};
use crate::backend::{spm, tasks};
use crate::i18n::t_args;
use crate::model::{Project, SessionStatus, new_id};
use crate::runtime;
use crate::state::sessions::SessionKind;
use crate::state::{Stores, plan};
use crate::ui::toast;

/// Session ids of foremen start with this, which tells them apart from workers.
pub const FOREMAN_PREFIX: &str = "foreman-";

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
    /// Projects whose plan is being published, or failed to be; not retried until the app restarts.
    publishing: HashSet<String>,
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

pub fn worker_prompt(task: &tasks::Task, board: &[tasks::Task]) -> String {
    let commit = if task.write {
        " Commit your changes on the current branch before closing the task."
    } else {
        " This task is read-only: do not change files."
    };
    let mut extra = String::new();
    if !task.acceptance.is_empty() {
        extra.push_str(&format!(
            " Acceptance criteria: {}.",
            plan::one_line(&task.acceptance)
        ));
    }
    for dep in task
        .depends_on
        .iter()
        .filter_map(|d| board.iter().find(|t| t.id == *d))
    {
        if !dep.summary.is_empty() {
            extra.push_str(&format!(
                " Result of {} ({}), which this task builds on: {}.",
                dep.id,
                plan::one_line(&dep.title),
                plan::one_line(&dep.summary)
            ));
        }
    }
    format!(
        "You are a worker on the project task board. Your task is {id}: {title}. Run \
         `agentory tasks show {id}` for the details and do the work.{commit}{extra} Then run \
         `agentory tasks close {id} --summary TEXT` with a short result. The result goes to the user \
         for review and may come back with feedback: keep this session open and address it. Do not \
         claim other tasks.\r",
        id = task.id,
        title = task.title,
    )
}

/// The startup prompt of a foreman. `input` is a markdown file with the issue or document to plan.
pub fn foreman_prompt(input: &str) -> String {
    format!(
        "You are the foreman of the project task board and you plan with the user in this chat. First read {input} and explore the code it touches. Ask the user clarifying questions whenever the scope, constraints or priorities are unclear, then propose the plan in the chat (the tasks, which depend on which, which change files) and wait for the user's agreement before posting anything. Then post each task with `agentory tasks post TITLE --body TEXT --accept CRITERIA --depends-on ID,ID --draft`: add --write when the task changes files (its work is merged into one plan branch, and one PR into dev is opened when the plan is done); leave it out for read-only tasks. `post` prints the task's id: post prerequisites first and use their ids in --depends-on, and make tasks independent only when they really are. Size each task so one worker finishes it in one session, and give it acceptance criteria the user can check. Fix a draft with `agentory tasks edit ID` or `agentory tasks drop ID`. Do not do the work yourself. When the plan is final run `agentory tasks headcount N` with how many workers should run in parallel, never more than the number of tasks, as the last step: the user then reviews and approves the plan. If the user asks for changes after that, edit the drafts and run headcount again.\r"
    )
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
        Self::reap_foremen(stores, &projects, cx);
        let cap = stores.prefs.read(cx).prefs.max_workers as usize;
        let boards: Vec<(Project, Vec<tasks::Task>)> = projects
            .into_iter()
            .map(|p| {
                let board = tasks::load(&p.path);
                (p, board)
            })
            .collect();
        let mut total: usize = boards.iter().map(|(p, b)| self.working_in(p, b)).sum();
        for (project, board) in &boards {
            let plan = tasks::load_plan(&project.path);
            self.publish_plan(project, &plan, board, cx);
            let limit = tasks::worker_limit(cap, &plan, board);
            let mut mine = self.working_in(project, board);
            for task in board {
                if total >= cap || mine >= limit {
                    break;
                }
                let startable = task.status == tasks::TaskStatus::Open
                    && task.ready(board)
                    && !self.failed.contains(&task.id);
                if startable {
                    self.start(stores, project, task.clone(), board, cx);
                    total += 1;
                    mine += 1;
                }
            }
        }
    }

    /// Workers of `project` that are working now. A worker parked in review is not: it keeps its
    /// session but does not take one of the slots.
    fn working_in(&self, project: &Project, board: &[tasks::Task]) -> usize {
        self.workers
            .values()
            .filter(|w| w.project.id == project.id)
            .filter(|w| {
                board
                    .iter()
                    .any(|t| t.id == w.task && t.status == tasks::TaskStatus::Claimed)
            })
            .count()
    }

    /// Opens the plan's PR into `dev` once all its tasks are accepted.
    fn publish_plan(
        &mut self,
        project: &Project,
        plan: &tasks::Plan,
        board: &[tasks::Task],
        cx: &mut Context<Self>,
    ) {
        let Some(branch) = plan.branch.clone() else {
            return;
        };
        if !tasks::plan_complete(plan, board) || self.publishing.contains(&project.id) {
            return;
        }
        self.publishing.insert(project.id.clone());
        let done: Vec<tasks::Task> = plan
            .merged
            .iter()
            .filter_map(|id| board.iter().find(|t| t.id == *id).cloned())
            .collect();
        let project = project.clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let repo = project.path.clone();
            let result =
                runtime::run(async move { plan::publish(&repo, &branch, &done).await }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    let _ = tasks::finish_plan(&project.path);
                    this.publishing.remove(&project.id);
                }
                Err(e) => toast::error(cx, t_args("office.pr.failed", &[("error", &e)])),
            });
        })
        .detach();
    }

    /// Closes the foremen whose session died, so the desk is free again. A live foreman stays: the
    /// user talks to it while planning and the office closes it when the plan is approved or
    /// rejected.
    fn reap_foremen(stores: &Stores, projects: &[Project], cx: &mut Context<Self>) {
        for project in projects {
            let done: Vec<String> = stores
                .sessions
                .read(cx)
                .agents(&project.id)
                .filter(|s| s.id.starts_with(FOREMAN_PREFIX))
                .filter(|s| s.status == SessionStatus::Exited)
                .map(|s| s.id.clone())
                .collect();
            for sid in done {
                stores.sessions.update(cx, |s, cx| s.close(&sid, cx));
            }
        }
    }

    /// Ends the workers whose task is accepted and frees the ones whose session died. A worker
    /// whose task waits for review is left alone: the user may send the task back to it.
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
            let accepted = state == Some(tasks::TaskStatus::Done);
            let done = accepted || state.is_none();
            if state == Some(tasks::TaskStatus::Review) || (!done && alive) {
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
                cx.spawn(async move |_, _: &mut AsyncApp| {
                    runtime::run(async move {
                        if let Err(e) = git::remove_worktree_keep_branch(&repo, &wt).await {
                            tracing::warn!("could not remove the worktree of a closed task: {e}");
                        }
                        // An accepted task is part of the plan branch; any other branch keeps
                        // work that was not merged.
                        if accepted {
                            if let Err(e) = git::delete_local_branch(&repo, &wt.branch).await {
                                tracing::warn!("could not delete the branch of a merged task: {e}");
                            }
                        }
                    })
                    .await;
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
        board: &[tasks::Task],
        cx: &mut Context<Self>,
    ) {
        let write = task.write && project.is_git_repo();
        let plan_branch = if write {
            match tasks::ensure_branch(&project.path) {
                Ok(branch) => Some(branch),
                Err(e) => {
                    tracing::error!("could not name the plan branch: {e:?}");
                    return;
                }
            }
        } else {
            None
        };
        let sid = new_id("w");
        match tasks::claim(&project.path, Some(&task.id), &sid) {
            Ok(Some(_)) => {}
            _ => return,
        }
        self.workers.insert(
            sid.clone(),
            Worker {
                task: task.id.clone(),
                project: project.clone(),
                started: !write,
                worktree: None,
            },
        );
        let prompt = worker_prompt(&task, board);
        if !write {
            Self::spawn(stores, project, sid, None, prompt, cx);
            return;
        }
        // The worktree is made off the UI thread; `reap` ignores the worker until it is started.
        let (stores, project, task_id) = (stores.clone(), project.clone(), task.id.clone());
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let (p, s) = (project.clone(), sid.clone());
            let result = runtime::run(async move {
                let wt = match &plan_branch {
                    Some(base) => {
                        git::ensure_branch(&p.path, base).await?;
                        git::add_worktree_from(&p.path, &p.name, &s, base).await?
                    }
                    None => git::add_worktree(&p.path, &p.name, &s).await?,
                };
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
    fn foreman_prompt_names_the_input_and_submits() {
        let p = foreman_prompt("C:/tmp/plan.md");
        assert!(p.contains("C:/tmp/plan.md") && p.contains("--draft") && p.contains("headcount"));
        assert!(p.contains("--depends-on") && p.contains("--accept"));
        assert!(p.ends_with('\r') && !p.contains('\n'));
    }

    #[test]
    fn prompt_names_the_task_and_submits() {
        let task = tasks::Task {
            id: "t3".into(),
            title: "Fix it".into(),
            write: true,
            ..Default::default()
        };
        let p = worker_prompt(&task, &[]);
        assert!(p.contains("t3") && p.contains("Fix it") && p.contains("Commit"));
        assert!(p.ends_with('\r') && !p.contains('\n'));
        let ro = worker_prompt(
            &tasks::Task {
                id: "t4".into(),
                ..Default::default()
            },
            &[],
        );
        assert!(ro.contains("read-only"));
    }

    #[test]
    fn prompt_carries_the_criteria_and_what_the_dependencies_found() {
        let dep = tasks::Task {
            id: "t1".into(),
            title: "Survey".into(),
            summary: "three callers".into(),
            ..Default::default()
        };
        let task = tasks::Task {
            id: "t2".into(),
            acceptance: "tests pass".into(),
            depends_on: vec!["t1".into()],
            ..Default::default()
        };
        let p = worker_prompt(&task, &[dep]);
        assert!(p.contains("tests pass") && p.contains("three callers") && p.contains("t1"));
    }
}
