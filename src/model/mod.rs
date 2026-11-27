//! Plain data types (serde where they cross a file boundary).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

pub const GLOBAL_ID: &str = "global";

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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skill_args: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_shell: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<CustomCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_detected: Vec<String>,
    #[serde(
        default,
        deserialize_with = "string_or_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub docs_dir: Option<String>,
    #[serde(
        default,
        deserialize_with = "string_or_none",
        skip_serializing_if = "Option::is_none"
    )]
    pub issues_dir: Option<String>,
}

pub fn string_or_none<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(serde_json::Value::deserialize(d)?
        .as_str()
        .map(str::to_string))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Docs,
    Issues,
}

impl Folder {
    pub const ALL: [Self; 2] = [Self::Docs, Self::Issues];

    pub fn builtin(self) -> &'static str {
        match self {
            Self::Docs => "docs",
            Self::Issues => "issues",
        }
    }

    pub fn resolve(self, project: Option<&str>, app: &str) -> String {
        project
            .and_then(normalize_folder)
            .or_else(|| normalize_folder(app))
            .unwrap_or_else(|| self.builtin().to_string())
    }
}

pub fn normalize_folder(raw: &str) -> Option<String> {
    let path = raw.trim().replace('\\', "/");
    if path.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part.ends_with(['.', ' ']) || part.contains(':') {
            return None;
        }
        parts.push(part);
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommand {
    pub id: String,
    pub name: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
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
            skill_args: BTreeMap::new(),
            default_shell: None,
            commands: Vec::new(),
            hidden_detected: Vec::new(),
            docs_dir: None,
            issues_dir: None,
        }
    }

    pub fn folder_override(&self, folder: Folder) -> Option<&str> {
        match folder {
            Folder::Docs => self.docs_dir.as_deref(),
            Folder::Issues => self.issues_dir.as_deref(),
        }
    }

    pub fn folder_override_mut(&mut self, folder: Folder) -> &mut Option<String> {
        match folder {
            Folder::Docs => &mut self.docs_dir,
            Folder::Issues => &mut self.issues_dir,
        }
    }

    pub fn global() -> Self {
        Self {
            id: GLOBAL_ID.to_string(),
            name: "Global".to_string(),
            path: dirs::home_dir().unwrap_or_default(),
            use_worktree: None,
            skill_args: BTreeMap::new(),
            default_shell: None,
            commands: Vec::new(),
            hidden_detected: Vec::new(),
            docs_dir: None,
            issues_dir: None,
        }
    }

    pub fn is_global(&self) -> bool {
        self.id == GLOBAL_ID
    }

    pub fn is_git_repo(&self) -> bool {
        !self.is_global() && self.path.join(".git").exists()
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn folder_paths_stay_inside_the_project() {
        for (raw, expected) in [
            ("docs", "docs"),
            (" ./notes//specs/ ", "notes/specs"),
            (r"notes\specs", "notes/specs"),
            (".github/docs", ".github/docs"),
        ] {
            assert_eq!(normalize_folder(raw).as_deref(), Some(expected), "{raw}");
        }
        for raw in [
            "",
            ".",
            "./",
            "..",
            "../docs",
            "docs/../..",
            ".. /docs",
            "...",
            "/docs",
            r"\\server\share",
            r"C:\docs",
            "C:docs",
        ] {
            assert_eq!(normalize_folder(raw), None, "{raw}");
        }
    }

    #[test]
    fn override_beats_default_beats_builtin() {
        let docs = Folder::Docs;
        assert_eq!(docs.resolve(Some("a"), "b"), "a");
        assert_eq!(docs.resolve(None, "b"), "b");
        assert_eq!(docs.resolve(Some("../a"), "b"), "b");
        assert_eq!(docs.resolve(Some("."), "/b"), "docs");
        assert_eq!(Folder::Issues.resolve(None, ""), "issues");
    }

    #[test]
    fn override_of_the_wrong_type_is_ignored() {
        let p: Project = serde_json::from_str(
            r#"{"id":"p","name":"n","path":"/p","docsDir":["x"],"issuesDir":"bugs"}"#,
        )
        .unwrap();
        assert_eq!(p.docs_dir, None);
        assert_eq!(p.issues_dir.as_deref(), Some("bugs"));
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
    /// Whether global skills are hidden in this project. A missing key counts as off.
    pub isolate: bool,
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

/// One skill of the context-cost report.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DoctorSkill {
    pub name: String,
    /// `spm` (declared in `spm.json`), `project` (hand-written) or `global`.
    pub source: String,
    pub description_tokens: u32,
    pub body_tokens: u32,
    /// Invocations in the transcripts window.
    pub uses: u32,
    /// Why it is worth removing: `unused`, `overlaps <skill> (<pct>%)`.
    pub reasons: Vec<String>,
    pub flagged: bool,
}

/// Two skills whose descriptions say much the same.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DoctorOverlap {
    pub a: String,
    pub b: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DoctorReport {
    /// Ranked by cost, highest first.
    pub skills: Vec<DoctorSkill>,
    pub overlaps: Vec<DoctorOverlap>,
    /// Days of transcripts the usage counts cover.
    pub days: u32,
    /// Transcripts read; without any, nothing is flagged as unused.
    pub transcripts: u32,
    pub advice: Vec<String>,
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
