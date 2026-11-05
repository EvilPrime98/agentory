use std::io::{IsTerminal as _, Read as _};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{SettingError, err, pretty, take_flag};
use crate::backend::workflow::{self, Step, Workflow};

type Res<T> = Result<T, SettingError>;

fn usage(message: &str) -> SettingError {
    err(
        "usage",
        format!("{message}. See `agentory workflow --help`"),
    )
}

fn io(e: &std::io::Error) -> SettingError {
    err("io", format!("could not access workflows: {e}"))
}

fn not_found(id: &str) -> SettingError {
    err("not_found", format!("no workflow with id `{id}`"))
}

fn invalid(message: String) -> SettingError {
    err("invalid", message)
}

fn take_values(args: &mut Vec<&str>, flag: &str) -> Res<Vec<String>> {
    let mut out = Vec::new();
    while let Some(i) = args.iter().position(|a| *a == flag) {
        let Some(value) = args.get(i + 1) else {
            return Err(usage(&format!("`{flag}` needs a value")));
        };
        out.push((*value).to_string());
        args.drain(i..=i + 1);
    }
    Ok(out)
}

fn take_value(args: &mut Vec<&str>, flag: &str) -> Res<Option<String>> {
    Ok(take_values(args, flag)?.pop())
}

fn exact<'a, const N: usize>(cmd: &str, args: &[&'a str]) -> Res<[&'a str; N]> {
    args.try_into()
        .map_err(|_| usage(&format!("wrong number of arguments for `{cmd}`")))
}

fn reject_flags(cmd: &str, args: &[&str]) -> Res<()> {
    match args.iter().find(|a| a.starts_with("--")) {
        Some(flag) => Err(usage(&format!("unknown flag `{flag}` for `{cmd}`"))),
        None => Ok(()),
    }
}

fn position(raw: &str, len: usize) -> Res<usize> {
    match raw.parse::<usize>() {
        Ok(n) if (1..=len).contains(&n) => Ok(n - 1),
        _ => Err(invalid(format!(
            "step position must be a number from 1 to {len}, got `{raw}`"
        ))),
    }
}

fn project(explicit: Option<&str>) -> Res<PathBuf> {
    if let Some(p) = explicit {
        let path = PathBuf::from(p);
        return if path.is_dir() {
            Ok(path)
        } else {
            Err(err(
                "no_project",
                format!("project path `{p}` is not a directory"),
            ))
        };
    }
    let cwd = std::env::current_dir().map_err(|e| io(&e))?;
    cwd.ancestors()
        .find(|dir| dir.join(".agentory").is_dir())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            err(
                "no_project",
                "no .agentory directory found in the current directory or its parents; \
                 run from inside a project or pass --project <path>"
                    .into(),
            )
        })
}

fn load(root: &Path, id: &str) -> Res<Workflow> {
    workflow::load_workflow(root, id).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found(id),
        _ => io(&e),
    })
}

fn save(root: &Path, wf: &Workflow) -> Res<()> {
    workflow::validate(wf).map_err(invalid)?;
    workflow::save_workflow(root, wf).map_err(|e| io(&e))
}

fn edit(root: &Path, id: &str, change: impl FnOnce(&mut Workflow) -> Res<()>) -> Res<Workflow> {
    let mut wf = load(root, id)?;
    change(&mut wf)?;
    save(root, &wf)?;
    Ok(wf)
}

fn done(wf: &Workflow, verb: &str, json: bool) -> String {
    if json {
        pretty(&serde_json::to_value(wf).unwrap_or(Value::Null))
    } else {
        format!("{verb} {}", wf.id)
    }
}

pub fn dispatch(args: &mut Vec<&str>, json: bool) -> Res<String> {
    let project_arg = take_value(args, "--project")?;
    let yes = take_flag(args, "--yes");
    let Some((&cmd, rest)) = args.split_first() else {
        return Err(usage("missing command"));
    };
    let mut rest = rest.to_vec();
    let root = || project(project_arg.as_deref());
    match cmd {
        "list" => {
            exact::<0>(cmd, &rest)?;
            list(&root()?, json)
        }
        "show" => {
            let [id] = exact(cmd, &rest)?;
            show(&load(&root()?, id)?, json)
        }
        "create" => create(&root, &mut rest, json),
        "apply" => apply(&root, &mut rest, json),
        "rename" => {
            let [id, name] = exact(cmd, &rest)?;
            let wf = edit(&root()?, id, |wf| {
                wf.name = name.trim().to_string();
                Ok(())
            })?;
            Ok(done(&wf, "renamed", json))
        }
        "delete" => {
            let [id] = exact(cmd, &rest)?;
            delete(&root()?, id, yes, json)
        }
        "step" => step(&root, &mut rest, json),
        other => Err(usage(&format!("unknown command `{other}`"))),
    }
}

