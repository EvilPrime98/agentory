use std::io::IsTerminal as _;
use std::path::Path;

use serde_json::{Value, json};

use super::workflow::take_value;
use super::{SettingError, err, pretty, take_flag};
use crate::backend::scripts;
use crate::backend::workflow::slugify;
use crate::model::{CustomCommand, Project};
use crate::platform::shells;
use crate::state::projects;

type Res<T> = Result<T, SettingError>;

fn usage(message: &str) -> SettingError {
    err(
        "usage",
        format!("{message}. See `agentory commands --help`"),
    )
}

fn io(e: &std::io::Error) -> SettingError {
    err("io", format!("could not access projects.json: {e}"))
}

fn invalid(message: String) -> SettingError {
    err("invalid", message)
}

fn reject_flags(cmd: &str, args: &[&str]) -> Res<()> {
    match args.iter().find(|a| a.starts_with("--")) {
        Some(flag) => Err(usage(&format!("unknown flag `{flag}` for `{cmd}`"))),
        None => Ok(()),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

fn find_project(all: &[Project], explicit: Option<&str>) -> Res<usize> {
    let start = match explicit {
        Some(p) => std::path::absolute(p).map_err(|e| io(&e))?,
        None => std::env::current_dir().map_err(|e| io(&e))?,
    };
    start
        .ancestors()
        .find_map(|dir| {
            all.iter()
                .position(|p| !p.is_global() && same_dir(&p.path, dir))
        })
        .ok_or_else(|| {
            err(
                "not_registered",
                format!(
                    "`{}` is not inside a project registered in Agentory; add it in the app first \
                     or pass --project <path>",
                    start.display()
                ),
            )
        })
}

fn save(all: &[Project]) -> Res<()> {
    let kept: Vec<&Project> = all.iter().collect();
    projects::write_saved(&kept).map_err(|e| io(&e))
}

fn check_shell(id: &str) -> Res<()> {
    let allowed: Vec<String> = shells::detect().iter().map(|s| s.id.to_string()).collect();
    if allowed.iter().any(|a| a == id) {
        return Ok(());
    }
    Err(SettingError {
        code: "invalid",
        message: format!("unknown shell `{id}` on this machine"),
        allowed,
    })
}

fn custom_json(c: &CustomCommand) -> Value {
    json!({
        "id": c.id, "name": c.name, "command": c.command, "shell": c.shell, "source": "custom",
    })
}

fn unique_id(project: &Project, name: &str) -> String {
    let base = if name.chars().any(|c| c.is_ascii_alphanumeric()) {
        slugify(name)
    } else {
        "command".to_string()
    };
    let taken = |id: &str| project.commands.iter().any(|c| c.id == id);
    let mut id = base.clone();
    let mut n = 2;
    while taken(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

pub fn dispatch(args: &mut Vec<&str>, json: bool) -> Res<String> {
    let project_arg = take_value(args, "--project")?;
    let Some((&cmd, rest)) = args.split_first() else {
        return Err(usage("missing command"));
    };
    let mut rest = rest.to_vec();
    let mut all = projects::read_saved().map_err(|e| io(&e))?;
    let at = find_project(&all, project_arg.as_deref())?;
    match cmd {
        "list" => {
            if !rest.is_empty() {
                return Err(usage("`list` takes no arguments"));
            }
            Ok(list(&all[at], json))
        }
        "add" => add(&mut all, at, &mut rest, json),
        "set" => set(&mut all, at, &mut rest, json),
        "rm" => rm(&mut all, at, &mut rest, json),
        "restore" => restore(&mut all, at, &rest, json),
        "shell" => shell(&mut all, at, &mut rest, json),
        other => Err(usage(&format!("unknown command `{other}`"))),
    }
}

fn list(project: &Project, json: bool) -> String {
    let detected = scripts::detect_visible(&project.path, &project.hidden_detected);
    if json {
        let mut items: Vec<Value> = project.commands.iter().map(custom_json).collect();
        items.extend(detected.iter().map(|d| {
            json!({
                "id": null, "name": d.label, "command": d.command, "shell": null,
                "source": "detected",
            })
        }));
        return pretty(&Value::Array(items));
    }
    let mut lines: Vec<String> = project
        .commands
        .iter()
        .map(|c| {
            let shell = c
                .shell
                .as_deref()
                .map_or(String::new(), |s| format!(" [{s}]"));
            format!("{}  {}: {}{shell}  (custom)", c.id, c.name, c.command)
        })
        .collect();
    lines.extend(
        detected
            .iter()
            .map(|d| format!("-  {}: {}  (detected)", d.label, d.command)),
    );
    lines.join("\n")
}

fn add(all: &mut [Project], at: usize, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let command = take_value(rest, "--command")?;
    let shell = take_value(rest, "--shell")?;
    reject_flags("add", rest)?;
    let [name] = rest.as_slice() else {
        return Err(usage("wrong number of arguments for `add`"));
    };
    let name = name.trim();
    let command = command.as_deref().map_or("", str::trim);
    if name.is_empty() {
        return Err(invalid("the command name cannot be empty".into()));
    }
    if command.is_empty() {
        return Err(invalid(
            "the command cannot be empty (pass it with --command)".into(),
        ));
    }
    if let Some(s) = &shell {
        check_shell(s)?;
    }
    let project = &mut all[at];
    let new = CustomCommand {
        id: unique_id(project, name),
        name: name.to_string(),
        command: command.to_string(),
        shell,
    };
    project.commands.push(new.clone());
    save(all)?;
    Ok(done(&new, "added", json))
}

fn set(all: &mut [Project], at: usize, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let name = take_value(rest, "--name")?;
    let command = take_value(rest, "--command")?;
    let shell = take_value(rest, "--shell")?;
    let clear_shell = take_flag(rest, "--clear-shell");
    reject_flags("set", rest)?;
    let [id] = rest.as_slice() else {
        return Err(usage("wrong number of arguments for `set`"));
    };
    if name.is_none() && command.is_none() && shell.is_none() && !clear_shell {
        return Err(usage(
            "nothing to change: pass --name, --command, --shell or --clear-shell",
        ));
    }
    if shell.is_some() && clear_shell {
        return Err(usage("`--shell` and `--clear-shell` cannot be combined"));
    }
    if let Some(s) = &shell {
        check_shell(s)?;
    }
    let project = &mut all[at];
    let Some(slot) = project.commands.iter().position(|c| c.id == *id) else {
        return Err(missing(project, id));
    };
    let cmd = &mut project.commands[slot];
    if let Some(v) = name {
        let v = v.trim();
        if v.is_empty() {
            return Err(invalid("the command name cannot be empty".into()));
        }
        cmd.name = v.to_string();
    }
    if let Some(v) = command {
        let v = v.trim();
        if v.is_empty() {
            return Err(invalid("the command cannot be empty".into()));
        }
        cmd.command = v.to_string();
    }
    if clear_shell {
        cmd.shell = None;
    }
    if shell.is_some() {
        cmd.shell = shell;
    }
    let updated = cmd.clone();
    save(all)?;
    Ok(done(&updated, "updated", json))
}

fn rm(all: &mut [Project], at: usize, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let yes = take_flag(rest, "--yes");
    reject_flags("rm", rest)?;
    let [id] = rest.as_slice() else {
        return Err(usage("wrong number of arguments for `rm`"));
    };
    let project = &mut all[at];
    let slot = project.commands.iter().position(|c| c.id == *id);
    let detected = slot.is_none()
        && scripts::detect_visible(&project.path, &project.hidden_detected)
            .iter()
            .any(|d| d.label == *id);
    if slot.is_none() && !detected {
        return Err(missing(project, id));
    }
    if !yes && std::io::stdout().is_terminal() {
        return Err(err(
            "confirm_required",
            format!("refusing to remove `{id}` on a terminal without --yes"),
        ));
    }
    match slot {
        Some(slot) => {
            project.commands.remove(slot);
        }
        None => project.hidden_detected.push((*id).to_string()),
    }
    save(all)?;
    Ok(if json {
        pretty(&json!({ "id": id, "deleted": true }))
    } else {
        format!("removed {id}")
    })
}

fn restore(all: &mut [Project], at: usize, rest: &[&str], json: bool) -> Res<String> {
    if !rest.is_empty() {
        return Err(usage("`restore` takes no arguments"));
    }
    let count = all[at].hidden_detected.len();
    if count > 0 {
        all[at].hidden_detected.clear();
        save(all)?;
    }
    Ok(if json {
        pretty(&json!({ "restored": count }))
    } else {
        format!("restored {count} detected command(s)")
    })
}

fn shell(all: &mut [Project], at: usize, rest: &mut Vec<&str>, json: bool) -> Res<String> {
    let clear = take_flag(rest, "--clear");
    reject_flags("shell", rest)?;
    let next = match (rest.as_slice(), clear) {
        ([], false) => None,
        ([], true) => Some(None),
        ([name], false) => {
            check_shell(name)?;
            Some(Some((*name).to_string()))
        }
        _ => return Err(usage("pass either a shell name or --clear, not both")),
    };
    if let Some(value) = next {
        all[at].default_shell = value;
        save(all)?;
    }
    let current = all[at].default_shell.as_deref();
    Ok(if json {
        pretty(&json!({ "shell": current }))
    } else {
        current.unwrap_or("(none)").to_string()
    })
}

fn missing(project: &Project, id: &str) -> SettingError {
    if scripts::detect(&project.path).iter().any(|d| d.label == id) {
        err(
            "read_only",
            format!("`{id}` is a detected command; it can only be removed or restored, not edited"),
        )
    } else {
        err("not_found", format!("no command with id `{id}`"))
    }
}

fn done(c: &CustomCommand, verb: &str, json: bool) -> String {
    if json {
        pretty(&custom_json(c))
    } else {
        format!("{verb} {}", c.id)
    }
}

pub fn help_text() -> String {
    "Manage the custom commands of a project and its default shell. The project is the registered \
     project containing the current directory; pass --project <path> to override. Projects are \
     added in the app, never by the CLI. The running app reloads changes.\n\n\
     USAGE:\n  \
     agentory commands list                           custom commands, then detected ones (read-only)\n  \
     agentory commands add NAME --command CMD [--shell S]\n  \
     agentory commands set ID [--name N] [--command CMD] [--shell S] [--clear-shell]\n  \
     agentory commands rm ID|LABEL [--yes]            a detected command is hidden, not deleted\n  \
     agentory commands restore                        show every hidden detected command again\n  \
     agentory commands shell [NAME|--clear]           read, set or clear the default shell\n\n\
     FLAGS:\n  \
     --project PATH   project directory\n  \
     --json           JSON output (the default when stdout is not a terminal)\n  \
     --yes            confirm rm (required on a terminal, ignored otherwise)\n\n\
     Shell values are the ids available on this machine (pwsh, powershell, cmd, git-bash, wsl on \
     Windows; shell elsewhere). Detected commands (package.json, Cargo.toml, ...) have no id: \
     remove them by label (e.g. rm 'cargo build'); they cannot be edited.\n\n\
     Errors go to stderr with a non-zero exit code (1 file error, 2 bad usage or value, including \
     not_registered, not_found and read_only)."
        .to_string()
}
