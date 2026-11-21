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
}

impl TaskStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claimed => "claimed",
            Self::Done => "done",
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

pub fn post(project: &Path, title: &str, body: &str) -> Result<Task, Failure> {
    edit(project, |tasks| {
        let mut n = tasks.len() + 1;
        while tasks.iter().any(|t| t.id == format!("t{n}")) {
            n += 1;
        }
        let task = Task {
            id: format!("t{n}"),
            title: title.to_string(),
            body: body.to_string(),
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
        post(&d, "a", "").unwrap();
        post(&d, "b", "").unwrap();
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
        post(&d, "a", "").unwrap();
        claim(&d, Some("t1"), "s1").unwrap();
        assert_eq!(close(&d, "t1", "s2", "x"), Err(Failure::NotOwner));
        let done = close(&d, "t1", "s1", "shipped").unwrap();
        assert_eq!(done.status, TaskStatus::Done);
        assert_eq!(close(&d, "t1", "s1", "x"), Err(Failure::Closed));
    }

    #[test]
    fn release_reopens_a_claimed_task() {
        let d = dir("release");
        post(&d, "a", "").unwrap();
        claim(&d, None, "s1").unwrap();
        release(&d, "t1", "s1").unwrap();
        assert_eq!(load(&d)[0].status, TaskStatus::Open);
        assert_eq!(claim(&d, None, "s2").unwrap().unwrap().id, "t1");
    }
}