fn list(root: &Path, json: bool) -> Res<String> {
    let all = workflow::load_workflows(root);
    if json {
        let items: Vec<Value> = all
            .iter()
            .map(|w| serde_json::json!({ "id": w.id, "name": w.name, "steps": w.steps.len() }))
            .collect();
        return Ok(pretty(&Value::Array(items)));
    }
    Ok(all
        .iter()
        .map(|w| format!("{}  {} ({} steps)", w.id, w.name, w.steps.len()))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn show(wf: &Workflow, json: bool) -> Res<String> {
    if json {
        return Ok(done(wf, "", true));
    }
    let mut out = format!("{} ({})", wf.name, wf.id);
    for (i, s) in wf.steps.iter().enumerate() {
        out.push_str(&format!("\n\n{}. {}", i + 1, s.name));
        if !s.model.is_empty() {
            out.push_str(&format!("  model: {}", s.model));
        }
        if !s.skills.is_empty() {
            out.push_str(&format!("  skills: {}", s.skills.join(", ")));
        }
        for line in s.prompt.lines() {
            out.push_str(&format!("\n   {line}"));
        }
    }
    Ok(out)
}

fn create(root: &dyn Fn() -> Res<PathBuf>, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let prompt = take_value(rest, "--prompt")?;
    let step_name = take_value(rest, "--step-name")?;
    let model = take_value(rest, "--model")?;
    let skills = take_values(rest, "--skill")?;
    reject_flags("create", rest)?;
    let [name] = exact("create", rest)?;
    let root = root()?;
    let wf = Workflow {
        id: workflow::unique_id(&root, name),
        name: name.trim().to_string(),
        steps: vec![Step {
            name: step_name.unwrap_or_else(|| "Step 1".into()),
            prompt: prompt.unwrap_or_default(),
            model: model.unwrap_or_default(),
            skills,
        }],
    };
    save(&root, &wf).map_err(|mut e| {
        if e.code == "invalid" && wf.steps[0].prompt.trim().is_empty() {
            e.message
                .push_str(" (pass the first step's prompt with --prompt)");
        }
        e
    })?;
    Ok(done(&wf, "created", json))
}

fn apply(root: &dyn Fn() -> Res<PathBuf>, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let file = take_value(rest, "--file")?;
    reject_flags("apply", rest)?;
    let [target] = exact("apply", rest)?;
    let root = root()?;
    let text = match file {
        Some(path) => std::fs::read_to_string(&path)
            .map_err(|e| err("io", format!("could not read `{path}`: {e}")))?,
        None if std::io::stdin().is_terminal() => {
            return Err(usage(
                "pipe the workflow JSON on stdin or pass --file <path>",
            ));
        }
        None => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| err("io", format!("could not read stdin: {e}")))?;
            buf
        }
    };
    let bad_json =
        |e: serde_json::Error| err("invalid_json", format!("invalid workflow JSON: {e}"));
    let mut value: Value = serde_json::from_str(&text).map_err(bad_json)?;
    if let Some(obj) = value.as_object_mut() {
        match (target, obj.get("id").and_then(Value::as_str)) {
            ("-", _) => {}
            (id, Some(inner)) if id != inner => {
                return Err(invalid(format!(
                    "the JSON id `{inner}` does not match `{id}`"
                )));
            }
            (id, _) => {
                obj.insert("id".into(), Value::String(id.to_string()));
            }
        }
    }
    let wf: Workflow = serde_json::from_value(value).map_err(bad_json)?;
    save(&root, &wf)?;
    Ok(done(&wf, "applied", json))
}

fn delete(root: &Path, id: &str, yes: bool, json: bool) -> Res<String> {
    load(root, id)?;
    if !yes && std::io::stdout().is_terminal() {
        return Err(err(
            "confirm_required",
            format!("refusing to delete `{id}` on a terminal without --yes"),
        ));
    }
    workflow::delete_workflow(root, id).map_err(|e| io(&e))?;
    Ok(if json {
        pretty(&serde_json::json!({ "id": id, "deleted": true }))
    } else {
        format!("deleted {id}")
    })
}

struct StepFlags {
    name: Option<String>,
    prompt: Option<String>,
    model: Option<String>,
    skills: Vec<String>,
    clear_skills: bool,
}

