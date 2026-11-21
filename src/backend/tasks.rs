use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    #[default]
    Open,
    Claimed,
    Done,
    /// Proposed by the foreman and waiting for approval; the dispatcher ignores it.
    Draft,
}

impl TaskStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claimed => "claimed",
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
    pub created_ms: u64,
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

pub fn post(
    project: &Path,
    title: &str,
    body: &str,
    write: bool,
    draft: bool,
) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let mut n = tasks.len() + 1;
        while tasks.iter().any(|t| t.id == format!("t{n}")) {
            n += 1;
        }
        let task = Task {
            id: format!("t{n}"),
            title: title.to_string(),
            body: body.to_string(),
            write,
            status: if draft {
                TaskStatus::Draft
            } else {
                TaskStatus::Open
            },
            created_ms: now_ms(),
            ..Task::default()
        };
        tasks.push(task.clone());
        Ok(task)
    })
}

/// Claims `id`, or the oldest open task when `id` is `None`. `Ok(None)` means nothing is open.
pub fn claim(project: &Path, id: Option<&str>, who: &str) -> Result<Option<Task>, Failure> {
    edit(project, |tasks| {
        let slot = match id {
            Some(id) => {
                let slot = tasks
                    .iter()
                    .position(|t| t.id == id)
                    .ok_or(Failure::NotFound)?;
                match (&tasks[slot].status, &tasks[slot].assignee) {
                    (TaskStatus::Open, _) => slot,
                    (TaskStatus::Claimed, Some(a)) if a == who => slot,
                    (_, a) => return Err(Failure::Taken(a.clone().unwrap_or_default())),
                }
            }
            None => match tasks.iter().position(|t| t.status == TaskStatus::Open) {
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

pub fn close(project: &Path, id: &str, who: &str, summary: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let task = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if task.status == TaskStatus::Done {
            return Err(Failure::Closed);
        }
        if task.assignee.as_deref() != Some(who) {
            return Err(Failure::NotOwner);
        }
        task.status = TaskStatus::Done;
        task.summary = summary.to_string();
        Ok(task.clone())
    })
}

/// Puts a claimed task back on the board, for a worker that cannot finish it.
pub fn release(project: &Path, id: &str, who: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let task = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(Failure::NotFound)?;
        if task.status == TaskStatus::Done {
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

/// Forgets the previous plan, for a new foreman.
pub fn clear_plan(project: &Path) -> Result<(), Failure> {
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
        write_plan(project, &Plan::default())?;
        Ok(before - tasks.len())
    })
}

/// How many workers the dispatcher may run: the approved limit while the plan has unfinished
/// tasks, never above the cap.
pub fn worker_limit(cap: usize, plan: &Plan, tasks: &[Task]) -> usize {
    let live = tasks
        .iter()
        .any(|t| matches!(t.status, TaskStatus::Open | TaskStatus::Claimed));
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
        post(&d, "a", "", false, false).unwrap();
        post(&d, "b", "", false, false).unwrap();
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
        post(&d, "a", "", false, false).unwrap();
        claim(&d, Some("t1"), "s1").unwrap();
        assert_eq!(close(&d, "t1", "s2", "x"), Err(Failure::NotOwner));
        let done = close(&d, "t1", "s1", "shipped").unwrap();
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(close(&d, "t1", "s1", "x"), Err(Failure::Closed));
    }

    #[test]
    fn release_reopens_a_claimed_task() {
        let d = dir("release");
        post(&d, "a", "", false, false).unwrap();
        claim(&d, None, "s1").unwrap();
        release(&d, "t1", "s1").unwrap();
        assert_eq!(load(&d)[0].status, TaskStatus::Open);
        assert_eq!(claim(&d, None, "s2").unwrap().unwrap().id, "t1");
    }

    #[test]
    fn drafts_wait_for_approval_and_approval_sets_the_limit() {
        let d = dir("draft");
        post(&d, "a", "", false, true).unwrap();
        post(&d, "b", "", true, true).unwrap();
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
        post(&d, "keep", "", false, false).unwrap();
        post(&d, "x", "", false, true).unwrap();
        assert_eq!(discard(&d).unwrap(), 1);
        assert_eq!(load(&d).len(), 1);
        assert_eq!(discard(&d).unwrap(), 0);
    }

    #[test]
    fn headcount_defaults_to_the_draft_count_within_the_cap() {
        let d = dir("headcount");
        for i in 0..6 {
            post(&d, &i.to_string(), "", false, true).unwrap();
        }
        let tasks = load(&d);
        assert_eq!(proposed_headcount(4, &Plan::default(), &tasks), 4);
        let plan = Plan {
            headcount: Some(9),
            limit: None,
        };
        assert_eq!(proposed_headcount(4, &plan, &tasks), 4);
        let plan = Plan {
            headcount: Some(2),
            limit: None,
        };
        assert_eq!(proposed_headcount(4, &plan, &tasks), 2);
    }
}
