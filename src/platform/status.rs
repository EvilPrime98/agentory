//! Session status via Claude Code hooks.
//!
//! Each session is started with `claude --settings <generated file>`. The file registers hooks
//! (`Notification`, `Stop`, `UserPromptSubmit`) that write the event name to
//! `<data dir>/Agentory/status/<session id>.status`; the app polls that file. No project or global
//! settings file is touched, and sessions started outside the app are unaffected.

use std::path::PathBuf;

use serde_json::json;

use crate::model::HookEvent;

pub fn status_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Agentory")
        .join("status")
}

fn status_file(session_id: &str) -> PathBuf {
    status_dir().join(format!("{session_id}.status"))
}

fn settings_file(session_id: &str) -> PathBuf {
    status_dir().join(format!("{session_id}.settings.json"))
}

/// Forward slashes work in both Git Bash and PowerShell, and need no JSON escaping.
fn shell_path(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The hook settings document for `session_id`.
pub fn settings_json(session_id: &str) -> serde_json::Value {
    let target = shell_path(&status_file(session_id));
    let hooks: serde_json::Map<String, serde_json::Value> = HookEvent::ALL
        .into_iter()
        .map(|ev| {
            (
                ev.name().to_string(),
                json!([{ "hooks": [{
                    "type": "command",
                    "command": format!("echo {} > '{}'", ev.name(), target),
                }]}]),
            )
        })
        .collect();
    json!({ "hooks": hooks })
}

/// Write the settings file for a session and return its path (for `--settings`).
pub fn write_settings(session_id: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(status_dir())?;
    let path = settings_file(session_id);
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&settings_json(session_id))?,
    )?;
    Ok(path)
}

/// Latest hook event reported for the session, if any.
pub fn read_event(session_id: &str) -> Option<HookEvent> {
    let bytes = std::fs::read(status_file(session_id)).ok()?;
    parse_event(&bytes)
}

/// Windows PowerShell's `>` writes UTF-16: keep printable ASCII only.
fn parse_event(bytes: &[u8]) -> Option<HookEvent> {
    let text: String = bytes
        .iter()
        .filter(|b| b.is_ascii_alphabetic())
        .map(|&b| b as char)
        .collect();
    HookEvent::parse(&text)
}

/// Delete a session's files.
pub fn cleanup(session_id: &str) {
    let _ = std::fs::remove_file(status_file(session_id));
    let _ = std::fs::remove_file(settings_file(session_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_utf8_and_utf16_event_files() {
        assert_eq!(parse_event(b"Stop\r\n"), Some(HookEvent::Stop));
        let utf16: Vec<u8> = "\u{feff}Notification\r\n"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(parse_event(&utf16), Some(HookEvent::Notification));
        assert_eq!(parse_event(b""), None);
    }

    #[test]
    fn settings_register_all_three_hooks() {
        let v = settings_json("abc");
        for ev in HookEvent::ALL {
            let cmd = v["hooks"][ev.name()][0]["hooks"][0]["command"]
                .as_str()
                .unwrap();
            assert!(cmd.contains(ev.name()));
            assert!(cmd.contains("abc.status"));
            assert!(!cmd.contains('\\'));
        }
    }
}
