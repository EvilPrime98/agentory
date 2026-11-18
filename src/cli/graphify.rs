use serde_json::{Value, json};

use super::workflow::{project, take_value};
use super::{SettingError, err, pretty};
use crate::backend::graphify::{self, Status};

type Res<T> = Result<T, SettingError>;

fn usage(message: &str) -> SettingError {
    err(
        "usage",
        format!("{message}. See `agentory graphify --help`"),
    )
}

pub fn dispatch(args: &mut Vec<&str>, json: bool) -> Res<String> {
    let project_arg = take_value(args, "--project")?;
    let Some((&cmd, rest)) = args.split_first() else {
        return Err(usage("missing command"));
    };
    if !matches!(cmd, "status" | "update") {
        return Err(usage(&format!("unknown command `{cmd}`")));
    }
    if !rest.is_empty() {
        return Err(usage(&format!("wrong number of arguments for `{cmd}`")));
    }
    let root = project(project_arg.as_deref())?;
    if cmd == "status" {
        return Ok(status_text(&graphify::status(&root), json));
    }
    if !graphify::status(&root).installed {
        return Err(err("not_installed", "graphify is not installed".into()));
    }
    let out = graphify::update(&root).map_err(|e| err("update_failed", e))?;
    Ok(if json {
        pretty(&json!({ "updated": true, "output": out }))
    } else if out.is_empty() {
        "updated".to_string()
    } else {
        out
    })
}

fn status_text(s: &Status, json: bool) -> String {
    if json {
        return pretty(&serde_json::to_value(s).unwrap_or(Value::Null));
    }
    if !s.installed {
        return "graphify: not installed".to_string();
    }
    let mut lines = vec![format!(
        "graphify: {}",
        s.version.as_deref().unwrap_or("unknown")
    )];
    if !s.graph {
        lines.push("graph: none".to_string());
        return lines.join("\n");
    }
    lines.push(format!(
        "graph: present, {} nodes",
        s.nodes.map_or("?".to_string(), |n| n.to_string())
    ));
    if let Some(b) = s.built_at {
        lines.push(format!("built: {b} (unix seconds)"));
    }
    lines.push(format!("stale: {}", if s.stale { "yes" } else { "no" }));
    lines.join("\n")
}

pub fn help_text() -> String {
    "Show and refresh a project's graphify knowledge graph. The project is the current directory \
     (or the nearest parent with a .agentory folder); pass --project <path> to override.\n\n\
     USAGE:\n  \
     agentory graphify status    installed version, graph present, node count, last built, stale\n  \
     agentory graphify update    run `graphify update .` (AST only, no API cost)\n\n\
     FLAGS:\n  \
     --project PATH   project directory\n  \
     --json           JSON output (the default when stdout is not a terminal)\n\n\
     A full build uses an LLM and is started from the Graphify page in the app (or `/graphify` in a session).\n\
     Errors go to stderr with a non-zero exit code (code `not_installed` when graphify is missing)."
        .to_string()
}
