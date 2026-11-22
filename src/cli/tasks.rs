use std::path::PathBuf;

use serde_json::{Value, json};

use super::workflow::{project, take_value};
use super::{SettingError, err, pretty};
use crate::backend::tasks::{self, DraftEdit, Failure, NewTask, Task};

type Res<T> = Result<T, SettingError>;

fn usage(message: &str) -> SettingError {
    err("usage", format!("{message}. See `agentory tasks --help`"))
}

fn fail(f: Failure, id: &str) -> SettingError {
    match f {
        Failure::NotFound => err("not_found", format!("no task with id `{id}`")),
        Failure::Taken(by) => err("taken", format!("task `{id}` is already claimed by `{by}`")),
        Failure::NotOwner => err("not_owner", format!("task `{id}` is not claimed by you")),
        Failure::Closed => err(
            "closed",
            format!("task `{id}` is already done or waiting for review"),
        ),
        Failure::Blocked => err(
            "blocked",
            format!("task `{id}` waits for tasks it depends on"),
        ),
        Failure::NotInReview => err("not_in_review", format!("task `{id}` is not in review")),
        Failure::Invalid(why) => err("invalid", why),
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

/// A comma separated list of task ids; empty entries are ignored.
fn ids(raw: Option<String>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .collect()
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
        "assignee": t.assignee, "summary": t.summary, "write": t.write,
        "dependsOn": t.depends_on, "acceptance": t.acceptance,
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
    if !t.depends_on.is_empty() {
        out.push_str(&format!("\n\nDepends on: {}", t.depends_on.join(", ")));
    }
    if !t.acceptance.is_empty() {
        out.push_str(&format!("\n\nAcceptance criteria: {}", t.acceptance));
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
                if !["open", "claimed", "review", "done", "draft"].contains(&s.as_str()) {
                    return Err(err(
                        "invalid",
                        format!("status must be open, claimed, review, done or draft, got `{s}`"),
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
            let acceptance = take_value(&mut rest, "--accept")?.unwrap_or_default();
            let depends_on = ids(take_value(&mut rest, "--depends-on")?);
            let write = match rest.iter().position(|a| *a == "--write") {
                Some(at) => {
                    rest.remove(at);
                    true
                }
                None => false,
            };
            let draft = match rest.iter().position(|a| *a == "--draft") {
                Some(at) => {
                    rest.remove(at);
                    true
                }
                None => false,
            };
            reject_flags("post", &rest)?;
            let [title] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `post`"));
            };
            if title.trim().is_empty() {
                return Err(err("invalid", "the task title cannot be empty".into()));
            }
            let task = tasks::post(
                &root,
                NewTask {
                    title: title.trim(),
                    body: body.trim(),
                    write,
                    draft,
                    depends_on,
                    acceptance: acceptance.trim(),
                },
            )
            .map_err(|f| fail(f, ""))?;
            Ok(if json {
                pretty(&task_json(&task))
            } else {
                format!("posted {}", task.id)
            })
        }
        "edit" => {
            let change = DraftEdit {
                title: take_value(&mut rest, "--title")?.map(|v| v.trim().to_string()),
                body: take_value(&mut rest, "--body")?.map(|v| v.trim().to_string()),
                depends_on: take_value(&mut rest, "--depends-on")?.map(|v| ids(Some(v))),
                acceptance: take_value(&mut rest, "--accept")?.map(|v| v.trim().to_string()),
            };
            reject_flags("edit", &rest)?;
            let [id] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `edit`"));
            };
            let task = tasks::edit_draft(&root, id, change).map_err(|f| fail(f, id))?;
            Ok(if json {
                pretty(&task_json(&task))
            } else {
                format!("edited {}", task.id)
            })
        }
        "drop" => {
            reject_flags("drop", &rest)?;
            let [id] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `drop`"));
            };
            tasks::drop_draft(&root, id).map_err(|f| fail(f, id))?;
            Ok(format!("dropped {id}"))
        }
        "headcount" => {
            reject_flags("headcount", &rest)?;
            let [n] = rest.as_slice() else {
                return Err(usage("wrong number of arguments for `headcount`"));
            };
            let n: u32 = n.parse().ok().filter(|n| *n >= 1).ok_or_else(|| {
                err(
                    "invalid",
                    "the headcount must be a number of at least 1".into(),
                )
            })?;
            tasks::propose_headcount(&root, n).map_err(|f| fail(f, ""))?;
            Ok(format!("proposed {n} workers"))
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
                format!("{} is waiting for review", task.id)
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
     agentory tasks list [--status open|claimed|review|done|draft]\n  \
     agentory tasks show ID\n  \
     agentory tasks post TITLE [--body TEXT] [--accept CRITERIA] [--depends-on ID,ID] [--write] [--draft]\n  \
     agentory tasks edit ID [--title T] [--body T] [--accept T] [--depends-on ID,ID]   change a draft\n  \
     agentory tasks drop ID                           delete a draft\n  \
     agentory tasks headcount N                       propose how many workers the plan needs\n  \
     agentory tasks claim [ID]                        the oldest open task when ID is omitted\n  \
     agentory tasks close ID [--summary TEXT]         hand the result to the user for review; only the assignee\n  \
     agentory tasks release ID                        put a claimed task back on the board\n\n\
     FLAGS:\n  \
     --project PATH   project directory\n  \
     --json           JSON output (the default when stdout is not a terminal)\n\n\
     Errors go to stderr with a non-zero exit code (1 file error, 2 bad usage or state, including \
     not_found, taken, not_owner, closed, blocked and invalid)."
        .to_string()
}
