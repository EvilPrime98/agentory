use std::io::IsTerminal as _;

use serde_json::{Value, json};

use crate::registry::{self, SETTINGS, SettingError};
use crate::state::prefs::{self, Map};

const EXIT_FAILURE: i32 = 1;
const EXIT_USAGE: i32 = 2;

mod workflow;

pub const AGENT_PROMPT: &str = "The Agentory app settings can be read and changed with the agentory settings CLI: agentory settings list, get KEY, set KEY VALUE, reset KEY. Run agentory settings --help for the keys and allowed values. Changes apply live in the running app. Project workflows (multi-step prompt pipelines) are managed with the agentory workflow CLI from inside the project: agentory workflow list, show ID, create NAME --prompt TEXT, apply ID|- --file PATH (or JSON on stdin), step add|set|rm|move, rename ID NAME, delete ID. Run agentory workflow --help for details. The running app reloads workflow changes automatically.";

pub fn shim_dir() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.to_str()?;
    let dir = prefs::config_dir()?.join("cli");
    std::fs::create_dir_all(&dir).ok()?;
    let sh = format!("#!/bin/sh\nexec \"{}\" \"$@\"\n", exe.replace('\\', "/"));
    let cmd = format!("@\"{exe}\" %*\r\n");
    for (name, body) in [("agentory", sh), ("agentory.cmd", cmd)] {
        let path = dir.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(&body) {
            std::fs::write(&path, body).ok()?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ =
            std::fs::set_permissions(dir.join("agentory"), std::fs::Permissions::from_mode(0o755));
    }
    Some(dir)
}

pub fn run(args: &[String]) -> Option<i32> {
    let area = args.first().map(String::as_str)?;
    if !matches!(area, "settings" | "workflow") {
        return None;
    }
    attach_console();
    let mut rest: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let json_flag = take_flag(&mut rest, "--json");
    let json = json_flag || !std::io::stdout().is_terminal();
    let help = take_flag(&mut rest, "--help") | take_flag(&mut rest, "-h");
    if help || rest.is_empty() {
        println!(
            "{}",
            if area == "workflow" {
                workflow::help_text()
            } else {
                help_text()
            }
        );
        return Some(if help { 0 } else { EXIT_USAGE });
    }
    let result = if area == "workflow" {
        workflow::dispatch(&mut rest, json)
    } else {
        dispatch(&rest, json)
    };
    Some(match result {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            report(&e, json);
            if e.code == "io" {
                EXIT_FAILURE
            } else {
                EXIT_USAGE
            }
        }
    })
}

fn take_flag(args: &mut Vec<&str>, flag: &str) -> bool {
    let n = args.len();
    args.retain(|a| *a != flag);
    args.len() != n
}

fn err(code: &'static str, message: String) -> SettingError {
    SettingError {
        code,
        message,
        allowed: Vec::new(),
    }
}

fn io(e: std::io::Error) -> SettingError {
    err("io", format!("could not access the prefs file: {e}"))
}

fn usage(message: &str) -> SettingError {
    err(
        "usage",
        format!("{message}. See `agentory settings --help`"),
    )
}

fn dispatch(args: &[&str], json: bool) -> Result<String, SettingError> {
    match args {
        ["list"] => list(json),
        ["get", key] => get(key, json),
        ["set", key, value] => set(key, value, json),
        ["reset", key] => reset(key, json),
        [cmd @ ("list" | "get" | "set" | "reset"), ..] => {
            Err(usage(&format!("wrong number of arguments for `{cmd}`")))
        }
        [other, ..] => Err(usage(&format!("unknown command `{other}`"))),
        [] => Err(usage("missing command")),
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.fract() == 0.0 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn list(json: bool) -> Result<String, SettingError> {
    let file = prefs::read_map().map_err(io)?;
    if json {
        let items: Vec<Value> = SETTINGS
            .iter()
            .map(|s| {
                json!({
                    "key": s.key,
                    "value": s.value_in(&file),
                    "default": s.default_value(),
                    "type": s.kind.name(),
                    "allowed": s.allowed(),
                    "description": s.description,
                })
            })
            .collect();
        return Ok(pretty(&Value::Array(items)));
    }
    Ok(SETTINGS
        .iter()
        .map(|s| format!("{} = {}", s.key, text(&s.value_in(&file))))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn get(key: &str, json: bool) -> Result<String, SettingError> {
    let s = registry::find(key)?;
    let value = s.value_in(&prefs::read_map().map_err(io)?);
    Ok(if json {
        pretty(&json!({ "key": s.key, "value": value }))
    } else {
        text(&value)
    })
}

fn set(key: &str, raw: &str, json: bool) -> Result<String, SettingError> {
    let s = registry::find(key)?;
    let value = s.parse(raw)?;
    let mut file = prefs::read_map().map_err(io)?;
    file.insert(s.field.to_string(), value.clone());
    prefs::write_map(&file).map_err(io)?;
    Ok(changed(s.key, &value, json))
}

fn reset(key: &str, json: bool) -> Result<String, SettingError> {
    let s = registry::find(key)?;
    let mut file: Map = prefs::read_map().map_err(io)?;
    if file.remove(s.field).is_some() {
        prefs::write_map(&file).map_err(io)?;
    }
    Ok(changed(s.key, &s.default_value(), json))
}

fn changed(key: &str, value: &Value, json: bool) -> String {
    if json {
        pretty(&json!({ "key": key, "value": value }))
    } else {
        format!("{key} = {}", text(value))
    }
}

fn report(e: &SettingError, json: bool) {
    if json {
        eprintln!(
            "{}",
            pretty(&json!({ "error": {
                "code": e.code, "message": e.message, "allowed": e.allowed,
            }}))
        );
    } else {
        eprintln!("error: {}", e.message);
        if !e.allowed.is_empty() {
            eprintln!("allowed: {}", e.allowed.join(", "));
        }
    }
}

fn help_text() -> String {
    let mut out = String::from(
        "Read and change Agentory settings. Changes apply live in the running app.\n\n\
         USAGE:\n  agentory settings list\n  agentory settings get KEY\n  \
         agentory settings set KEY VALUE\n  agentory settings reset KEY\n\n\
         FLAGS:\n  --json   JSON output (the default when stdout is not a terminal)\n\n\
         Errors go to stderr with a non-zero exit code (1 file error, 2 bad usage or value).\n\n\
         SETTINGS:\n",
    );
    for s in SETTINGS {
        let allowed = s.allowed();
        let allowed = if allowed.is_empty() {
            String::new()
        } else {
            format!(" [{}]", allowed.join(" | "))
        };
        out.push_str(&format!(
            "  {} ({}){}, default {}\n      {}\n",
            s.key,
            s.kind.name(),
            allowed,
            s.default_value(),
            s.description
        ));
    }
    out.trim_end().to_string()
}

#[cfg(all(windows, not(debug_assertions)))]
fn attach_console() {
    use std::os::windows::io::IntoRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        SetStdHandle,
    };

    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let missing = GetStdHandle(which).map_or(true, |h| h.is_invalid());
            if missing {
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                    let _ = SetStdHandle(which, HANDLE(f.into_raw_handle()));
                }
            }
        }
    }
}

#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_console() {}
