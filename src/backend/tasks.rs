use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    #[default]
    Open,
    Claimed,
    /// The worker finished and the user has not accepted the result yet.
    Review,
    Done,
    /// Proposed by the foreman and waiting for approval; the dispatcher ignores it.
    Draft,
}

impl TaskStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claimed => "claimed",
            Self::Review => "review",
            Self::Done => "done",
            Self::Draft => "draft",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub summary: String,
    /// The task changes files, so its worker gets a worktree. Read-only tasks run in the project.
    #[serde(default)]
    pub write: bool,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// What the result must satisfy for the user to accept it.
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub created_ms: u64,
}

impl Task {
    /// Whether every task this one depends on is done. A dependency that is gone blocks nothing.
    pub fn ready(&self, board: &[Task]) -> bool {
        self.depends_on.iter().all(|d| {
            board
                .iter()
                .find(|t| t.id == *d)
                .is_none_or(|t| t.status == TaskStatus::Done)
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    tasks: Vec<Task>,
}

pub fn file_path(project: &Path) -> PathBuf {
    project.join(".agentory").join("tasks.json")
}

pub fn load(project: &Path) -> Vec<Task> {
    std::fs::read_to_string(file_path(project))
        .ok()
        .and_then(|s| serde_json::from_str::<File>(&s).ok())
        .map(|f| f.tasks)
        .unwrap_or_default()
}

fn write(project: &Path, tasks: Vec<Task>) -> std::io::Result<()> {
    let path = file_path(project);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&File { tasks }).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn lock(project: &Path) -> std::io::Result<Lock> {
    let path = project.join(".agentory").join("tasks.lock");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    for _ in 0..100 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(Lock(path)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(10));
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("the task board is locked"))
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    NotFound,
    Taken(String),
    NotOwner,
    Closed,
    Blocked,
    NotInReview,
    Invalid(String),
    Io(String),
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Reads the board, applies `f` and writes it back, all under the board lock.
fn edit<T>(
    project: &Path,
    f: impl FnOnce(&mut Vec<Task>) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let _guard = lock(project)?;
    let mut tasks = load(project);
    let out = f(&mut tasks)?;
    write(project, tasks)?;
    Ok(out)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Default)]
pub struct NewTask<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub write: bool,
    pub draft: bool,
    pub depends_on: Vec<String>,
    pub acceptance: &'a str,
}

fn check_deps(tasks: &[Task], id: &str, deps: &[String]) -> Result<(), Failure> {
    for d in deps {
        if d == id {
            return Err(Failure::Invalid(format!(
                "task `{id}` cannot depend on itself"
            )));
        }
        if !tasks.iter().any(|t| t.id == *d) {
            return Err(Failure::Invalid(format!(
                "no task with id `{d}` to depend on"
            )));
        }
        if reaches(tasks, d, id) {
            return Err(Failure::Invalid(format!(
                "`{id}` depending on `{d}` would make a cycle"
            )));
        }
    }
    Ok(())
}

/// Whether `from` depends, directly or not, on `target`.
fn reaches(tasks: &[Task], from: &str, target: &str) -> bool {
    let mut seen: Vec<String> = Vec::new();
    let mut stack = vec![from.to_string()];
    while let Some(id) = stack.pop() {
        if id == target {
            return true;
        }
        if seen.contains(&id) {
            continue;
        }
        if let Some(t) = tasks.iter().find(|t| t.id == id) {
            stack.extend(t.depends_on.iter().cloned());
        }
        seen.push(id);
    }
    false
}

/// A change to a draft makes the proposal incomplete: the foreman proposes the headcount again.
fn unpropose(project: &Path) -> Result<(), Failure> {
    let plan = load_plan(project);
    if plan.headcount.is_some() {
        write_plan(
            project,
            &Plan {
                headcount: None,
                ..plan
            },
        )?;
    }
    Ok(())
}

pub fn post(project: &Path, new: NewTask) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let mut n = tasks.len() + 1;
        while tasks.iter().any(|t| t.id == format!("t{n}")) {
            n += 1;
        }
        let id = format!("t{n}");
        check_deps(tasks, &id, &new.depends_on)?;
        let task = Task {
            id,
            title: new.title.to_string(),
            body: new.body.to_string(),
            write: new.write,
            status: if new.draft {
                TaskStatus::Draft
            } else {
                TaskStatus::Open
            },
            depends_on: new.depends_on,
            acceptance: new.acceptance.to_string(),
            created_ms: now_ms(),
            ..Task::default()
        };
        tasks.push(task.clone());
        if new.draft {
            unpropose(project)?;
        }
        Ok(task)
    })
}

