//! Plain data types (serde where they cross a file boundary).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A folder the user added to the sidebar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Remembered answer to "worktree or project folder?" (`None`: not decided yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_worktree: Option<bool>,
}

impl Project {
    pub fn from_path(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        Self {
            id: new_id("p"),
            name,
            path,
            use_worktree: None,
        }
    }

    pub fn is_git_repo(&self) -> bool {
        self.path.join(".git").exists()
    }
}

/// Short unique id (`prefix` + unix millis + counter), good enough for local file names.
pub fn new_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{prefix}{ms:x}{:x}", N.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Spawned, no hook has reported yet.
    Starting,
    /// A prompt was submitted and Claude is working (`UserPromptSubmit`).
    Working,
    /// Claude is waiting for the user (`Notification`).
    NeedsInput,
    /// Claude finished its turn (`Stop`).
    Idle,
    Exited,
}

/// The Claude Code hook events the app listens to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    Notification,
    Stop,
    UserPromptSubmit,
}

impl HookEvent {
    pub const ALL: [HookEvent; 3] = [Self::Notification, Self::Stop, Self::UserPromptSubmit];

    pub fn name(self) -> &'static str {
        match self {
            Self::Notification => "Notification",
            Self::Stop => "Stop",
            Self::UserPromptSubmit => "UserPromptSubmit",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.name() == s.trim())
    }

    pub fn status(self) -> SessionStatus {
        match self {
            Self::Notification => SessionStatus::NeedsInput,
            Self::Stop => SessionStatus::Idle,
            Self::UserPromptSubmit => SessionStatus::Working,
        }
    }
}

/// `spm.json`. Skill values are a plain repo URL or, for market installs, an object; only the
/// keys matter to the app.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct SpmManifest {
    pub repos: Vec<String>,
    pub skills: BTreeMap<String, serde_json::Value>,
}

impl SpmManifest {
    /// Where a declared skill comes from, for display.
    pub fn source_of(&self, skill: &str) -> Option<String> {
        match self.skills.get(skill)? {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o
                .get("source")
                .or_else(|| o.get("repo"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        }
    }
}

/// One row of `spm doctor --json`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DoctorSkill {
    pub name: String,
    pub description_tokens: u32,
    pub body_tokens: u32,
}

impl Default for DoctorSkill {
    fn default() -> Self {
        Self {
            name: String::new(),
            description_tokens: 0,
            body_tokens: 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DoctorReport {
    pub skills: Vec<DoctorSkill>,
}

impl DoctorReport {
    /// Tokens Claude pays up front for every session: all descriptions.
    pub fn description_tokens(&self) -> u32 {
        self.skills.iter().map(|s| s.description_tokens).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_events_map_to_status() {
        assert_eq!(HookEvent::parse("Stop\r\n"), Some(HookEvent::Stop));
        assert_eq!(HookEvent::Stop.status(), SessionStatus::Idle);
        assert_eq!(HookEvent::Notification.status(), SessionStatus::NeedsInput);
        assert_eq!(HookEvent::parse("nope"), None);
    }

    #[test]
    fn manifest_parses_partial_files() {
        let m: SpmManifest = serde_json::from_str(
            r#"{"repos":["a/b"],"skills":{"x":"https://github.com/a/b","y":{"source":"m:y"}},"isolate":false}"#,
        )
        .unwrap();
        assert_eq!(m.repos, vec!["a/b"]);
        assert_eq!(m.source_of("x").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(m.source_of("y").as_deref(), Some("m:y"));
        let empty: SpmManifest = serde_json::from_str("{}").unwrap();
        assert!(empty.skills.is_empty());
    }

    #[test]
    fn doctor_report_sums_description_tokens() {
        let r: DoctorReport = serde_json::from_str(
            r#"{"days":60,"skills":[{"name":"a","descriptionTokens":10,"bodyTokens":5},{"name":"b","descriptionTokens":7}]}"#,
        )
        .unwrap();
        assert_eq!(r.description_tokens(), 17);
    }
}
