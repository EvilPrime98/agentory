use std::path::PathBuf;

use serde_json::{Value, json};

use super::workflow::{project, take_value};
use super::{SettingError, err, pretty};
use crate::backend::tasks::{self, Failure, Task};

type Res<T> = Result<T, SettingError>;

fn usage(message: &str) -> SettingError {
    err("usage", format!("{message}. See `agentory tasks --help`"))
}

fn fail(f: Failure, id: &str) -> SettingError {
    match f {
        Failure::NotFound => err("not_found", format!("no task with id `{id}`")),
        Failure::Taken(by) => err("taken", format!("task `{id}` is already claimed by `{by}`")),
        Failure::NotOwner => err("not_owner", format!("task `{id}` is not claimed by you")),
        Failure::Closed => err("closed", format!("task `{id}` is already done")),
        Failure::Io(e) => err("io", format!("could not access the task board: {e}")),
    }
}

fn root(explicit: Option<&str>) -> Res<PathBuf> {
    if explicit.is_none() {
        if let Some(dir) = std::env::var_os(crate::state::sessions::PROJECT_ENV) {
            let dir = PathBuf::from(dir);
            if dir.is_dir() {
                return Ok(dir);
            }
        }
    }
    project(explicit)
}

fn who() -> String {
    std::env::var("AGENTORY_SESSION").unwrap_or_else(|_| "user".into())
}

fn reject_flags(cmd: &str, args: &[&str]) -> Res<()> {
    match args.iter().find(|a| a.starts_with("--")) {
        Some(flag) => Err(usage(&format!("unknown flag `{flag}` for `{cmd}`"))),
        None => Ok(()),
    }
}

fn task_json(t: &Task) -> Value {
    json!({
        "id": t.id, "title": t.title, "body": t.body, "status": t.status.name(),
        "assignee": t.assignee, "summary": t.summary,
    })
}

fn line(t: &Task) -> String {
    let who = t
        .assignee
        .as_deref()
        .map_or(String::new(), |a| format!(" ({a})"));
    format!("{}  [{}]{who}  {}", t.id, t.status.name(), t.title)
}

fn show_one(t: &Task, json: bool) -> String {
    if json {
        return pretty(&task_json(t));
    }
    let mut out = line(t);
    if !t.body.is_empty() {
        out.push_str(&format!("\n\n{}", t.body));
    }
    if !t.summary.is_empty() {
        out.push_str(&format!("\n\nResult: {}", t.summary));
    }
    out
}

pub fn dispatch(args: &mut Vec<&str>, json: bool) -> Res<String> {
    let project_arg = take_value(args, "--project")?;
    let Some((&cmd, rest)) = args.split_first() else {
        return Err(usage("missing command"));
    };
    let mut rest = rest.to_vec();
    let root = root(project_arg.as_deref())?;
    match cmd {
        "list" => {
            let status = take_value(&mut rest, "--status")?;
            reject_flags("list", &rest)?;
            if !rest.is_empty() {
                return Err(usage("`list` takes no arguments"));
            }
            if let Some(s) = &status {
                if !["open", "claimed", "done"].contains(&s.as_str()) {
                    return Err(err(
                        "invalid",
                        format!("status must be open, claimed or done, got `{s}`"),
                    ));
                }
            }
            let all: Vec<Task> = tasks::load(&root)
                .into_iter()
                .filter(|t| status.as_deref().is_none_or(|s| t.status.name() == s))
                .collect();
            Ok(if json {
                pretty(&Value::Array(all.iter().map(task_json).collect()))
            } else {
                all.iter().map(line).collect::<Vec<_>>().join("\n")
            })
        }
        "show" => {
            reject_flags("show", &rest)?;
            let [id] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `show`"));
            };
            let all = tasks::load(&root);
            let task = all
                .iter()
                .find(|t| t.id == *id)
                .ok_or_else(|| fail(Failure::NotFound, id))?;
            Ok(show_one(task, json))
        }
        "post" => {
            let body = take_value(&mut rest, "--body")?.unwrap_or_default();
            reject_flags("post", &rest)?;
            let [title] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `post`"));
            };
            if title.trim().is_empty() {
                return Err(err("invalid", "the task title cannot be empty".into()));
            }
            let task = tasks::post(&root, title.trim(), body.trim()).map_err(|f| fail(f, ""))?;
            Ok(if json {
                pretty(&task_json(&task))
            } else {
                format!("posted {}", task.id)
            })
        }
        "claim" => {
            reject_flags("claim", &rest)?;
            let id = match rest.as_slice() {
                [] => None,
                [id] => Some(*id),
                _ => return Err(usage("wrong number of arguments for `claim`")),
            };
            match tasks::claim(&root, id, &who()).map_err(|f| fail(f, id.unwrap_or("")))? {
                Some(task) => Ok(show_one(&task, json)),
                None => Ok(if json {
                    "null".into()
                } else {
                    "no open tasks".into()
                }),
            }
        }
        "close" => {
            let summary = take_value(&mut rest, "--summary")?.unwrap_or_default();
            reject_flags("close", &rest)?;
            let [id] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `close`"));
            };
            let task = tasks::close(&root, id, &who(), summary.trim()).map_err(|f| fail(f, id))?;
            Ok(if json {
                pretty(&task_json(&task))
            } else {
                format!("closed {}", task.id)
            })
        }
        "release" => {
            reject_flags("release", &rest)?;
            let [id] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `release`"));
            };
            let task = tasks::release(&root, id, &who()).map_err(|f| fail(f, id))?;
            Ok(if json {
                pretty(&task_json(&task))
            } else {
                format!("released {}", task.id)
            })
        }
        other => Err(usage(&format!("unknown command `{other}`"))),
    }
}

pub fn help_text() -> String {
    "Read and change the task board of a project, stored in .agentory/tasks.json. The project is \
     $AGENTORY_PROJECT (set in Agentory sessions), else the nearest directory with .agentory; \
     pass --project <path> to override. Claiming and closing are done as $AGENTORY_SESSION. The \
     running app reloads changes.\n\n\
     USAGE:\n  \
     agentory tasks list [--status open|claimed|done]\n  \
     agentory tasks show ID\n  \
     agentory tasks post TITLE [--body TEXT]\n  \
     agentory tasks claim [ID]                        the oldest open task when ID is omitted\n  \
     agentory tasks close ID [--summary TEXT]         only the task's assignee can close it\n  \
     agentory tasks release ID                        put a claimed task back on the board\n\n\
     FLAGS:\n  \
     --project PATH   project directory\n  \
     --json           JSON output (the default when stdout is not a terminal)\n\n\
     Errors go to stderr with a non-zero exit code (1 file error, 2 bad usage or state, including \
     not_found, taken, not_owner and closed)."
        .to_string()
}