/// The fields of a draft that `edit_draft` can change; `None` leaves a field as it is.
#[derive(Debug, Default)]
pub struct DraftEdit {
    pub title: Option<String>,
    pub body: Option<String>,
    pub depends_on: Option<Vec<String>>,
    pub acceptance: Option<String>,
}

pub fn edit_draft(project: &Path, id: &str, change: DraftEdit) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let slot = tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if tasks[slot].status != TaskStatus::Draft {
            return Err(Failure::Invalid(format!("task `{id}` is not a draft")));
        }
        if let Some(deps) = &change.depends_on {
            check_deps(tasks, id, deps)?;
        }
        let task = &mut tasks[slot];
        if let Some(v) = change.title {
            task.title = v;
        }
        if let Some(v) = change.body {
            task.body = v;
        }
        if let Some(v) = change.depends_on {
            task.depends_on = v;
        }
        if let Some(v) = change.acceptance {
            task.acceptance = v;
        }
        let task = task.clone();
        unpropose(project)?;
        Ok(task)
    })
}

/// Deletes a draft and removes it from the dependencies of the others.
pub fn drop_draft(project: &Path, id: &str) -> Result<(), Failure> {
    edit(project, |tasks| {
        let slot = tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if tasks[slot].status != TaskStatus::Draft {
            return Err(Failure::Invalid(format!("task `{id}` is not a draft")));
        }
        tasks.remove(slot);
        for t in tasks.iter_mut() {
            t.depends_on.retain(|d| d != id);
        }
        unpropose(project)
    })
}

/// Claims `id`, or the oldest open task that is not blocked when `id` is `None`. `Ok(None)` means nothing is open.
pub fn claim(project: &Path, id: Option<&str>, who: &str) -> Result<Option<Task>, Failure> {
    edit(project, |tasks| {
        let slot = match id {
            Some(id) => {
                let slot = tasks
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or(Failure::NotFound)?;
                match (&tasks[slot].status, &tasks[slot].assignee) {
                    (TaskStatus::Open, _) if !tasks[slot].ready(tasks) => {
                        return Err(Failure::Blocked);
                    }
                    (TaskStatus::Open, _) => slot,
                    (TaskStatus::Claimed, Some(a)) if a == who => slot,
                    (_, a) => return Err(Failure::Taken(a.clone().unwrap_or_default())),
                }
            }
            None => match tasks
                .iter()
                .position(|t| t.status == TaskStatus::Open && t.ready(tasks))
            {
                Some(slot) => slot,
                None => return Ok(None),
            },
        };
        let task = &mut tasks[slot];
        task.status = TaskStatus::Claimed;
        task.assignee = Some(who.to_string());
        Ok(Some(task.clone()))
    })
}

/// The worker is finished: the task waits for the user to accept it or send it back.
pub fn close(project: &Path, id: &str, who: &str, summary: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let task = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if matches!(task.status, TaskStatus::Done | TaskStatus::Review) {
            return Err(Failure::Closed);
        }
        if task.assignee.as_deref() != Some(who) {
            return Err(Failure::NotOwner);
        }
        task.status = TaskStatus::Review;
        task.summary = summary.to_string();
        Ok(task.clone())
    })
}

fn reviewed(tasks: &[Task], id: &str) -> Result<usize, Failure> {
    let slot = tasks
        .iter()
        .position(|t| t.id == id)
        .ok_or(Failure::NotFound)?;
    if tasks[slot].status != TaskStatus::Review {
        return Err(Failure::NotInReview);
    }
    Ok(slot)
}

/// The user accepted the result: the task is done and its work counts as part of the plan.
pub fn accept(project: &Path, id: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let slot = reviewed(tasks, id)?;
        tasks[slot].status = TaskStatus::Done;
        let mut plan = load_plan(project);
        if tasks[slot].write && !plan.merged.iter().any(|m| m == id) {
            plan.merged.push(id.to_string());
            write_plan(project, &plan)?;
        }
        Ok(tasks[slot].clone())
    })
}

/// The user wants more work: the task goes back to its worker, who keeps it.
pub fn send_back(project: &Path, id: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let slot = reviewed(tasks, id)?;
        tasks[slot].status = TaskStatus::Claimed;
        Ok(tasks[slot].clone())
    })
}