impl StepFlags {
    fn take(rest: &mut Vec<&str>) -> Res<Self> {
        Ok(Self {
            name: take_value(rest, "--name")?,
            prompt: take_value(rest, "--prompt")?,
            model: take_value(rest, "--model")?,
            skills: take_values(rest, "--skill")?,
            clear_skills: take_flag(rest, "--clear-skills"),
        })
    }

    fn apply(self, step: &mut Step) {
        if let Some(v) = self.name {
            step.name = v;
        }
        if let Some(v) = self.prompt {
            step.prompt = v;
        }
        if let Some(v) = self.model {
            step.model = v;
        }
        if self.clear_skills {
            step.skills.clear();
        }
        if !self.skills.is_empty() {
            step.skills = self.skills;
        }
    }
}

fn step(root: &dyn Fn() -> Res<PathBuf>, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let Some((&sub, _)) = rest.split_first() else {
        return Err(usage("missing step command (add, set, rm, move)"));
    };
    rest.remove(0);
    let label = format!("step {sub}");
    match sub {
        "add" => {
            let at = take_value(rest, "--at")?;
            let flags = StepFlags::take(rest)?;
            reject_flags(&label, rest)?;
            let [id] = exact(&label, rest)?;
            let wf = edit(&root()?, id, |wf| {
                let index = match at {
                    Some(raw) => position(&raw, wf.steps.len() + 1)?,
                    None => wf.steps.len(),
                };
                let mut new = Step::default();
                flags.apply(&mut new);
                wf.steps.insert(index, new);
                Ok(())
            })?;
            Ok(done(&wf, "updated", json))
        }
        "set" => {
            let flags = StepFlags::take(rest)?;
            reject_flags(&label, rest)?;
            let [id, n] = exact(&label, rest)?;
            let wf = edit(&root()?, id, |wf| {
                let i = position(n, wf.steps.len())?;
                flags.apply(&mut wf.steps[i]);
                Ok(())
            })?;
            Ok(done(&wf, "updated", json))
        }
        "rm" => {
            reject_flags(&label, rest)?;
            let [id, n] = exact(&label, rest)?;
            let wf = edit(&root()?, id, |wf| {
                let i = position(n, wf.steps.len())?;
                wf.steps.remove(i);
                Ok(())
            })?;
            Ok(done(&wf, "updated", json))
        }
        "move" => {
            reject_flags(&label, rest)?;
            let [id, from, to] = exact(&label, rest)?;
            let wf = edit(&root()?, id, |wf| {
                let from = position(from, wf.steps.len())?;
                let to = position(to, wf.steps.len())?;
                let moved = wf.steps.remove(from);
                wf.steps.insert(to, moved);
                Ok(())
            })?;
            Ok(done(&wf, "updated", json))
        }
        other => Err(usage(&format!("unknown step command `{other}`"))),
    }
}

pub fn help_text() -> String {
    "Manage the workflows of a project. The project is the current directory (or the nearest parent \
     with a .agentory folder); pass --project <path> to override. The running app reloads changes.\n\n\
     USAGE:\n  \
     agentory workflow list\n  \
     agentory workflow show ID\n  \
     agentory workflow create NAME --prompt TEXT [--step-name N] [--model M] [--skill S]...\n  \
     agentory workflow apply ID|- [--file PATH]      replace a whole definition from JSON (stdin by default;\n  \
     \u{20}                                              `-` takes the id from the JSON, unknown ids are created)\n  \
     agentory workflow step add ID [--at POS] [--name N] [--prompt P] [--model M] [--skill S]...\n  \
     agentory workflow step set ID POS [--name N] [--prompt P] [--model M] [--skill S]... [--clear-skills]\n  \
     agentory workflow step rm ID POS\n  \
     agentory workflow step move ID FROM TO\n  \
     agentory workflow rename ID NAME\n  \
     agentory workflow delete ID [--yes]\n\n\
     FLAGS:\n  \
     --project PATH   project directory\n  \
     --json           JSON output (the default when stdout is not a terminal)\n  \
     --yes            confirm delete (required on a terminal, ignored otherwise)\n\n\
     JSON SHAPE: {\"id\": \"...\", \"name\": \"...\", \"steps\": [{\"name\": \"...\", \"prompt\": \"...\", \
     \"model\": \"\", \"skills\": []}]}\n\
     Step positions are 1-based. Prompts may use {{input}}, {{prev}} and {{stepN}} (earlier steps only).\n\n\
     Errors go to stderr with a non-zero exit code (1 file error, 2 bad usage or invalid workflow)."
        .to_string()
}