/// Puts a claimed task back on the board, for a worker that cannot finish it.
pub fn release(project: &Path, id: &str, who: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let task = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if matches!(task.status, TaskStatus::Done | TaskStatus::Review) {
            return Err(Failure::Closed);
        }
        if task.assignee.as_deref() != Some(who) {
            return Err(Failure::NotOwner);
        }
        task.status = TaskStatus::Open;
        task.assignee = None;
        Ok(task.clone())
    })
}

/// State of the foreman's plan, kept beside the board in `.agentory/plan.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Workers the foreman proposes. Set last, so it also marks the draft as complete.
    #[serde(default)]
    pub headcount: Option<u32>,
    /// Workers the user approved; caps the dispatcher while the plan has unfinished tasks.
    #[serde(default)]
    pub limit: Option<u32>,
    /// The integration branch of the plan: write tasks start from it and accepted work merges
    /// into it. Named when the first write task starts.
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub merged: Vec<String>,
}

fn plan_path(project: &Path) -> PathBuf {
    project.join(".agentory").join("plan.json")
}

pub fn load_plan(project: &Path) -> Plan {
    std::fs::read_to_string(plan_path(project))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_plan(project: &Path, plan: &Plan) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(plan).map_err(std::io::Error::other)?;
    std::fs::write(plan_path(project), json)
}

/// Records the foreman's proposed headcount.
pub fn propose_headcount(project: &Path, n: u32) -> Result<(), Failure> {
    edit(project, |_| {
        let plan = Plan {
            headcount: Some(n),
            ..load_plan(project)
        };
        write_plan(project, &plan)?;
        Ok(())
    })
}

/// Forgets the previous proposal, for a new foreman. The plan branch and its accepted tasks stay
/// until the plan is published.
pub fn clear_plan(project: &Path) -> Result<(), Failure> {
    edit(project, |_| {
        Ok(write_plan(project, &kept(load_plan(project)))?)
    })
}

fn kept(plan: Plan) -> Plan {
    Plan {
        branch: plan.branch,
        merged: plan.merged,
        ..Plan::default()
    }
}

pub fn ensure_branch(project: &Path) -> Result<String, Failure> {
    edit(project, |_| {
        let plan = load_plan(project);
        if let Some(b) = &plan.branch {
            return Ok(b.clone());
        }
        let branch = format!("agentory/plan-{}", now_ms());
        write_plan(
            project,
            &Plan {
                branch: Some(branch.clone()),
                ..plan
            },
        )?;
        Ok(branch)
    })
}

/// Whether every task of a plan with accepted work is done, so its branch can be published.
pub fn plan_complete(plan: &Plan, tasks: &[Task]) -> bool {
    plan.branch.is_some()
        && !plan.merged.is_empty()
        && tasks.iter().all(|t| t.status == TaskStatus::Done)
}

/// Forgets a published plan, so the next tasks start a new branch.
pub fn finish_plan(project: &Path) -> Result<(), Failure> {
    edit(project, |_| Ok(write_plan(project, &Plan::default())?))
}

/// Opens every draft and stores `limit` as the plan's worker limit. Returns how many opened.
pub fn approve(project: &Path, limit: u32) -> Result<usize, Failure> {
    edit(project, |tasks| {
        let mut opened = 0;
        for t in tasks.iter_mut().filter(|t| t.status == TaskStatus::Draft) {
            t.status = TaskStatus::Open;
            opened += 1;
        }
        write_plan(
            project,
            &Plan {
                headcount: None,
                limit: Some(limit),
                ..load_plan(project)
            },
        )?;
        Ok(opened)
    })
}

/// Deletes every draft and forgets the plan. Returns how many were deleted.
pub fn discard(project: &Path) -> Result<usize, Failure> {
    edit(project, |tasks| {
        let before = tasks.len();
        tasks.retain(|t| t.status != TaskStatus::Draft);
        write_plan(project, &kept(load_plan(project)))?;
        Ok(before - tasks.len())
    })
}

/// How many workers the dispatcher may run: the approved limit while the plan has unfinished
/// tasks, never above the cap.
pub fn worker_limit(cap: usize, plan: &Plan, tasks: &[Task]) -> usize {
    let live = tasks.iter().any(|t| {
        matches!(
            t.status,
            TaskStatus::Open | TaskStatus::Claimed | TaskStatus::Review
        )
    });
    match plan.limit {
        Some(n) if live => cap.min(n as usize),
        _ => cap,
    }
}

/// The headcount to show for a draft: the proposal, else one worker per task, within the cap.
pub fn proposed_headcount(cap: u32, plan: &Plan, tasks: &[Task]) -> u32 {
    let drafts = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Draft)
        .count() as u32;
    plan.headcount.unwrap_or(drafts).clamp(1, cap.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agentory-tasks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claim_takes_the_oldest_open_task_once() {
        let d = dir("claim");
        post(
            &d,
            NewTask {
                title: "a",
                write: false,
                draft: false,
                ..NewTask::default()
            },
        )
        .unwrap();
        post(
            &d,
            NewTask {
                title: "b",
                write: false,
                draft: false,
                ..NewTask::default()
            },
        )
        .unwrap();
        let first = claim(&d, None, "s1").unwrap().unwrap();
        assert_eq!(first.title, "a");
        let second = claim(&d, None, "s2").unwrap().unwrap();
        assert_eq!(second.title, "b");
        assert_eq!(claim(&d, None, "s3").unwrap(), None);
        assert_eq!(
            claim(&d, Some("t1"), "s2"),
            Err(Failure::Taken("s1".into()))
        );
    }

    #[test]
    fn only_the_assignee_closes_a_task() {
        let d = dir("close");
        post(
            &d,
            NewTask {
                title: "a",
                write: false,
                draft: false,
                ..NewTask::default()
            },
        )
        .unwrap();
        claim(&d, Some("t1"), "s1").unwrap();
        assert_eq!(close(&d, "t1", "s2", "x"), Err(Failure::NotOwner));
        let parked = close(&d, "t1", "s1", "shipped").unwrap();
        assert_eq!(parked.status, TaskStatus::Review);
        assert_eq!(close(&d, "t1", "s1", "x"), Err(Failure::Closed));
        assert_eq!(accept(&d, "t1").unwrap().status, TaskStatus::Done);
        assert_eq!(accept(&d, "t1"), Err(Failure::NotInReview));
    }

    #[test]
    fn sending_a_task_back_returns_it_to_its_worker() {
        let d = dir("sendback");
        post(
            &d,
            NewTask {
                title: "a",
                ..NewTask::default()
            },
        )
        .unwrap();
        claim(&d, Some("t1"), "s1").unwrap();
        assert_eq!(send_back(&d, "t1"), Err(Failure::NotInReview));
        close(&d, "t1", "s1", "first try").unwrap();
        let back = send_back(&d, "t1").unwrap();
        assert_eq!(
            (back.status, back.assignee.as_deref()),
            (TaskStatus::Claimed, Some("s1"))
        );
        close(&d, "t1", "s1", "second try").unwrap();
        assert_eq!(load(&d)[0].summary, "second try");
    }

    #[test]
    fn a_task_waits_for_its_dependencies_to_be_accepted() {
        let d = dir("deps");
        post(
            &d,
            NewTask {
                title: "a",
                ..NewTask::default()
            },
        )
        .unwrap();
        post(
            &d,
            NewTask {
                title: "b",
                depends_on: vec!["t1".into()],
                ..NewTask::default()
            },
        )
        .unwrap();
        assert_eq!(claim(&d, Some("t2"), "s2"), Err(Failure::Blocked));
        assert_eq!(claim(&d, None, "s1").unwrap().unwrap().id, "t1");
        assert_eq!(claim(&d, None, "s2").unwrap(), None);
        close(&d, "t1", "s1", "x").unwrap();
        assert_eq!(
            claim(&d, None, "s2").unwrap(),
            None,
            "in review is not done"
        );
        accept(&d, "t1").unwrap();
        assert_eq!(claim(&d, None, "s2").unwrap().unwrap().id, "t2");
    }

    #[test]
    fn dependencies_must_exist_and_cannot_cycle() {
        let d = dir("cycle");
        let draft = |deps: Vec<String>| NewTask {
            title: "x",
            draft: true,
            depends_on: deps,
            ..NewTask::default()
        };
        post(&d, draft(vec![])).unwrap();
        post(&d, draft(vec!["t1".into()])).unwrap();
        assert!(matches!(
            post(&d, draft(vec!["t9".into()])),
            Err(Failure::Invalid(_))
        ));
        let cyc = DraftEdit {
            depends_on: Some(vec!["t2".into()]),
            ..DraftEdit::default()
        };
        assert!(matches!(
            edit_draft(&d, "t1", cyc),
            Err(Failure::Invalid(_))
        ));
        drop_draft(&d, "t1").unwrap();
        assert!(load(&d)[0].depends_on.is_empty());
    }

    #[test]
    fn changing_a_draft_withdraws_the_proposal() {
        let d = dir("unpropose");
        post(
            &d,
            NewTask {
                title: "a",
                draft: true,
                ..NewTask::default()
            },
        )
        .unwrap();
        propose_headcount(&d, 1).unwrap();
        let change = DraftEdit {
            title: Some("b".into()),
            ..DraftEdit::default()
        };
        edit_draft(&d, "t1", change).unwrap();
        assert_eq!(load_plan(&d).headcount, None);
    }

    #[test]
    fn a_plan_is_complete_once_accepted_work_is_all_done() {
        let d = dir("complete");
        post(
            &d,
            NewTask {
                title: "a",
                write: true,
                ..NewTask::default()
            },
        )
        .unwrap();
        let branch = ensure_branch(&d).unwrap();
        assert_eq!(ensure_branch(&d).unwrap(), branch);
        claim(&d, None, "s1").unwrap();
        close(&d, "t1", "s1", "x").unwrap();
        assert!(!plan_complete(&load_plan(&d), &load(&d)));
        accept(&d, "t1").unwrap();
        assert!(plan_complete(&load_plan(&d), &load(&d)));
        finish_plan(&d).unwrap();
        assert!(!plan_complete(&load_plan(&d), &load(&d)));
    }

    #[test]
    fn release_reopens_a_claimed_task() {
        let d = dir("release");
        post(
            &d,
            NewTask {
                title: "a",
                write: false,
                draft: false,
                ..NewTask::default()
            },
        )
        .unwrap();
        claim(&d, None, "s1").unwrap();
        release(&d, "t1", "s1").unwrap();
        assert_eq!(load(&d)[0].status, TaskStatus::Open);
        assert_eq!(claim(&d, None, "s2").unwrap().unwrap().id, "t1");
    }

    #[test]
    fn drafts_wait_for_approval_and_approval_sets_the_limit() {
        let d = dir("draft");
        post(
            &d,
            NewTask {
                title: "a",
                write: false,
                draft: true,
                ..NewTask::default()
            },
        )
        .unwrap();
        post(
            &d,
            NewTask {
                title: "b",
                write: true,
                draft: true,
                ..NewTask::default()
            },
        )
        .unwrap();
        assert_eq!(claim(&d, None, "s1").unwrap(), None);
        propose_headcount(&d, 2).unwrap();
        assert_eq!(approve(&d, 1).unwrap(), 2);
        let plan = load_plan(&d);
        assert_eq!(plan.limit, Some(1));
        assert_eq!(plan.headcount, None);
        assert_eq!(worker_limit(4, &plan, &load(&d)), 1);
        assert_eq!(claim(&d, None, "s1").unwrap().unwrap().title, "a");
    }

    #[test]
    fn discard_deletes_only_drafts() {
        let d = dir("discard");
        post(
            &d,
            NewTask {
                title: "keep",
                write: false,
                draft: false,
                ..NewTask::default()
            },
        )
        .unwrap();
        post(
            &d,
            NewTask {
                title: "x",
                write: false,
                draft: true,
                ..NewTask::default()
            },
        )
        .unwrap();
        assert_eq!(discard(&d).unwrap(), 1);
        assert_eq!(load(&d).len(), 1);
        assert_eq!(discard(&d).unwrap(), 0);
    }

    #[test]
    fn headcount_defaults_to_the_draft_count_within_the_cap() {
        let d = dir("headcount");
        for i in 0..6 {
            post(
                &d,
                NewTask {
                    title: &i.to_string(),
                    write: false,
                    draft: true,
                    ..NewTask::default()
                },
            )
            .unwrap();
        }
        let tasks = load(&d);
        assert_eq!(proposed_headcount(4, &Plan::default(), &tasks), 4);
        let plan = Plan {
            headcount: Some(9),
            ..Plan::default()
        };
        assert_eq!(proposed_headcount(4, &plan, &tasks), 4);
        let plan = Plan {
            headcount: Some(2),
            ..Plan::default()
        };
        assert_eq!(proposed_headcount(4, &plan, &tasks), 2);
    }
}
